#!/usr/bin/env python3
"""主播坐立状态 / 小幅度动作检测器。

由 Rust 端通过 ffmpeg 管道喂入 320x240 灰度原始视频帧（rawvideo），
逐帧做人脸尺寸占比 + 帧间动作强度判定，向 stdout 输出 JSON 事件行：
    {"state": "standing"|"sitting"|"unknown", "motion": 0.x, "conf": 0.x}

设计目标：低资源占用（320x240 / 3fps / 灰度 / Haar 或轻量 DNN）。
"""
import argparse
import json
import os
import sys

import cv2
import numpy as np

parser = argparse.ArgumentParser()
parser.add_argument("--width", type=int, default=320)
parser.add_argument("--height", type=int, default=240)
parser.add_argument("--sensitivity", type=float, default=1.0)
args = parser.parse_args()

W, H = args.width, args.height
SENS = max(0.1, args.sensitivity)
BYTES_PER_FRAME = W * H

# ---- 人脸检测模型（优先轻量 DNN，回退 Haar 级联）----
_face_net = None
_haar = None

_model = os.path.join(os.path.dirname(__file__), "opencv_face_detector_uint8.pb")
_config = os.path.join(os.path.dirname(__file__), "opencv_face_detector.pbtxt")
if os.path.exists(_model) and os.path.exists(_config):
    try:
        _face_net = cv2.dnn.readNetFromTensorflow(_model, _config)
    except Exception:
        _face_net = None

if _face_net is None:
    _haar_path = cv2.data.haarcascades + "haarcascade_frontalface_default.xml"
    _haar = cv2.CascadeClassifier(_haar_path)


def detect_faces(gray: np.ndarray):
    if _face_net is not None:
        blob = cv2.dnn.blobFromImage(gray, 1.0, (W, H), (104.0, 177.0, 123.0))
        _face_net.setInput(blob)
        dets = _face_net.forward()
        boxes = []
        for i in range(dets.shape[2]):
            conf = float(dets[0, 0, i, 2])
            if conf > 0.5:
                x1 = int(dets[0, 0, i, 3] * W)
                y1 = int(dets[0, 0, i, 4] * H)
                x2 = int(dets[0, 0, i, 5] * W)
                y2 = int(dets[0, 0, i, 6] * H)
                boxes.append((x1, y1, x2, y2))
        return boxes
    faces = _haar.detectMultiScale(gray, 1.3, 5)
    return [(int(x), int(y), int(x + w), int(y + h)) for (x, y, w, h) in faces]


def main():
    prev = None
    buf = b""
    fp = sys.stdin.buffer

    while True:
        # 补足一帧
        while len(buf) < BYTES_PER_FRAME:
            chunk = fp.read(BYTES_PER_FRAME - len(buf))
            if not chunk:
                return
            buf += chunk

        frame = np.frombuffer(buf[:BYTES_PER_FRAME], dtype=np.uint8).reshape((H, W))
        buf = buf[BYTES_PER_FRAME:]

        gray = frame
        boxes = detect_faces(gray)

        motion = 0.0
        if prev is not None:
            # 使用“变化像素占比”而非全局均值差：对局部小幅动作（摆手等）更敏感
            diff = cv2.absdiff(prev, gray)
            _, th = cv2.threshold(diff, 18, 255, cv2.THRESH_BINARY)
            motion = float(np.count_nonzero(th)) / float(th.size)
        prev = gray.copy()

        state = "unknown"
        conf = 0.2

        if boxes:
            # 取最大人脸
            x1, y1, x2, y2 = max(boxes, key=lambda b: (b[2] - b[0]) * (b[3] - b[1]))
            fh = (y2 - y1) / H  # 人脸高度占画面比例
            # 占比大 -> 头部离镜头近 -> 坐下；占比小 -> 站远 -> 站立
            sit_thr = 0.33 / SENS
            stand_thr = 0.20 / SENS
            if fh > sit_thr:
                state = "sitting"
                conf = min(1.0, (fh - sit_thr) / sit_thr + 0.5)
            elif fh < stand_thr:
                state = "standing"
                conf = min(1.0, (stand_thr - fh) / stand_thr + 0.5)
            else:
                state = "unknown"
                conf = 0.4
        else:
            state = "unknown"
            conf = 0.2

        # 有明显动作（如摆手、轻微摆动）视为“在场/站立”，保持录制
        motion_thr = 0.006 / SENS
        if motion > motion_thr:
            state = "standing"
            conf = max(conf, min(1.0, motion * 5.0))

        out = {
            "state": state,
            "motion": round(motion, 4),
            "conf": round(conf, 3),
        }
        sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
