"""
FFmpeg 录制/转码/合并/抽帧/剪辑/预览 引擎。

关键点（对照 v3.2 / v3.3 / v3.6 / v3.7 笔记）：
- Windows 下所有 ffmpeg 子进程强制隐藏控制台窗口（_win_hide_kwargs）。
- 模块级 _ACTIVE_PROCS 追踪全部 ffmpeg 进程，退出时 terminate_all_ffmpeg 兜底清理，
  解决“退出后文件夹删不掉 / 程序文件夹被占用”问题。
- 录制用 -c copy 原画，低资源模式 -threads 1 + 收紧 -rtbufsize。
- 提供 transcode_file / merge_videos(concat) / clip_segments(按时间切合并) /
  get_video_info / 实时预览(start/stop/get_frame)。
"""
from __future__ import annotations

import base64
import binascii
import io
import math
import os
import subprocess
import sys
import tempfile
import threading
import time

try:
    from PIL import Image
except ImportError:  # pragma: no cover
    Image = None

from util import app_path

# 追踪所有 ffmpeg 子进程，便于彻底清理
_ACTIVE_PROCS = set()
_PROC_LOCK = threading.Lock()

FFMPEG_BIN = None  # 缓存路径


# ----------------------------------------------------------------------------
# ffmpeg 定位 / 下载
# ----------------------------------------------------------------------------
def find_ffmpeg(explicit: str = "") -> str:
    """按优先级查找 ffmpeg 可执行文件。"""
    global FFMPEG_BIN
    if FFMPEG_BIN and os.path.exists(FFMPEG_BIN):
        return FFMPEG_BIN

    candidates = []
    if explicit:
        candidates.append(explicit)
    # 与 exe 同目录 / _internal
    candidates.append(app_path("ffmpeg.exe"))
    candidates.append(app_path("ffmpeg"))
    # PATH
    from shutil import which
    p = which("ffmpeg")
    if p:
        candidates.append(p)
    # imageio-ffmpeg 可能装过
    try:
        import imageio_ffmpeg
        candidates.append(imageio_ffmpeg.get_ffmpeg_exe())
    except Exception:
        pass
    for c in candidates:
        if c and os.path.exists(c) and os.path.isfile(c):
            FFMPEG_BIN = c
            return c
    return ""


def ensure_ffmpeg(explicit: str = "", force: bool = False) -> str:
    """返回 ffmpeg 路径；缺失时尝试多源下载。"""
    f = find_ffmpeg(explicit)
    if f and not force:
        return f
    # 多源静态构建
    import urllib.request
    urls = [
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
        "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
    ]
    dest = app_path("ffmpeg.exe")
    for u in urls:
        try:
            print(f"[ffmpeg] 尝试下载: {u}")
            urllib.request.urlretrieve(u, dest + ".zip")
            # 解压（需 zipfile，内部含 bin/ffmpeg.exe）
            import zipfile
            with zipfile.ZipFile(dest + ".zip") as z:
                for name in z.namelist():
                    if name.endswith("ffmpeg.exe"):
                        with z.open(name) as src, open(dest, "wb") as out:
                            out.write(src.read())
                        break
            os.remove(dest + ".zip")
            if os.path.exists(dest):
                FFMPEG_BIN = dest
                return dest
        except Exception as e:
            print(f"[ffmpeg] 下载失败: {e}")
    raise RuntimeError("未找到 ffmpeg，且自动下载失败；请手动放置 ffmpeg.exe 到程序目录")


# ----------------------------------------------------------------------------
# Windows 隐藏控制台
# ----------------------------------------------------------------------------
def _win_hide_kwargs():
    kw = {}
    if sys.platform.startswith("win"):
        try:
            CREATE_NO_WINDOW = 0x08000000
            kw["creationflags"] = CREATE_NO_WINDOW
            si = subprocess.STARTUPINFO()
            si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            si.wShowWindow = 0  # SW_HIDE
            kw["startupinfo"] = si
        except Exception:
            pass
    return kw


def _track(proc):
    with _PROC_LOCK:
        _ACTIVE_PROCS.add(proc)
    return proc


