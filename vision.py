"""
智能识别 SmartAnalyzer（纯 PIL，无 numpy/cv2 依赖）。

三种模式（配置 smart_mode）：
- posture（默认，v3.8 重写）：单帧「水平分带边缘密度」检测，与背景无关。
  站立 = 身体边缘延伸到画面足够低(body_bottom_frac>=span) 且 底部/顶部边缘强度比足够(bottom_ratio>=bottom_ratio)。
  跳舞兜底：下半区也动且变化比例高。首帧仅初始化；滞后缓冲“站立立即录、连续2次确认坐下才停”+ posture_keep 记忆。
- motion：双指标（平均差+显著变化像素比例）+ 滞后缓冲；变化比例>=阈值才录。
- person：无现成检测器时的近似——出现明显运动即视为“有人”，低阈值触发录制。

对外：analyze(frame_pil) -> (active: bool, label: str)
"""
from __future__ import annotations

import math
import time

try:
    from PIL import Image, ImageFilter
    _PIL_OK = True
except ImportError:  # pragma: no cover
    Image = None
    ImageFilter = None
    _PIL_OK = False

BANDS = 12
DIFF_THR = 25          # 帧差判定阈值(0-255)
EDGE_FRAC = 120        # 边缘梯度阈值基数（高斯模糊后梯度被压低，取较低基数）


class SmartAnalyzer:
    def __init__(self, mode: str = "posture", params: dict = None):
        self.mode = mode
        p = params or {}
        self.lower = p.get("smart_posture_lower", 0.06)
        self.span = p.get("smart_posture_span", 0.60)
        self.motion = p.get("smart_posture_motion", 8.0)
        self.keep = p.get("smart_posture_keep", 5)
        self.edge_thr = p.get("smart_posture_edge_thr", 0.30)
        self.bottom_ratio = p.get("smart_posture_bottom_ratio", 0.45)
        self.motion_threshold = p.get("smart_motion_threshold", 1.8)

        self._prev = None            # 上一帧灰度(降采样)
        self._inited = False
        self._standing = False
        self._sit_confirms = 0
        self._last_stand = 0.0
        self._hist = []              # motion 模式滞后缓冲

    # ---------- 帧预处理 ----------
    @staticmethod
    def _prep(frame):
        if frame is None:
            return None
        if not _PIL_OK:
            return None
        if not isinstance(frame, Image.Image):
            try:
                frame = Image.open(frame)
            except Exception:
                return None
        img = frame.convert("L").resize((240, 180))
        img = img.filter(ImageFilter.GaussianBlur(radius=1))
        return img

    @staticmethod
    def _gray_pixels(img):
        return list(img.getdata())

    # ---------- 指标计算 ----------
    def _band_densities(self, px, w=240, h=180):
        """返回 12 条水平带的边缘密度(0-1)。"""
        band_h = h // BANDS
        dens = [0.0] * BANDS
        thr = self.edge_thr * EDGE_FRAC
        for by in range(BANDS):
            top = by * band_h
            bot = top + band_h
            edges = 0
            total = 0
            for y in range(top, min(bot, h - 1)):
                row = y * w
                for x in range(w - 1):
                    i = row + x
                    g = abs(px[i + 1] - px[i]) + abs(px[i + w] - px[i])
                    if g >= thr:
                        edges += 1
                    total += 1
            dens[by] = (edges / total) if total else 0.0
        return dens

    def _changed_ratio(self, cur, prev, w=240, h=180):
        if prev is None:
            return 0.0
        changed = 0
        for i in range(len(cur)):
            if abs(cur[i] - prev[i]) >= DIFF_THR:
                changed += 1
        return 100.0 * changed / len(cur)

    # ---------- posture 判定 ----------
    def _posture_active(self, dens):
        maxd = max(dens) if max(dens) > 0 else 1e-6
        # 找到身体（高边缘带）延伸到的最低带
        lowest = -1
        edge_floor = self.edge_thr * maxd
        for b in range(BANDS):
            if dens[b] >= edge_floor:
                lowest = b
        if lowest < 0:
            return False, "空镜"
        body_bottom_frac = (lowest + 1) / BANDS
        bottom_ratio = dens[BANDS - 1] / dens[0] if dens[0] > 1e-6 else 0.0
        stand = body_bottom_frac >= self.span and bottom_ratio >= self.bottom_ratio
        return stand, f"bottom={body_bottom_frac:.2f},ratio={bottom_ratio:.2f}"

    # ---------- 主入口 ----------
    def analyze(self, frame) -> tuple[bool, str]:
        img = self._prep(frame)
        if img is None:
            return False, "无帧"
        px = self._gray_pixels(img)
        now = time.time()

        if self.mode == "posture":
            if not self._inited:
                self._prev = px
                self._inited = True
                self._standing = False
                return False, "初始化中"
            dens = self._band_densities(px)
            changed = self._changed_ratio(px, self._prev)
            lower_changed = self._lower_changed_ratio(px, self._prev)
            stand, detail = self._posture_active(dens)
            # 跳舞兜底：下半区动且整体变化高
            if (changed >= self.motion) and (lower_changed >= 2.0):
                stand = True
                detail += ",dance"
            if stand:
                self._standing = True
                self._sit_confirms = 0
                self._last_stand = now
                self._prev = px
                return True, "站立/跳舞"
            else:
                # 滞后：posture_keep 记忆期内保持站立
                if (now - self._last_stand) < self.keep:
                    self._prev = px
                    return True, "保持(记忆)"
                self._sit_confirms += 1
                if self._sit_confirms >= 2:
                    self._standing = False
                    self._sit_confirms = 0
                    self._prev = px
                    return False, "坐下/静止"
                self._prev = px
                return True, "坐下待确认"

        elif self.mode == "motion":
            if not self._inited:
                self._prev = px
                self._inited = True
                return False, "初始化中"
            changed = self._changed_ratio(px, self._prev)
            # 双指标：变化像素比例 + 平均差(近似用 changed*scale)
            active = changed >= self.motion_threshold
            self._hist.append(active)
            if len(self._hist) > 3:
                self._hist.pop(0)
            # 一触即发、二静才停
            if active:
                self._prev = px
                return True, f"运动{changed:.1f}%"
            else:
                if len(self._hist) >= 2 and not any(self._hist):
                    self._prev = px
                    return False, f"静止{changed:.1f}%"
                self._prev = px
                return True, f"运动余韵{changed:.1f}%"

        else:  # person：运动即视为有人
            if not self._inited:
                self._prev = px
                self._inited = True
                return False, "初始化中"
            changed = self._changed_ratio(px, self._prev)
            active = changed >= 0.5
            self._prev = px
            return (True, f"有人(运动{changed:.1f}%)") if active else (False, f"无人{changed:.1f}%")

    def _lower_changed_ratio(self, cur, prev, w=240, h=180):
        if prev is None:
            return 0.0
        changed = 0
        total = 0
        for y in range(int(h * 0.6), h):
            row = y * w
            for x in range(w):
                total += 1
                if abs(cur[row + x] - prev[row + x]) >= DIFF_THR:
                    changed += 1
        return 100.0 * changed / total if total else 0.0

    def reset(self):
        self._prev = None
        self._inited = False
        self._standing = False
        self._sit_confirms = 0
        self._hist = []


def available() -> bool:
    return _PIL_OK
