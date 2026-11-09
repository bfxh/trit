#!/usr/bin/env python3
"""`trit_train_tasks` 可复算 smoke：toy 数据跑 example，校验 WSP1 产物与 JSON 字段。"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def run_example(out: Path, init: str) -> dict:
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
            "--init",
            init,
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        print(proc.stderr, file=sys.stderr)
        raise SystemExit(1)
    return json.loads(proc.stdout)


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "artifacts"
        report = run_example(out, "xorshift")
        if report["mode"] != "toy":
            raise AssertionError(report["mode"])
        if report["data"]["init"] != "xorshift":
            raise AssertionError(report["data"]["init"])
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

        modulo_out = Path(tmp) / "artifacts-modulo"
        modulo_report = run_example(modulo_out, "modulo")
        if modulo_report["data"]["init"] != "modulo":
            raise AssertionError(modulo_report["data"]["init"])
        xor_text = (out / "seed7" / "base.wsp").read_text(encoding="utf-8")
        modulo_text = (modulo_out / "seed7" / "base.wsp").read_text(encoding="utf-8")
        if xor_text == modulo_text:
            raise AssertionError("--init 没有改变初始化")

        scale_lines = [line for line in xor_text.splitlines() if line.startswith("scale ")]
        if not scale_lines:
            raise AssertionError("WSP1 缺少 scale 行")
        scales = [float(token) for line in scale_lines for token in line.split()[1:]]
        if all(abs(scale - 1.0) < 1e-9 for scale in scales):
            raise AssertionError("训练 scale 没有写进 WSP1")

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
