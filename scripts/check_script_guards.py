"""两个评估脚本的超限参数必须在重量级 import 前失败。

本机没有 cv2 / bpy 也能跑：early check 在 import 之前就退出。
CI 的 registry cmd 检查直接调这个脚本。
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def run(script: str, args: list[str]) -> tuple[int, str]:
    proc = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / script), *args],
        capture_output=True,
        text=True,
    )
    return proc.returncode, proc.stdout + proc.stderr


def main() -> int:
    cases = [
        ("render_obj.py", ["--size", "65536"]),
        ("blender_gt.py", ["--", "--res", "65536", "--out", "unused"]),
    ]
    failed = 0
    for script, args in cases:
        code, output = run(script, args)
        if code == 0 or "必须在 1..=" not in output:
            failed += 1
            print(f"FAIL {script}: rc={code} output={output[-200:]!r}")
        else:
            print(f"PASS {script}: 超限参数在 import 前被拒")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
