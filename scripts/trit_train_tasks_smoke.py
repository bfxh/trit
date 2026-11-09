#!/usr/bin/env python3
"""`trit_train_tasks` 可复算 smoke：toy 数据跑 example，校验 WSP1 产物与 JSON 字段。"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "artifacts"
        proc = subprocess.run(
            [
                "cargo",
                "run",
                "--release",
                "-q",
                "-p",
                "trit-wspace",
                "--example",
                "trit_train_tasks",
                "--locked",
                "--",
                "--out",
                str(out),
                "--seeds",
                "1",
                "--hidden",
                "2,3",
                "--activation",
                "tanh",
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
        )
        if proc.returncode != 0:
            print(proc.stderr, file=sys.stderr)
            return 1
        report = json.loads(proc.stdout)
        if report["mode"] != "toy":
            raise AssertionError(report["mode"])
        for key in ("base", "equal", "tuned"):
            mean = report[key]["mean"]
            if not 0.0 <= mean <= 1.0:
                raise AssertionError((key, mean))
        if not report["lambda_scan"]:
            raise AssertionError("lambda_scan 不能为空")
        if "dare" not in report["runs"][0]:
            raise AssertionError("runs[0].dare 缺失")
        if not (out / "seed7" / "base.wsp").is_file():
            raise AssertionError("base.wsp 产物缺失")
        print(
            "OK trit_train_tasks smoke",
            json.dumps(
                {"base": report["base"]["mean"], "tuned": report["tuned"]["mean"]},
                ensure_ascii=False,
            ),
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
