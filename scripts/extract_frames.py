"""从视频里均匀抽帧，供 `parts-3d look` 量风格/光影特征（数据准备，不参与训练）。

用法：python scripts/extract_frames.py <视频或目录> <输出目录> [间隔秒] [宽]
  · 默认每 4 秒抽一帧，缩到宽 640（等比），存 PNG；
  · 同时写 frames.json（视频名/时长/抽到的帧数），便于复跑对照。

为什么用 cv2：本机没有 ffmpeg；python3.11 带 cv2，解码稳。
注意：游戏/动画素材只在本机做研究与评测，**不进公开数据集、不随仓分发**。
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import cv2


def pick_frames(video: Path, out: Path, every: float, width: int, tag: str) -> list[dict]:
    cap = cv2.VideoCapture(str(video))
    fps = cap.get(cv2.CAP_PROP_FPS) or 30.0
    total = int(cap.get(cv2.CAP_PROP_FRAME_COUNT) or 0)
    dur = total / fps if fps > 0 else 0.0
    step = max(1, int(round(fps * every)))
    stem = video.stem
    rows: list[dict] = []
    idx = 0
    saved = 0
    while True:
        ok, frame = cap.read()
        if not ok:
            break
        if idx % step == 0:
            h, w = frame.shape[:2]
            scale = width / float(w)
            small = cv2.resize(frame, (width, int(round(h * scale))), interpolation=cv2.INTER_AREA)
            # 用 ASCII 名（v{n}_{k}.png）：中文文件名在 cv2/Windows 上会变成乱码
            name = f"{tag}_{idx // step:04d}.png"
            cv2.imwrite(str(out / name), small)
            rows.append({"file": name, "t": round(idx / fps, 2)})
            saved += 1
        idx += 1
    cap.release()
    return [{"video": video.name, "duration_s": round(dur, 1), "frames": saved, "out": rows}]


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    src = Path(sys.argv[1])
    dst = Path(sys.argv[2])
    every = float(sys.argv[3]) if len(sys.argv) > 3 else 4.0
    width = int(sys.argv[4]) if len(sys.argv) > 4 else 640
    dst.mkdir(parents=True, exist_ok=True)
    videos = sorted(src.glob("*.mp4")) if src.is_dir() else [src]
    if not videos:
        print(f"没找到 mp4：{src}")
        return 1
    manifest = []
    for vi, v in enumerate(videos, 1):
        before = len(list(dst.glob("*.png")))
        manifest += pick_frames(v, dst, every, width, f"v{vi}")
        after = len(list(dst.glob("*.png")))
        print(f"{v.name[:30]} 抽帧 {after - before}")
    (dst / "frames.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1), encoding="utf-8")
    total = sum(m["frames"] for m in manifest)
    print(f"FRAMES_DONE {total} → {dst}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
