"""tkinter 降级 GUI（pywebview 不可用时使用）。"""
from __future__ import annotations

import os
import sys
import tkinter as tk
from tkinter import messagebox, scrolledtext


def run_tk(controller, api):
    root = tk.Tk()
    root.title("DouyinRecorder")
    root.geometry("820x560")

    frm = tk.Frame(root)
    frm.pack(fill="x", padx=8, pady=6)
    tk.Label(frm, text="直播间链接:").pack(side="left")
    entry = tk.Entry(frm)
    entry.pack(side="left", fill="x", expand=True, padx=4)
    tk.Button(frm, text="添加", command=lambda: _add(api, entry)).pack(side="left")

    btns = tk.Frame(root)
    btns.pack(fill="x", padx=8)
    tk.Button(btns, text="全部开始", command=controller.start_all).pack(side="left", padx=2)
    tk.Button(btns, text="全部停止", command=controller.stop_all).pack(side="left", padx=2)
    tk.Button(btns, text="打开输出目录", command=lambda: api.open_folder()).pack(side="left", padx=2)

    listbox = tk.Listbox(root)
    listbox.pack(fill="both", expand=True, padx=8, pady=6)

    log = scrolledtext.ScrolledText(root, height=8)
    log.pack(fill="x", padx=8, pady=6)

    def refresh():
        try:
            st = controller.get_state()
            listbox.delete(0, tk.END)
            for t in st["tasks"]:
                listbox.insert(tk.END, f"#{t['id']} {t['name']} | {t['status']} | {t['smart_state']}")
            logs = controller.get_logs()
            log.delete(1.0, tk.END)
            for e in logs[-200:]:
                log.insert(tk.END, f"[{e['t']}] {e['msg']}\n")
        except Exception:
            pass
        root.after(700, refresh)

    def on_select(event):
        sel = listbox.curselection()
        if not sel:
            return
        tid = controller.get_state()["tasks"][sel[0]]["id"]
        menu = tk.Menu(root, tearoff=0)
        menu.add_command(label="开始", command=lambda: controller.start_task(tid))
        menu.add_command(label="停止", command=lambda: controller.stop_task(tid))
        menu.add_command(label="移除", command=lambda: controller.remove_task(tid))
        menu.post(event.x_root, event.y_root)

    listbox.bind("<Button-3>", on_select)
    root.after(500, refresh)
    root.mainloop()


def _add(api, entry):
    url = entry.get().strip()
    if url:
        api.add_task(url)
        entry.delete(0, tk.END)


if __name__ == "__main__":
    import config as config_mod
    import controller as controller_mod
    import resolver as resolver_mod
    import webui as webui_mod
    cfg = config_mod.get_config()
    ctrl = controller_mod.Controller(cfg=cfg, resolver=resolver_mod.resolve)
    ctrl.start()
    api = webui_mod.Api(ctrl)
    run_tk(ctrl, api)