def _untrack(proc):
    with _PROC_LOCK:
        _ACTIVE_PROCS.discard(proc)


def terminate_all_ffmpeg():
    """强杀全部追踪中的 ffmpeg，并兜底清理孤儿进程。"""
    with _PROC_LOCK:
        procs = list(_ACTIVE_PROCS)
    for proc in procs:
        try:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=8)
                except Exception:
                    try:
                        import subprocess as _sp
                        _sp.run(["taskkill", "/f", "/t", "/pid", str(proc.pid)],
                                **_win_hide_kwargs(), capture_output=True, timeout=5)
                    except Exception:
                        pass
        except Exception:
            pass
        finally:
            _untrack(proc)
    # 兜底：连任何残留 ffmpeg 一起杀（Windows）
    if sys.platform.startswith("win"):
        try:
            subprocess.run(["taskkill", "/f", "/t", "/im", "ffmpeg.exe"],
                           **_win_hide_kwargs(), capture_output=True, timeout=5)
        except Exception:
            pass


# ----------------------------------------------------------------------------
# 跟踪式运行
# ----------------------------------------------------------------------------
def _run_tracked(cmd, stdin=None, timeout=None, capture=False):
    kw = _win_hide_kwargs()
    if capture:
        kw["stdout"] = subprocess.PIPE
        kw["stderr"] = subprocess.PIPE
    else:
        kw["stdout"] = subprocess.DEVNULL
        kw["stderr"] = subprocess.DEVNULL
    proc = subprocess.Popen(cmd, stdin=stdin, **kw)
    _track(proc)
    try:
        out, err = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        out, err = proc.communicate()
    finally:
        _untrack(proc)
    return proc.returncode, out, err


# ----------------------------------------------------------------------------
# 录制器
# ----------------------------------------------------------------------------
class FFmpegRecorder:
    def __init__(self, ffmpeg: str = "", low_resource: bool = False, proxy: str = ""):
        self.ffmpeg = ffmpeg or find_ffmpeg()
        self.low_resource = low_resource
        self.proxy = proxy
        self.proc = None
        self.out_path = None
        self._reader = None
        self._stderr_buf = []

    def _base_args(self):
        args = [self.ffmpeg, "-y", "-loglevel", "error", "-threads", "1" if self.low_resource else "0"]
        if self.proxy:
            args += ["-http_proxy", self.proxy, "-https_proxy", self.proxy]
        return args

    def start(self, stream_url: str, out_path: str, title: str = ""):
        self.out_path = out_path
        os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
        args = self._base_args() + ["-i", stream_url]
        if self.low_resource:
            args += ["-rtbufsize", "8M"]
        args += ["-c", "copy", "-bsf:a", "aac_adtstosc", "-movflags", "+faststart", out_path]
        if title:
            # 限定 ASCII，避免元数据编码问题
            safe = "".join(ch for ch in title if 32 <= ord(ch) < 127)
            args += ["-metadata", f"title={safe}"]
        kw = _win_hide_kwargs()
        self.proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, **kw)
        _track(self.proc)
        self._reader = threading.Thread(target=self._drain_stderr, daemon=True)
        self._reader.start()
        return self.proc.pid

    def _drain_stderr(self):
        if self.proc is None:
            return
        try:
            for line in self.proc.stderr:
                self._stderr_buf.append(line.decode("utf-8", "replace").strip())
                if len(self._stderr_buf) > 50:
                    self._stderr_buf.pop(0)
        except Exception:
            pass

    def is_running(self):
        return self.proc is not None and self.proc.poll() is None

    def stop(self):
        if self.proc is None:
            return
        try:
            if self.proc.poll() is None:
                self.proc.terminate()
                try:
                    self.proc.wait(timeout=8)
                except Exception:
                    try:
                        subprocess.run(["taskkill", "/f", "/t", "/pid", str(self.proc.pid)],
                                       **_win_hide_kwargs(), capture_output=True, timeout=5)
                    except Exception:
                        pass
        finally:
            if self._reader:
                self._reader.join(timeout=2)
            _untrack(self.proc)
            self.proc = None

    def last_errors(self):
        return list(self._stderr_buf)


