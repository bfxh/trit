#!/usr/bin/env python3
"""Developer-only CI quality gate for the agent work surface.

The gate wraps the existing PTC driver instead of duplicating its checks:

1. `tools/cmd_driver.py -SelfTest`
2. `tools/cmd_driver.py -Plan testdata/work-bench/ptc-plan.json`

The second command runs the work-bench preflight, 182/182 self-test,
81/81 reference checks, 20/20 negative checks and a two-repeat run, then
persists a local receipt.  This script only reads the receipt and the two
work-bench reports; it does not invent a second scoring layer.

Output is a machine-readable gate result.  It is for CI logs/release
evidence, not for end users: release assets must contain only binaries,
their checksums and user-facing notes.  The detailed receipt is deliberately
kept beside the gate result as a local artifact.

Usage:
    python scripts/agent_quality_gate.py --out target/agent-quality/agent-quality-gate.json
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import pathlib
import re
import subprocess
import sys
from typing import Any


SELF_TEST_RE = re.compile(r"cmd-driver self-test:\s*(\d+)/(\d+)")
DEFAULT_PLAN = "testdata/work-bench/ptc-plan.json"
DRIVER = "tools/cmd_driver.py"
WORK_BENCH_REPORT = "work-bench-report.json"
WORK_BENCH_REPEAT = "work-bench-repeat2.json"
RECEIPT = "ptc-receipt.json"


def _run(argv: list[str], root: pathlib.Path, timeout: int) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            argv,
            cwd=str(root),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
        return proc.returncode, proc.stdout or ""
    except subprocess.TimeoutExpired as exc:
        text = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
        return 124, text
    except OSError as exc:
        return 127, f"{type(exc).__name__}: {exc}"


def _git_sha(root: pathlib.Path) -> str:
    rc, out = _run(["git", "rev-parse", "HEAD"], root, 20)
    return out.strip() if rc == 0 else ""


def _read_json(path: pathlib.Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _write_json(path: pathlib.Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def evaluate(root: pathlib.Path, out: pathlib.Path, timeout: int) -> dict[str, Any]:
    out = out.resolve()
    out.parent.mkdir(parents=True, exist_ok=True)
    reasons: list[str] = []

    self_rc, self_out = _run(
        [sys.executable, DRIVER, "-SelfTest"], root, min(timeout, 180)
    )
    match = SELF_TEST_RE.search(self_out)
    self_passed = int(match.group(1)) if match else 0
    self_total = int(match.group(2)) if match else 0
    self_ok = self_rc == 0 and self_total > 0 and self_passed == self_total
    if not self_ok:
        reasons.append("cmd_driver self-test did not pass")

    receipt_path = out.parent / RECEIPT
    plan_rc, plan_out = _run(
        [
            sys.executable,
            DRIVER,
            "-Plan",
            str(root / DEFAULT_PLAN),
            "-Out",
            str(receipt_path),
            "-Quiet",
        ],
        root,
        timeout,
    )
    receipt = _read_json(receipt_path) or {}
    summary = receipt.get("summary") if isinstance(receipt.get("summary"), dict) else {}
    preflight = receipt.get("preflight") if isinstance(receipt.get("preflight"), list) else []

    steps = summary.get("steps")
    steps_ok = (
        plan_rc == 0
        and summary.get("verdict") == "ok"
        and isinstance(steps, int)
        and steps == 4
        and summary.get("executed") == steps
        and summary.get("passed_steps") == steps
        and summary.get("failed_steps") == 0
    )
    if not steps_ok:
        reasons.append("PTC plan did not complete all four steps")
    verification_ok = (
        summary.get("verification_checks") == 13
        and summary.get("verification_passed") == 13
    )
    if not verification_ok:
        reasons.append("PTC verification checks are not 13/13")
    retries_ok = summary.get("retries") == 0
    if not retries_ok:
        reasons.append("PTC retries not proven zero")
    if summary.get("error_classes"):
        reasons.append("PTC plan recorded error classes")
    preflight_ok = bool(preflight) and all(bool(x.get("ok")) for x in preflight)
    if not preflight_ok:
        reasons.append("PTC preflight failed")

    report = _read_json(out.parent / WORK_BENCH_REPORT) or {}
    repeat = _read_json(out.parent / WORK_BENCH_REPEAT) or {}
    reference = report.get("reference") if isinstance(report.get("reference"), dict) else {}
    negative = report.get("negative") if isinstance(report.get("negative"), dict) else {}
    metrics = report.get("metrics") if isinstance(report.get("metrics"), dict) else {}
    workbench_ok = (
        reference.get("passed") == 81
        and reference.get("total") == 81
        and negative.get("caught") == 20
        and negative.get("total") == 20
        and metrics.get("failed") == 0
        and metrics.get("pass_rate") == 1.0
        and repeat.get("repeat_all_ok") is True
        and (repeat.get("metrics") or {}).get("failed") == 0
    )
    if not workbench_ok:
        reasons.append("work-bench report is not 81/81 + 20/20 and repeat-stable")

    artifact_names = [RECEIPT, WORK_BENCH_REPORT, WORK_BENCH_REPEAT]
    artifacts = []
    for name in artifact_names:
        p = out.parent / name
        artifacts.append(
            {
                "name": name,
                "exists": p.is_file(),
                "bytes": p.stat().st_size if p.is_file() else 0,
            }
        )

    return {
        "schema": "trit-agent-quality-gate/v1",
        "ok": not reasons,
        "developer_only": True,
        "generated_at": _dt.datetime.now(_dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "git_sha": _git_sha(root),
        "reasons": reasons,
        "self_test": {
            "rc": self_rc,
            "passed": self_passed,
            "total": self_total,
            "ok": self_ok,
        },
        "work_bench": {
            "reference_passed": reference.get("passed"),
            "reference_total": reference.get("total"),
            "negative_caught": negative.get("caught"),
            "negative_total": negative.get("total"),
            "failed": metrics.get("failed"),
            "pass_rate": metrics.get("pass_rate"),
            "repeat_all_ok": repeat.get("repeat_all_ok"),
            "ok": workbench_ok,
        },
        "ptc": {
            "rc": plan_rc,
            "steps": steps,
            "executed": summary.get("executed"),
            "passed_steps": summary.get("passed_steps"),
            "failed_steps": summary.get("failed_steps"),
            "verification_passed": summary.get("verification_passed"),
            "verification_checks": summary.get("verification_checks"),
            "retries": summary.get("retries"),
            "error_classes": summary.get("error_classes"),
            "preflight_ok": preflight_ok,
            "wall_s": summary.get("wall_s"),
        },
        "artifacts": artifacts,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description="Developer-only CI gate: work-bench + PTC driver"
    )
    ap.add_argument(
        "--out",
        default="target/agent-quality/agent-quality-gate.json",
        help="gate JSON path (default: target/agent-quality/agent-quality-gate.json)",
    )
    ap.add_argument(
        "--root",
        default=str(pathlib.Path(__file__).resolve().parents[1]),
        help="repository root (default: parent of this script)",
    )
    ap.add_argument(
        "--timeout",
        type=int,
        default=300,
        help="per-command timeout in seconds (default: 300)",
    )
    args = ap.parse_args(argv)

    root = pathlib.Path(args.root).resolve()
    out = pathlib.Path(args.out)
    if not out.is_absolute():
        out = root / out

    gate = evaluate(root, out, args.timeout)
    _write_json(out, gate)
    print(
        "agent-quality: "
        + ("ok" if gate["ok"] else "FAIL")
        + f" / self-test {gate['self_test']['passed']}/{gate['self_test']['total']}"
        + f" / work-bench {gate['work_bench']['reference_passed']}/"
        + f"{gate['work_bench']['reference_total']}"
        + f" + negative {gate['work_bench']['negative_caught']}/"
        + f"{gate['work_bench']['negative_total']}"
        + f" / PTC {gate['ptc']['passed_steps']}/{gate['ptc']['steps']}"
        + f" steps, {gate['ptc']['verification_passed']}/"
        + f"{gate['ptc']['verification_checks']} checks"
        + f" / retries {gate['ptc']['retries']}"
    )
    for reason in gate["reasons"]:
        print(f"  - {reason}", file=sys.stderr)
    print(f"gate: {out}")
    return 0 if gate["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
