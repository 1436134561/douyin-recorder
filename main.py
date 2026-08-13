"""
程序入口。

优先 pywebview（原生窗口 + Web UI）；webview 不可用则降级到 tkinter (app.py)。
关闭窗口 -> 最小化到托盘；托盘退出 -> 彻底清理 ffmpeg 并 os._exit。
"""
from __future__ import annotations

import os
import sys

import config as config_mod
import controller as controller_mod
import resolver as resolver_mod
import trayicon as trayicon_mod
import webui as webui_mod


def build_controller():
    cfg = config_mod.get_config()
    ctrl = controller_mod.Controller(cfg=cfg, resolver=resolver_mod.resolve)
    # 恢复已保存任务
    for item in cfg.get("tasks", []) or []:
        url = item.get("url") if isinstance(item, dict) else item
        active = item.get("active", True) if isinstance(item, dict) else True
        if url:
            ctrl.add_task(url, active=bool(active))
    ctrl.start()
    return ctrl


def _quit(ctrl, tray):
    try:
        ctrl.shutdown()
    except Exception:
        pass
    try:
        tray.stop()
    except Exception:
        pass
    os._exit(0)


def main():
    ctrl = build_controller()
    api = webui_mod.Api(ctrl)

    # 托盘
    tray = trayicon_mod.Tray(
        on_show=lambda: api.window and (api.window.show(), api.window.restore()),
        on_start_all=ctrl.start_all,
        on_stop_all=ctrl.stop_all,
        on_quit=lambda: _quit(ctrl, tray),
    )
    tray.start()
    webui_mod._tray_ref = tray

    try:
        import webview
    except Exception:
        webui_mod.trayicon_mod_stop = lambda: tray.stop()
        # 降级
        import app
        app.run_tk(ctrl, api)
        return

    from util import resource_path
    url = resource_path("web/index.html")
    window = webview.create_window(
        "DouyinRecorder", url=url, js_api=api,
        width=1100, height=760, min_size=(900, 600),
    )
    api.set_window(window)

    def on_closing():
        # 最小化到托盘而非退出
        try:
            window.minimize()
            window.hide()
        except Exception:
            pass
        return False

    try:
        window.events.closing += on_closing
    except Exception:
        pass

    try:
        webview.start()
    except Exception:
        # 无显示环境：直接阻塞，便于无头冒烟
        import time
        while True:
            time.sleep(1)


if __name__ == "__main__":
    main()
