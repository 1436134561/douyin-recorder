"""
调度核心 Controller。

职责（对照 v3.0 / v3.4 / v3.8 笔记）：
- 加入即监控（active=True）；调度线程按 next_check 时间戳轮询：
  未开播任务每 check_offline_interval 秒检测一次；在线且未工作则启动录制 worker。
- 录制 worker 集成智能门控：smart 模式先 _smart_wait（命中才开录），录制中按 smart_interval
  采样判断，未命中则停止本段；离线/停止则 finalize。
- 分段录制：每段直接写入输出目录 {base}.{idx:04d}.{ext}，边录边出文件（不做自动合并，保留 v3.7 文件行为）。
- shutdown：stop_all -> 等 worker(≤15s) -> 等后台线程(≤8s) -> terminate_all_ffmpeg 兜底。
"""
from __future__ import annotations

import os
import threading
import time
from typing import Callable, Optional

from recorder import FFmpegRecorder, grab_frame, transcode_file, terminate_all_ffmpeg
from vision import SmartAnalyzer
import config as config_mod


def _sanitize(s: str) -> str:
    return "".join(ch for ch in (s or "live") if ch.isalnum() or ch in " _-").strip() or "live"


class Task:
    def __init__(self, tid, url, active=True):
        self.id = tid
        self.url = url
        self.room_id = ""
        self.web_rid = ""
        self.name = ""
        self.active = active
        self.status = "等待解析"      # 等待解析/离线/在线/录制中/已停止/错误
        self.smart_state = ""
        self.stream = None            # 当前选用的流 {url,type}
        self._worker = None
        self._stop = threading.Event()
        self._recorder: Optional[FFmpegRecorder] = None
        self.segments = []            # 本段分片绝对路径
        self.spell_output = ""        # 合并后的最终文件
        self._lock = threading.Lock()
        self.error = ""

    def to_dict(self):
        return {
            "id": self.id,
            "url": self.url,
            "name": self.name or self.web_rid or "解析中",
            "active": self.active,
            "status": self.status,
            "smart_state": self.smart_state,
            "error": self.error,
            "has_output": bool(self.spell_output and os.path.exists(self.spell_output)),
            "output": self.spell_output,
        }