# ----------------------------------------------------------------------------
# 抽帧（用于智能识别采样 / 预览）
# ----------------------------------------------------------------------------
def grab_frame(src: str, out_path: str, size: tuple = (240, 180),
               timeout: int = 8, live: bool = False) -> bool:
    """从本地文件或直播流抓取一帧 jpg。成功返回 True。"""
    ffmpeg = find_ffmpeg()
    if not ffmpeg:
        return False
    w, h = size
    args = [ffmpeg, "-y", "-loglevel", "error", "-ss", "0" if not live else "1",
            "-i", src, "-frames:v", "1", "-vf", f"scale={w}:{h}", out_path]
    if live:
        # 直播流抓取避免无限等待
        args = [ffmpeg, "-y", "-loglevel", "error", "-i", src,
                "-frames:v", "1", "-vf", f"scale={w}:{h}", out_path]
    rc, _, err = _run_tracked(args, timeout=timeout)
    return rc == 0 and os.path.exists(out_path)


# ----------------------------------------------------------------------------
# 转码
# ----------------------------------------------------------------------------
def transcode_file(inp: str, out: str, mode: str = "copy", fmt: str = "mp4",
                   low_resource: bool = False, delete_src: bool = False) -> bool:
    ffmpeg = find_ffmpeg()
    if not ffmpeg or not os.path.exists(inp):
        return False
    ext = "." + fmt.lstrip(".")
    if not out.endswith(ext):
        out += ext
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    args = [ffmpeg, "-y", "-loglevel", "error", "-i", inp]
    if mode == "copy":
        args += ["-c", "copy"]
    else:
        args += ["-c", "v", "libx264", "-c:a", "aac", "-preset", "veryfast"]
        if low_resource:
            args += ["-threads", "1"]
    args += [out]
    rc, _, _ = _run_tracked(args, timeout=600)
    ok = rc == 0 and os.path.exists(out) and os.path.getsize(out) > 0
    if ok and delete_src and os.path.abspath(inp) != os.path.abspath(out):
        try:
            os.remove(inp)
        except Exception:
            pass
    return ok


