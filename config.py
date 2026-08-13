"""DouyinRecorder 配置模块。

负责加载/保存设置，并处理旧版配置的一次性迁移（version 1 -> 2 -> 3）。
所有默认值集中在此，UI 与后端均从这里读取。
"""
import json
import os

APP_NAME = "DouyinRecorder"
CONFIG_VERSION = 3
DEFAULT_OUTPUT = os.path.join(os.path.expanduser("~"), "Videos", "DouyinRecorder")

DEFAULTS = {
    "version": CONFIG_VERSION,
    "output_dir": DEFAULT_OUTPUT,
    "ffmpeg_path": "",            # 空 = 自动检测
    "cookie": "",                 # ttwid / sessionid 等
    "proxy": "",                  # http://user:pass@host:port
    "check_offline_interval": 180,  # 未开播检测间隔（秒）
    # 智能识别
    "smart_record": False,
    "smart_mode": "posture",      # posture | motion | person
    "smart_interval": 1.0,        # 0.5 ~ 5 秒
    "smart_motion_threshold": 1.8,
    "smart_posture_lower": 0.06,
    "smart_posture_span": 0.60,
    "smart_posture_motion": 8.0,
    "smart_posture_keep": 5,      # 站立保持记忆（秒）
    "smart_posture_edge_thr": 0.30,
    "smart_posture_bottom_ratio": 0.45,
    # 资源/转码
    "low_resource": False,
    "auto_transcode": False,
    "transcode_format": "mp4",    # mp4 / mov / mkv / ts
    "transcode_mode": "copy",     # copy | recode
    "transcode_delete_src": False,
    # 其它
    "startup_auto_launch": False,
    "filename_template": "{name}_{time}",
    "tasks": [],                  # [{url, active}]
}


def _deep_merge(base, override):
    out = dict(base)
    for k, v in (override or {}).items():
        if k in base and isinstance(base[k], dict) and isinstance(v, dict):
            out[k] = _deep_merge(base[k], v)
        else:
            out[k] = v
    return out


class Config:
    def __init__(self, path=None):
        self.path = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), "config.json")
        self.data = dict(DEFAULTS)
        self.load()

    # ---- 读写 ----
    def load(self):
        if os.path.exists(self.path):
            try:
                with open(self.path, "r", encoding="utf-8") as f:
                    raw = json.load(f)
                self.data = _deep_merge(DEFAULTS, raw)
            except Exception:
                self.data = dict(DEFAULTS)
        self._migrate()
        return self

    def save(self):
        self.data["version"] = CONFIG_VERSION
        os.makedirs(os.path.dirname(os.path.abspath(self.path)), exist_ok=True)
        with open(self.path, "w", encoding="utf-8") as f:
            json.dump(self.data, f, ensure_ascii=False, indent=2)

    def _migrate(self):
        """一次性迁移旧版配置到当前结构。"""
        ver = self.data.get("version", 1)
        # v1 -> v2：补齐 posture 参数（v3.5 阈值收紧）
        if ver < 2:
            self.data.setdefault("smart_posture_lower", 0.06)
            self.data.setdefault("smart_posture_span", 0.55)
            self.data.setdefault("smart_posture_motion", 8.0)
            self.data.setdefault("smart_posture_keep", 5)
            # 旧版 motion 模式且无 posture 参数 -> 升级为 posture
            if self.data.get("smart_mode") == "motion" and "smart_posture_span" not in self.data:
                self.data["smart_mode"] = "posture"
        # v2 -> v3：新增边缘分带参数，span 0.55 -> 0.60
        if ver < 3:
            self.data.setdefault("smart_posture_edge_thr", 0.30)
            self.data.setdefault("smart_posture_bottom_ratio", 0.45)
            if self.data.get("smart_posture_span", 0) < 0.60:
                self.data["smart_posture_span"] = 0.60
        # 补齐任何缺失的默认键
        for k, v in DEFAULTS.items():
            self.data.setdefault(k, v)
        self.data["version"] = CONFIG_VERSION

    # ---- 便捷访问 ----
    def get(self, key, default=None):
        return self.data.get(key, default)

    def set(self, key, value):
        self.data[key] = value

    def as_dict(self):
        return dict(self.data)

    @property
    def output_dir(self):
        p = self.data.get("output_dir") or DEFAULT_OUTPUT
        return p

    def ensure_output(self):
        os.makedirs(self.output_dir, exist_ok=True)
        return self.output_dir


# 全局单例（后端多模块共享）
_cfg = None


def get_config(path=None):
    global _cfg
    if _cfg is None:
        _cfg = Config(path)
    return _cfg


if __name__ == "__main__":
    c = get_config()
    print(json.dumps(c.as_dict(), ensure_ascii=False, indent=2))