class Controller:
    def __init__(self, cfg: "config_mod.Config" = None, resolver=None):
        self.config = cfg or config_mod.get_config()
        self.resolver = resolver
        self.tasks: dict[int, Task] = {}
        self._id = 0
        self._id_lock = threading.Lock()
        self.lock = threading.Lock()
        self.running = False
        self._shutting_down = False
        self._bg_threads = []
        self._sched = None
        self.logs = []
        self._log_lock = threading.Lock()
        self.log_callback: Optional[Callable] = None
        self.max_logs = 800

    # ---------------- 日志 ----------------
    def _log(self, level: str, msg: str):
        entry = {"t": time.strftime("%H:%M:%S"), "level": level, "msg": msg}
        with self._log_lock:
            self.logs.append(entry)
            if len(self.logs) > self.max_logs:
                self.logs.pop(0)
        if self.log_callback:
            try:
                self.log_callback(entry)
            except Exception:
                pass

    # ---------------- 任务管理 ----------------
    def add_task(self, url: str, active: bool = True) -> int:
        with self._id_lock:
            self._id += 1
            tid = self._id
        t = Task(tid, url, active=active)
        with self.lock:
            self.tasks[tid] = t
        self._log("info", f"添加任务 #{tid}: {url} (active={active})")
        return tid

    def add_tasks(self, urls: list) -> list:
        return [self.add_task(u, True) for u in urls if u.strip()]

    def remove_task(self, tid: int):
        with self.lock:
            t = self.tasks.pop(tid, None)
        if t:
            t.active = False
            t._stop.set()
            self._log("info", f"移除任务 #{tid}")

    def start_task(self, tid: int):
        t = self.tasks.get(tid)
        if t:
            t.active = True
            t._stop.clear()
            self._log("info", f"开始任务 #{tid}")

    def stop_task(self, tid: int):
        t = self.tasks.get(tid)
        if t:
            t.active = False
            t._stop.set()
            self._log("info", f"停止任务 #{tid}")

    def start_all(self):
        for t in self.tasks.values():
            t.active = True
            t._stop.clear()
        self._log("info", "全部开始")

    def stop_all(self):
        for t in self.tasks.values():
            t.active = False
            t._stop.set()
        self._log("info", "全部停止")

    # ---------------- 调度 ----------------
    def start(self):
        if self.running:
            return
        self.running = True
        self._shutting_down = False
        self._sched = threading.Thread(target=self._run_scheduler, daemon=True)
        self._sched.start()
        self._log("info", "调度器已启动")

    def _run_scheduler(self):
        while self.running and not self._shutting_down:
            try:
                with self.lock:
                    items = list(self.tasks.items())
                for tid, t in items:
                    if not t.active or self._shutting_down:
                        continue
                    if t._worker and t._worker.is_alive():
                        continue  # 正在工作
                    # 启动 worker
                    t._stop.clear()
                    w = threading.Thread(target=self._record_worker, args=(t,), daemon=True)
                    t._worker = w
                    w.start()
            except Exception as e:
                self._log("error", f"调度异常: {e}")
            time.sleep(1.0)

    # ---------------- 解析 ----------------
    def _resolve(self, t: Task):
        if self.resolver is None:
            raise RuntimeError("未配置 resolver")
        res = self.resolver(t.url, cookie=self.config.get("cookie", ""),
                            proxy=self.config.get("proxy", ""))
        if not res or not res.get("streams"):
            return None
        t.room_id = res.get("room_id", "")
        t.web_rid = res.get("web_rid", "")
        t.name = res.get("name", "") or t.web_rid
        # 优先 flv，其次 hls
        flv = [s for s in res["streams"] if s["type"] == "flv"]
        hls = [s for s in res["streams"] if s["type"] == "hls"]
        t.stream = (flv or hls)[0]
        return t.stream

    # ---------------- 录制 worker ----------------
    def _record_worker(self, t: Task):
        try:
            self._log("info", f"#{t.id} 解析直播间…")
            try:
                stream = self._resolve(t)
            except Exception as e:
                t.status = "解析失败"
                t.error = str(e)
                self._log("error", f"#{t.id} 解析失败: {e}")
                # 节流：解析失败的房间降低重试频率，避免刷屏/频繁请求
                if t.active and not self._shutting_down:
                    time.sleep(10)
                return
            if not stream:
                t.status = "离线"
                self._log("info", f"#{t.id} 未开播，稍后重试")
                return
            t.status = "在线"
            self._log("info", f"#{t.id} 开播中: {t.name}")

            smart = self.config.get("smart_record", False)
            analyzer = None
            if smart:
                analyzer = SmartAnalyzer(mode=self.config.get("smart_mode", "posture"),
                                          params=self.config.data)

            # 文件名（直接写到输出目录，逐段成文件、不自动合并，保留 v3.7 文件行为）
            ext = "flv" if stream["type"] == "flv" else "ts"
            base = self._make_basename(t)

            while t.active and not self._shutting_down:
                # 智能识别等待命中
                if smart and analyzer is not None:
                    t.status = "智能等待"
                    hit = self._smart_wait(t, analyzer, stream["url"])
                    if not hit:
                        break  # 被停止/离线
                # 录制一段（直接写到输出目录，边录边出文件，不自动合并）
                t.status = "录制中"
                seg_idx = len(t.segments)
                out_path = os.path.join(self.config.output_dir, f"{base}.{seg_idx:04d}.{ext}")
                rec = FFmpegRecorder(ffmpeg="", low_resource=self.config.get("low_resource", False),
                                     proxy=self.config.get("proxy", ""))
                t._recorder = rec
                try:
                    rec.start(stream["url"], out_path, title=t.name)
                    self._log("info", f"#{t.id} 开始录制 -> {os.path.basename(out_path)}")
                    # 录制中采样
                    stop_seg = False
                    while rec.is_running() and not t._stop.is_set() and not self._shutting_down:
                        if smart and analyzer is not None:
                            time.sleep(self.config.get("smart_interval", 1.0))
                            if not self._smart_should(t, analyzer, stream["url"]):
                                self._log("info", f"#{t.id} 智能识别未命中，停止本段")
                                stop_seg = True
                                break
                        else:
                            time.sleep(1.0)
                    rec.stop()
                finally:
                    t._recorder = None
                if rec.out_path and os.path.exists(rec.out_path) and os.path.getsize(rec.out_path) > 0:
                    t.segments.append(rec.out_path)
                    t.spell_output = rec.out_path
                    # 自动转码（v3.8 行为，逐段进行）
                    if self.config.get("auto_transcode", False):
                        self._transcode_after(t)
                else:
                    # 录制失败（流断）
                    if rec.out_path and os.path.exists(rec.out_path):
                        try:
                            os.remove(rec.out_path)
                        except Exception:
                            pass
                if stop_seg:
                    # 智能模式下，未命中后回到等待；若任务仍 active 继续循环
                    if not t.active or self._shutting_down:
                        break
                    continue
                else:
                    # 非智能：录完一段即结束
                    break
            t.status = "已停止" if not t.active else "在线"
        except Exception as e:
            t.status = "错误"
            t.error = str(e)
            self._log("error", f"#{t.id} worker 异常: {e}")
        finally:
            if t._recorder:
                try:
                    t._recorder.stop()
                except Exception:
                    pass
                t._recorder = None

    # ---------------- 智能门控 ----------------
    def _grab_and_analyze(self, analyzer: SmartAnalyzer, src: str, live: bool) -> tuple:
        import tempfile
        tmp = os.path.join(tempfile.gettempdir(), f"smart_{os.getpid()}_{threading.get_ident()}.jpg")
        timeout = 5 if not live else 8
        ok = grab_frame(src, tmp, size=(240, 180), timeout=timeout, live=live)
        if not ok or not os.path.exists(tmp):
            return False, "采样失败"
        try:
            from PIL import Image
            img = Image.open(tmp)
            active, label = analyzer.analyze(img)
        except Exception as e:
            return False, f"分析异常:{e}"
        finally:
            try:
                os.remove(tmp)
            except Exception:
                pass
        return active, label

    def _smart_wait(self, t: Task, analyzer: SmartAnalyzer, stream_url: str) -> bool:
        """等待智能识别命中。返回 False 表示被停止/离线。"""
        interval = max(0.5, self.config.get("smart_interval", 1.0))
        while t.active and not self._shutting_down:
            active, label = self._grab_and_analyze(analyzer, stream_url, live=True)
            t.smart_state = label
            if active:
                self._log("info", f"#{t.id} 智能识别命中: {label}")
                return True
            # 检测是否还在播
            time.sleep(interval)
        return False

    def _smart_should(self, t: Task, analyzer: SmartAnalyzer, stream_url: str) -> bool:
        """录制中采样：safe_on_fail=True（采样失败不误停）。"""
        try:
            active, label = self._grab_and_analyze(analyzer, stream_url, live=True)
            t.smart_state = label
            return active
        except Exception:
            return True

    # ---------------- 分段合并 ----------------
    def _make_basename(self, t: Task) -> str:
        tmpl = self.config.get("filename_template", "{name}_{time}")
        ts = time.strftime("%Y%m%d_%H%M%S")
        name = _sanitize(t.name or t.web_rid or "live")
        return tmpl.format(name=name, time=ts, room_id=t.room_id or t.web_rid or "live")


    def _transcode_after(self, t: Task):
        fmt = self.config.get("transcode_format", "mp4")
        mode = self.config.get("transcode_mode", "copy")
        out = os.path.splitext(t.spell_output)[0] + "." + fmt
        ok = transcode_file(t.spell_output, out, mode=mode, fmt=fmt,
                            low_resource=self.config.get("low_resource", False),
                            delete_src=self.config.get("transcode_delete_src", False))
        if ok:
            self._log("info", f"#{t.id} 转码完成: {os.path.basename(out)}")
            t.spell_output = out

    # ---------------- 状态/日志 ----------------
    def get_state(self) -> dict:
        with self.lock:
            tasks = [t.to_dict() for t in self.tasks.values()]
        return {
            "tasks": tasks,
            "smart_record": self.config.get("smart_record", False),
            "output_dir": self.config.output_dir,
            "running": self.running,
        }

    def get_logs(self, since: int = 0) -> list:
        with self._log_lock:
            if since <= 0:
                return list(self.logs)
            return list(self.logs[since:])

    def list_recordings(self, root: str = None) -> list:
        root = root or self.config.output_dir
        out = []
        for dirpath, dirnames, filenames in os.walk(root):
            # 忽略隐藏目录（如 .segments）
            dirnames[:] = [d for d in dirnames if not d.startswith(".")]
            for fn in filenames:
                if fn.lower().endswith((".mp4", ".flv", ".ts", ".mkv", ".mov")):
                    p = os.path.join(dirpath, fn)
                    out.append({
                        "name": fn,
                        "path": p,
                        "size": os.path.getsize(p),
                        "mtime": os.path.getmtime(p),
                    })
        out.sort(key=lambda x: x["mtime"], reverse=True)
        return out

    # ---------------- 关闭 ----------------
    def shutdown(self):
        self._shutting_down = True
        self.running = False
        self.stop_all()
        # 等 worker
        deadline = time.time() + 15
        for t in list(self.tasks.values()):
            if t._worker and t._worker.is_alive():
                t._worker.join(timeout=max(0.1, deadline - time.time()))
        # 等后台线程
        for th in list(self._bg_threads):
            if th.is_alive():
                th.join(timeout=8)
        terminate_all_ffmpeg()
        self._log("info", "已关闭并清理 ffmpeg")
