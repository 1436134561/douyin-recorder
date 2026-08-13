"""系统托盘（pystray）。导入失败则优雅降级。"""
from __future__ import annotations

import threading

try:
    import pystray
    from PIL import Image, ImageDraw
    _TRAY_OK = True
except Exception:  # pragma: no cover
    pystray = None
    Image = None
    ImageDraw = None
    _TRAY_OK = False


def _make_icon():
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle((10, 14, 54, 50), fill=(255, 60, 90, 255))
    d.polygon([(28, 24), (28, 40), (44, 32)], fill=(255, 255, 255, 255))
    return img


class Tray:
    def __init__(self, on_show, on_start_all, on_stop_all, on_quit):
        self.on_show = on_show
        self.on_start_all = on_start_all
        self.on_stop_all = on_stop_all
        self.on_quit = on_quit
        self.icon = None
        self._thread = None

    def _build_menu(self):
        return pystray.Menu(
            pystray.MenuItem("显示窗口", lambda: self.on_show()),
            pystray.MenuItem("开始全部", lambda: self.on_start_all()),
            pystray.MenuItem("停止全部", lambda: self.on_stop_all()),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("退出", lambda: self.on_quit()),
        )

    def start(self):
        if not _TRAY_OK:
            return False
        self.icon = pystray.Icon(
            "DouyinRecorder", _make_icon(), "DouyinRecorder", self._build_menu()
        )
        self.icon.double_click = lambda icon, event: self.on_show()
        self._thread = threading.Thread(target=self.icon.run, daemon=True)
        self._thread.start()
        return True

    def stop(self):
        if self.icon:
            try:
                self.icon.stop()
            except Exception:
                pass


def available():
    return _TRAY_OK