# ----------------------------------------------------------------------------
# 合并（concat demuxer）
# ----------------------------------------------------------------------------
def merge_videos(files: list, out: str, low_resource: bool = False,
                 delete_srcs: bool = False, recode_on_fail: bool = True) -> bool:
    ffmpeg = find_ffmpeg()
    files = [f for f in files if f and os.path.exists(f)]
    if not files:
        return False
    if len(files) == 1:
        # 单分片直接 rename
        try:
            if os.path.abspath(files[0]) != os.path.abspath(out):
                if os.path.exists(out):
                    os.remove(out)
                os.replace(files[0], out)
            ok = True
        except Exception:
            ok = False
        if ok and delete_srcs:
            pass
        return ok
    # 写 concat 列表
    fd, listfile = tempfile.mkstemp(suffix=".txt", prefix="concat_")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as lf:
            for f in files:
                # 用正斜杠，避免 Windows 反斜杠转义问题
                lf.write("file '" + f.replace("\\", "/") + "'\n")
        args = [ffmpeg, "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                "-i", listfile, "-c", "copy", out]
        rc, _, err = _run_tracked(args, timeout=900)
        ok = rc == 0 and os.path.exists(out) and os.path.getsize(out) > 0
        if not ok and recode_on_fail:
            # 流拷贝失败降级重编码
            args2 = [ffmpeg, "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                     "-i", listfile, "-c:v", "libx264", "-c:a", "aac",
                     "-preset", "veryfast", out]
            rc2, _, _ = _run_tracked(args2, timeout=1200)
            ok = rc2 == 0 and os.path.exists(out) and os.path.getsize(out) > 0
    finally:
        try:
            os.remove(listfile)
        except Exception:
            pass
    if ok and delete_srcs:
        for f in files:
            try:
                os.remove(f)
            except Exception:
                pass
    return ok


# ----------------------------------------------------------------------------
# 视频信息 / 剪辑
# ----------------------------------------------------------------------------
def get_video_info(path: str) -> dict:
    """用 ffprobe 获取时长(秒)与大小。"""
    info = {"duration": 0.0, "size": 0, "exists": os.path.exists(path)}
    if not info["exists"]:
        return info
    info["size"] = os.path.getsize(path)
    ffprobe = os.path.join(os.path.dirname(find_ffmpeg() or "."), "ffprobe.exe")
    if not os.path.exists(ffprobe):
        # 退而用 ffmpeg -i 解析（无 ffprobe 时）
        ffprobe = None
    if ffprobe:
        try:
            rc, out, _ = _run_tracked(
                [ffprobe, "-v", "error", "-show_entries", "format=duration",
                 "-of", "default=noprint_wrappers=1:nokey=1", path],
                timeout=20, capture=True)
            if rc == 0 and out:
                info["duration"] = float(out.decode().strip())
        except Exception:
            pass
    return info


def clip_segments(inp: str, segments: list, out: str,
                  low_resource: bool = False) -> bool:
    """
    按多个起止时间段裁剪并合并。
    segments: [{"start": float, "end": float}, ...]（秒）。
    中间未选部分自动剔除。先 -c copy 切 TS 再 concat。
    """
    ffmpeg = find_ffmpeg()
    if not ffmpeg or not os.path.exists(inp) or not segments:
        return False
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    tmp_parts = []
    try:
        for i, seg in enumerate(segments):
            s = max(0.0, float(seg.get("start", 0)))
            e = float(seg.get("end", 0))
            if e <= s:
                continue
            part = os.path.join(tempfile.gettempdir(), f"clip_{os.getpid()}_{i}.ts")
            args = [ffmpeg, "-y", "-loglevel", "error", "-ss", f"{s:.3f}", "-to",
                    f"{e:.3f}", "-i", inp, "-c", "copy", "-f", "mpegts", part]
            rc, _, _ = _run_tracked(args, timeout=600)
            if rc == 0 and os.path.exists(part):
                tmp_parts.append(part)
        if not tmp_parts:
            return False
        ok = merge_videos(tmp_parts, out, low_resource=low_resource, delete_srcs=False)
        return ok
    finally:
        for p in tmp_parts:
            try:
                os.remove(p)
            except Exception:
                pass


# ----------------------------------------------------------------------------
# 实时预览（低画质抽帧）
# ----------------------------------------------------------------------------
_PREVIEWS = {}  # tid -> {"proc", "tmp"}


def start_preview(tid, stream_url: str, width: int = 320, fps: int = 2, q: int = 50):
    ffmpeg = find_ffmpeg()
    if not ffmpeg:
        return False
    stop_preview(tid)
    tmp = os.path.join(tempfile.gettempdir(), f"preview_{tid}.jpg")
    args = [ffmpeg, "-y", "-loglevel", "error", "-i", stream_url,
            "-vf", f"scale={width}:-1,fps={fps}", "-q:v", str(q),
            "-f", "image2", "-update", "1", tmp]
    kw = _win_hide_kwargs()
    proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kw)
    _track(proc)
    _PREVIEWS[tid] = {"proc": proc, "tmp": tmp}
    return True


def stop_preview(tid):
    item = _PREVIEWS.pop(tid, None)
    if not item:
        return
    proc = item["proc"]
    try:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except Exception:
                try:
                    subprocess.run(["taskkill", "/f", "/t", "/pid", str(proc.pid)],
                                   **_win_hide_kwargs(), capture_output=True, timeout=5)
                except Exception:
                    pass
    finally:
        _untrack(proc)


def stop_all_previews():
    for tid in list(_PREVIEWS.keys()):
        stop_preview(tid)


def get_preview_frame(tid: str) -> str:
    """返回当前预览帧的 base64 JPEG 字符串（无则返回空串）。"""
    item = _PREVIEWS.get(tid)
    if not item:
        return ""
    tmp = item["tmp"]
    if not os.path.exists(tmp):
        return ""
    try:
        with open(tmp, "rb") as f:
            return base64.b64encode(f.read()).decode("ascii")
    except Exception:
        return ""


if __name__ == "__main__":
    print("ffmpeg:", find_ffmpeg())
