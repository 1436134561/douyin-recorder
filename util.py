"""通用工具：资源路径解析（兼容 PyInstaller onedir 的 _internal）。"""
import os
import sys


def resource_path(rel_path: str) -> str:
    """返回打包后仍正确的资源路径。

    PyInstaller 6 onedir 把资源放在 sys._MEIPASS/_internal/ 下；
    开发模式直接按脚本所在目录解析。
    """
    base = getattr(sys, "_MEIPASS", None)
    if base:
        cand = os.path.join(base, "_internal", rel_path)
        if os.path.exists(cand):
            return cand
        cand2 = os.path.join(base, rel_path)
        if os.path.exists(cand2):
            return cand2
    # 开发模式：相对本文件所在目录
    here = os.path.dirname(os.path.abspath(__file__))
    dev = os.path.join(here, rel_path)
    if os.path.exists(dev):
        return dev
    return dev


def app_path(rel_path: str) -> str:
    """返回应用根目录下的路径（ffmpeg.exe 等随 exe 捆绑的文件）。"""
    base = getattr(sys, "_MEIPASS", None)
    if base:
        for cand in (
            os.path.join(base, "_internal", rel_path),
            os.path.join(base, rel_path),
            os.path.join(base, "_internal", "DouyinRecorder", rel_path),
        ):
            if os.path.exists(cand):
                return cand
    here = os.path.dirname(os.path.abspath(__file__))
    return os.path.join(here, rel_path)
