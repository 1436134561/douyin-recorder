# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller onedir 打包配置（console=False，自包含 ffmpeg + web）。"""
import os

block_cipher = None

added_files = [("web", "web")]
binaries = []
# 项目根目录放一份 ffmpeg.exe，自动捆绑进 _internal/
if os.path.exists("ffmpeg.exe"):
    binaries.append(("ffmpeg.exe", "."))

a = Analysis(
    ["main.py"],
    pathex=[],
    binaries=binaries,
    datas=added_files,
    hiddenimports=[
        "ab_sign", "resolver", "recorder", "config", "controller",
        "vision", "app", "trayicon", "pystray", "PIL", "webui",
        "webview", "webview.platforms", "webview.platforms.cef", "bottle",
        "requests",
    ],
    hookspath=[],
    runtime_hooks=[],
    excludes=[],
    win_no_prefer_redirects=False,
    win_private_assemblies=False,
    cipher=block_cipher,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz, a.scripts, [],
    exclude_binaries=True,
    name="DouyinRecorder",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=False,
    icon=None,
)

coll = COLLECT(
    exe, a.binaries, a.zipfiles, a.datas,
    name="DouyinRecorder",
)
