"""
Web UI 桥接层（pywebview JS-API）。

Api 类的方法即前端 window.pywebview.api.* 可调用接口。
pick_* 对话框依赖 self.window（由 main.py 注入）；无窗口时返回空。
"""
from __future__ import annotations

import os
import sys

from recorder import merge_videos, clip_segments
import trayicon as trayicon_mod

VERSION = "3.9"


class Api:
    def __init__(self, controller):
        self.controller = controller
        self.window = None  # main.py 注入 webview 窗口

    # ---------------- 状态/日志 ----------------
    def get_state(self):
        return self.controller.get_state()

    def get_logs(self, since=0):
        return self.controller.get_logs(int(since or 0))

    def version(self):
        return VERSION

    # ---------------- 任务 ----------------
    def add_task(self, url):
        if not url or not str(url).strip():
            return {"ok": False, "msg": "链接为空"}
        self.controller.add_task(str(url).strip(), True)
        return {"ok": True}

    def add_tasks(self, text):
        lines = [l.strip() for l in str(text).replace("\r", "\n").split("\n") if l.strip()]
        ids = self.controller.add_tasks(lines)
        return {"ok": True, "count": len(ids)}

    def remove_task(self, tid):
        self.controller.remove_task(int(tid))
        return {"ok": True}

    def start_task(self, tid):
        self.controller.start_task(int(tid))
        return {"ok": True}

    def stop_task(self, tid):
        self.controller.stop_task(int(tid))
        return {"ok": True}

    def start_all(self):
        self.controller.start_all()
        return {"ok": True}

    def stop_all(self):
        self.controller.stop_all()
        return {"ok": True}

    # ---------------- 文件/文件夹 ----------------
    def open_folder(self, path=None):
        p = path or self.controller.config.output_dir
        try:
            if sys.platform.startswith("win"):
                os.startfile(p)
            else:
                import subprocess
                subprocess.Popen(["xdg-open", p])
        except Exception as e:
            return {"ok": False, "msg": str(e)}
        return {"ok": True}

    def pick_files(self):
        files = self._dialog("open_file")
        return files or []

    def pick_output(self):
        f = self._dialog("save_file")
        return f[0] if f else ""

    def pick_dir(self):
        d = self._dialog("open_folder")
        return d[0] if d else ""

    def _dialog(self, kind):
        if self.window is None:
            return []
        try:
            import webview
            m = {
                "open_file": webview.OPEN_DIALOG,
                "save_file": webview.SAVE_DIALOG,
                "open_folder": webview.FOLDER_DIALOG,
            }[kind]
            return self.window.create_file_dialog(m)
        except Exception:
            return []

    def load_text_file(self, path):
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as f:
                return {"ok": True, "text": f.read()}
        except Exception as e:
            return {"ok": False, "msg": str(e)}

    # ---------------- 配置 ----------------
    def get_config(self):
        return self.controller.config.as_dict()

    def save_config(self, cfg):
        try:
            for k, v in (cfg or {}).items():
                self.controller.config.set(k, v)
            # 同步持久化当前任务列表
            self.controller.config.set("tasks", [
                {"url": t.url, "active": t.active} for t in self.controller.tasks.values()
            ])
            self.controller.config.save()
            # 应用开机自启
            if "startup_auto_launch" in (cfg or {}):
                self.set_auto_start(bool(cfg["startup_auto_launch"]))
            return {"ok": True}
        except Exception as e:
            return {"ok": False, "msg": str(e)}

    # ---------------- 合并/剪辑 ----------------
    def merge_files(self, files, fmt="mp4", mode="copy", low_resource=False):
        files = [f for f in (files or []) if f]
        if len(files) < 2:
            return {"ok": False, "msg": "请至少选择 2 个文件"}
        out = os.path.splitext(files[0])[0] + "_merged." + fmt
        ok = merge_videos(files, out, low_resource=bool(low_resource), delete_srcs=False)
        return {"ok": ok, "out": out}

    def list_recordings(self):
        return self.controller.list_recordings()

    def clip(self, path, segments, out):
        ok = clip_segments(path, segments, out,
                           low_resource=self.controller.config.get("low_resource", False))
        return {"ok": ok, "out": out}

    # ---------------- 实时预览 ----------------
    def start_preview(self, tid):
        from recorder import start_preview
        t = self.controller.tasks.get(int(tid))
        if not t or not t.stream:
            return {"ok": False, "msg": "无可用流"}
        ok = start_preview(int(tid), t.stream["url"])
        return {"ok": ok}

    def stop_preview(self, tid):
        from recorder import stop_preview
        stop_preview(int(tid))
        return {"ok": True}

    def get_preview_frame(self, tid):
        from recorder import get_preview_frame
        return get_preview_frame(int(tid))

    # ---------------- 开机自启 ----------------
    def get_auto_start(self):
        return {"enabled": self._read_auto_start()}

    def set_auto_start(self, enabled):
        try:
            import winreg
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                                 r"Software\Microsoft\Windows\CurrentVersion\Run", 0,
                                 winreg.KEY_SET_VALUE)
            if enabled:
                exe = self._exe_path()
                winreg.SetValueEx(key, "DouyinRecorder", 0, winreg.REG_SZ, exe)
            else:
                try:
                    winreg.DeleteValue(key, "DouyinRecorder")
                except FileNotFoundError:
                    pass
            winreg.CloseKey(key)
            return {"ok": True}
        except Exception as e:
            return {"ok": False, "msg": str(e)}

    def _read_auto_start(self):
        try:
            import winreg
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                                 r"Software\Microsoft\Windows\CurrentVersion\Run")
            try:
                winreg.QueryValueEx(key, "DouyinRecorder")
                return True
            except FileNotFoundError:
                return False
            finally:
                winreg.CloseKey(key)
        except Exception:
            return False

    def _exe_path(self):
        if getattr(sys, "frozen", False):
            return os.path.abspath(sys.executable)
        # 开发模式：pythonw + 脚本
        return f'"{sys.executable}" "{os.path.abspath(__file__)}"'

    # ---------------- 窗口控制 ----------------
    def minimize_to_tray(self):
        if self.window is not None:
            try:
                self.window.minimize()
                self.window.hide()
            except Exception:
                pass
        return {"ok": True}

    def quit_app(self):
        try:
            self.controller.shutdown()
        except Exception:
            pass
        try:
            trayicon_mod_stop()
        except Exception:
            pass
        os._exit(0)

    def set_window(self, window):
        self.window = window


_tray_ref = None


def trayicon_mod_stop():
    global _tray_ref
    if _tray_ref:
        try:
            _tray_ref.stop()
        except Exception:
            pass
