#!/usr/bin/env python3
"""PTC-style command driver: preflight, execute a plan, verify every step.

The point is not to hide failures.  The point is to stop the common failure
loop: one command per model turn, accumulated context, retry after an uncertain
result, and a long transcript that the next turn has to reread.

A plan is a JSON file with argv arrays (no shell quoting), preflight checks,
per-step verification and a local receipt.  The driver runs all steps in one
process, stops at the first failed verification, never retries blindly, and
prints only a compact summary.  Full stdout/stderr stays in the receipt file.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys

# tools/signal.py shadows the stdlib `signal` module when this script is run as
# `python tools/cmd_driver.py` (sys.path[0] is tools/). On POSIX, subprocess's
# timeout path uses signal.SIGKILL, so the shadow makes `-SelfTest` crash after
# the timeout case. cmd_driver imports no local tool modules; drop the script
# dir before importing subprocess so the stdlib module wins.
_SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
if sys.path and os.path.abspath(sys.path[0]) == _SCRIPT_DIR:
    del sys.path[0]

import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

VERIFY_TYPES = {
    "stdout_contains", "stdout_not_contains", "stdout_regex",
    "stderr_contains", "stderr_not_contains",
    "file_exists", "file_absent", "file_nonempty", "file_min_bytes",
    "json_field", "json_keys", "file_count",
}
ERROR_PATTERNS = [
    (re.compile(r"no space left|enospc|disk full|磁盘空间|空间不足", re.I), "disk_full"),
    (re.compile(r"permission denied|access is denied|eperm|eacces", re.I), "permission"),
    (re.compile(r"not recognized|not found|no such file|找不到", re.I), "not_found"),
]


class PlanError(ValueError):
    pass


def _expand(value: Any, variables: dict[str, str]) -> Any:
    if isinstance(value, str):
        def repl(match: re.Match[str]) -> str:
            key = match.group(1)
            if key not in variables:
                raise PlanError(f"unknown plan variable: {key}")
            return variables[key]
        text = re.sub(r"\${([A-Za-z0-9_]+)}", repl, value)
        if "${" in text:
            raise PlanError(f"unexpanded variable in {value!r}")
        return text
    if isinstance(value, list):
        return [_expand(v, variables) for v in value]
    if isinstance(value, dict):
        return {k: _expand(v, variables) for k, v in value.items()}
    return value


def _lookup(data: Any, path: str) -> tuple[bool, Any]:
    if not path:
        return True, data
    cur = data
    for part in path.split("."):
        if isinstance(cur, dict):
            if part not in cur:
                return False, None
            cur = cur[part]
        elif isinstance(cur, list) and part.isdigit():
            idx = int(part)
            if idx >= len(cur):
                return False, None
            cur = cur[idx]
        else:
            return False, None
    return True, cur


def _load_json(path: Path) -> tuple[bool, Any, str]:
    try:
        return True, json.loads(path.read_text(encoding="utf-8-sig")), ""
    except Exception as exc:
        return False, None, f"invalid JSON: {exc}"


def _check_verify(spec: dict[str, Any], stdout: str, stderr: str) -> tuple[bool, str]:
    kind = spec.get("type")
    if kind not in VERIFY_TYPES:
        return False, f"unknown verify type: {kind}"
    try:
        if kind == "stdout_contains":
            want = str(spec.get("value", ""))
            return want in stdout, ("stdout contains" if want in stdout else f"stdout missing {want!r}")
        if kind == "stdout_not_contains":
            bad = str(spec.get("value", ""))
            return bad not in stdout, ("stdout clean" if bad not in stdout else f"stdout contains {bad!r}")
        if kind == "stdout_regex":
            pat = re.compile(str(spec.get("value", "")), re.MULTILINE)
            m = pat.search(stdout)
            return bool(m), (f"stdout regex matched: {m.group(0)!r}" if m else "stdout regex missed")
        if kind == "stderr_contains":
            want = str(spec.get("value", ""))
            return want in stderr, ("stderr contains" if want in stderr else f"stderr missing {want!r}")
        if kind == "stderr_not_contains":
            bad = str(spec.get("value", ""))
            return bad not in stderr, ("stderr clean" if bad not in stderr else f"stderr contains {bad!r}")
        if kind == "file_exists":
            return Path(spec["path"]).exists(), f"exists={Path(spec['path']).exists()}"
        if kind == "file_absent":
            return not Path(spec["path"]).exists(), f"absent={not Path(spec['path']).exists()}"
        if kind == "file_nonempty":
            p = Path(spec["path"])
            return p.is_file() and p.stat().st_size > 0, f"size={p.stat().st_size if p.exists() else 0}"
        if kind == "file_min_bytes":
            p = Path(spec["path"])
            n = p.stat().st_size if p.exists() else 0
            want = int(spec.get("value", 1))
            return n >= want, f"size={n} need>={want}"
        if kind == "file_count":
            p = Path(spec["path"])
            pattern = str(spec.get("pattern", "*"))
            n = len(list(p.glob(pattern))) if p.is_dir() else 0
            want = int(spec.get("value", 0))
            return n == want, f"count={n} need={want}"
        if kind in ("json_field", "json_keys"):
            p = Path(spec["path"])
            ok, data, err = _load_json(p)
            if not ok:
                return False, err
            if kind == "json_keys":
                keys = [str(x) for x in spec.get("values") or []]
                missing = [k for k in keys if k not in data]
                return not missing, ("keys ok" if not missing else "missing keys: " + ", ".join(missing))
            found, actual = _lookup(data, str(spec.get("field", "")))
            if not found:
                return False, f"missing JSON field: {spec.get('field')}"
            if "equals" in spec:
                expected = spec["equals"]
                if isinstance(expected, bool) or isinstance(actual, bool):
                    good = actual is expected
                elif isinstance(expected, (int, float)) and isinstance(actual, (int, float)):
                    good = float(actual) == float(expected)
                else:
                    good = actual == expected
                return good, f"{spec.get('field')}={actual!r} need {expected!r}"
            if "min" in spec:
                good = float(actual) >= float(spec["min"])
                return good, f"{spec.get('field')}={actual!r} need>={spec['min']}"
            if "max" in spec:
                good = float(actual) <= float(spec["max"])
                return good, f"{spec.get('field')}={actual!r} need<={spec['max']}"
            return True, f"{spec.get('field')}={actual!r}"
        return False, f"unhandled verify type: {kind}"
    except Exception as exc:
        return False, f"{kind} raised {type(exc).__name__}: {exc}"


def _classify_error(stderr: str) -> str:
    text = stderr or ""
    for pat, cls in ERROR_PATTERNS:
        if pat.search(text):
            return cls
    return "nonzero"


def _validate_plan(plan: dict[str, Any]) -> list[str]:
    errs: list[str] = []
    if plan.get("schema") != 1:
        errs.append("schema must be 1")
    steps = plan.get("steps")
    if not isinstance(steps, list) or not steps:
        errs.append("steps must be a non-empty list")
        return errs
    ids: set[str] = set()
    for i, step in enumerate(steps, 1):
        if not isinstance(step, dict):
            errs.append(f"step {i} is not an object")
            continue
        sid = str(step.get("id") or "")
        if not sid:
            errs.append(f"step {i} missing id")
        elif sid in ids:
            errs.append(f"duplicate step id: {sid}")
        ids.add(sid)
        argv = step.get("argv")
        if not isinstance(argv, list) or not argv or not all(isinstance(x, str) for x in argv):
            errs.append(f"step {sid or i}: argv must be a non-empty string list")
        for bad_key in ("retry", "retries", "shell"):
            if bad_key in step:
                errs.append(f"step {sid or i}: {bad_key!r} is not allowed; use argv and verify instead")
        verify = step.get("verify") or []
        if not isinstance(verify, list):
            errs.append(f"step {sid or i}: verify must be a list")
        for spec in verify:
            if not isinstance(spec, dict) or spec.get("type") not in VERIFY_TYPES:
                errs.append(f"step {sid or i}: unknown verify type {spec.get('type') if isinstance(spec, dict) else spec!r}")
    return errs


def _preflight(plan: dict[str, Any], variables: dict[str, str], root: Path,
               out_path: Path, min_free_mb: int) -> tuple[list[dict[str, Any]], bool]:
    checks: list[dict[str, Any]] = []

    def add(name: str, ok: bool, detail: str, error_class: str = "") -> None:
        checks.append({"check": name, "ok": ok, "detail": detail, "error_class": error_class if not ok else ""})

    try:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        add("output_dir", True, str(out_path.parent))
    except Exception as exc:
        add("output_dir", False, f"{type(exc).__name__}: {exc}", "disk_full")
        return checks, False

    free = shutil.disk_usage(out_path.parent).free
    need = int(min_free_mb) * 1024 * 1024
    add("free_space", free >= need, f"free={free} need={need}", "disk_low")

    if not root.is_dir():
        add("root", False, f"not a directory: {root}", "bad_cwd")
    else:
        add("root", True, str(root))

    for step in plan.get("steps") or []:
        sid = str(step.get("id"))
        cwd = Path(step.get("cwd") or root)
        if not cwd.is_dir():
            add(f"{sid}.cwd", False, f"not a directory: {cwd}", "bad_cwd")
        else:
            add(f"{sid}.cwd", True, str(cwd))
        argv = step.get("argv") or []
        if argv:
            exe = argv[0]
            if any(sep in exe for sep in ("/", "\\")):
                if not Path(exe).exists():
                    add(f"{sid}.exe", False, f"missing executable: {exe}", "not_found")
                else:
                    add(f"{sid}.exe", True, exe)
            else:
                found = shutil.which(exe)
                if not found:
                    add(f"{sid}.exe", False, f"not on PATH: {exe}", "not_found")
                else:
                    add(f"{sid}.exe", True, found)
    return checks, all(c["ok"] for c in checks)


def _summary_text(receipt: dict[str, Any], quiet: bool, verbose: bool) -> str:
    sm = receipt["summary"]
    lines: list[str] = []
    if verbose:
        for step in receipt.get("steps") or []:
            mark = "PASS" if step["status"] == "ok" else "FAIL"
            lines.append(f"  {mark} {step['id']}: {step['status']} ({step['duration_s']}s)")
    if sm["verdict"] == "ok":
        lines.append(
            f"cmd-driver: {sm['passed_steps']}/{sm['steps']} steps ok / "
            f"quality-gates {sm['verification_passed']}/{sm['verification_checks']} / "
            f"retries {sm['retries']} / errors 0 / context {sm['context_chars']}B "
            f"(local-detail {sm['local_detail_chars']}B) / wall {sm['wall_s']}s"
        )
    else:
        first = sm.get("first_failure") or {}
        detail = (first.get("detail") or "")[:240].replace("\n", " ")
        lines.append(
            f"cmd-driver: FAIL step={first.get('id')} class={first.get('error_class')} "
            f"rc={first.get('rc')} / quality {sm['verification_passed']}/{sm['verification_checks']} "
            f"/ retries {sm['retries']} / wall {sm['wall_s']}s"
            + (f" / {detail}" if detail else "")
        )
    return "\n".join(lines)


def execute_plan(plan: dict[str, Any], root: Path, out_path: Path,
                 min_free_mb: int = 64, quiet: bool = False,
                 verbose: bool = False, dry_run: bool = False) -> tuple[int, dict[str, Any]]:
    errs = _validate_plan(plan)
    if errs:
        print("cmd-driver plan invalid:\n- " + "\n- ".join(errs), file=sys.stderr)
        return 2, {"schema": 1, "verdict": "invalid", "errors": errs}

    variables = {
        "ROOT": str(root),
        "OUTDIR": str(out_path.parent),
        "PLAN_DIR": str(out_path.parent),
    }
    try:
        expanded = _expand(plan, variables)
    except PlanError as exc:
        print(f"cmd-driver plan variable error: {exc}", file=sys.stderr)
        return 2, {"schema": 1, "verdict": "invalid", "errors": [str(exc)]}
    plan = expanded
    out_path.parent.mkdir(parents=True, exist_ok=True)

    pre, pre_ok = _preflight(plan, variables, root, out_path, min_free_mb)
    wall0 = time.perf_counter()
    steps: list[dict[str, Any]] = []
    first_failure: dict[str, Any] | None = None
    per_step_timeout = float(plan.get("timeout_s") or 120.0)
    env = os.environ.copy()
    env["PYTHONUTF8"] = "1"
    env["PYTHONIOENCODING"] = "utf-8"

    if pre_ok and not dry_run:
        for step in plan["steps"]:
            sid = str(step["id"])
            argv = list(step["argv"])
            cwd = Path(step.get("cwd") or root)
            expect_rc = step.get("expect_rc", 0)
            if isinstance(expect_rc, int):
                expected = {expect_rc}
            else:
                expected = {int(x) for x in expect_rc}
            start = time.perf_counter()
            stdout = stderr = ""
            rc: int | None = None
            status = "ok"
            error_class = ""
            try:
                proc = subprocess.run(
                    argv, cwd=str(cwd), env=env, capture_output=True,
                    text=True, encoding="utf-8", errors="replace",
                    timeout=float(step.get("timeout_s", per_step_timeout)),
                )
                stdout, stderr, rc = proc.stdout or "", proc.stderr or "", proc.returncode
                if rc not in expected:
                    status = "failed"
                    error_class = _classify_error(stderr)
            except subprocess.TimeoutExpired as exc:
                status = "timeout"
                error_class = "timeout"
                stdout = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
                stderr = (exc.stderr or "") if isinstance(exc.stderr, str) else ""
            except FileNotFoundError as exc:
                status = "failed"
                error_class = "not_found"
                stderr = str(exc)
            except PermissionError as exc:
                status = "failed"
                error_class = "permission"
                stderr = str(exc)
            except OSError as exc:
                status = "failed"
                error_class = "disk_full" if getattr(exc, "errno", None) == 28 else "os_error"
                stderr = str(exc)

            verifications: list[dict[str, Any]] = []
            if status == "ok":
                for spec in step.get("verify") or []:
                    ok, detail = _check_verify(spec, stdout, stderr)
                    verifications.append({
                        "type": spec.get("type"),
                        "ok": ok,
                        "detail": detail,
                    })
                    if not ok:
                        status = "verify_failed"
                        error_class = "verify_failed"
                        break

            elapsed = round(time.perf_counter() - start, 3)
            row = {
                "id": sid,
                "argv": argv,
                "cwd": str(cwd),
                "rc": rc,
                "status": status,
                "error_class": error_class,
                "duration_s": elapsed,
                "attempts": 1,
                "stdout": stdout,
                "stderr": stderr,
                "stdout_chars": len(stdout),
                "stderr_chars": len(stderr),
                "stdout_tail": stdout[-800:],
                "stderr_tail": stderr[-800:],
                "verifications": verifications,
                "quality": all(v["ok"] for v in verifications) if status == "ok" else False,
            }
            steps.append(row)
            if status != "ok":
                first_failure = {
                    "id": sid,
                    "error_class": error_class,
                    "rc": rc,
                    "status": status,
                    "detail": (stderr or stdout or "")[-800:],
                }
                break

    wall = round(time.perf_counter() - wall0, 3)
    verification_checks = sum(len(s["verifications"]) for s in steps)
    verification_passed = sum(1 for s in steps for v in s["verifications"] if v["ok"])
    errors: dict[str, int] = {}
    for s in steps:
        if s["status"] != "ok":
            errors[s["error_class"]] = errors.get(s["error_class"], 0) + 1
    full_chars = sum(s["stdout_chars"] + s["stderr_chars"] for s in steps)
    local_detail_chars = 0
    seen_paths: set[str] = set()
    for step in plan.get("steps") or []:
        for spec in step.get("verify") or []:
            p = spec.get("path")
            if not p:
                continue
            key = str(p)
            if key in seen_paths:
                continue
            seen_paths.add(key)
            path = Path(key)
            try:
                if path.is_file():
                    local_detail_chars += path.stat().st_size
                elif path.is_dir():
                    local_detail_chars += sum(f.stat().st_size for f in path.glob(str(spec.get("pattern", "*"))) if f.is_file())
            except OSError:
                pass
    verdict = "ok" if pre_ok and not dry_run and not first_failure and len(steps) == len(plan["steps"]) else (
        "dry_run" if dry_run and pre_ok else "failed"
    )
    receipt: dict[str, Any] = {
        "schema": 1,
        "name": plan.get("name"),
        "root": str(root),
        "out": str(out_path),
        "dry_run": dry_run,
        "preflight": pre,
        "steps": steps,
        "summary": {
            "verdict": verdict,
            "steps": len(plan["steps"]),
            "executed": len(steps),
            "passed_steps": sum(1 for s in steps if s["status"] == "ok"),
            "failed_steps": sum(1 for s in steps if s["status"] != "ok"),
            "preflight_failures": sum(1 for c in pre if not c["ok"]),
            "error_classes": errors,
            "retries": 0,
            "uncertain": sum(1 for s in steps if s["status"] == "timeout"),
            "verification_checks": verification_checks,
            "verification_passed": verification_passed,
            "verification_catches": verification_checks - verification_passed,
            "wall_s": wall,
            "full_chars": full_chars,
            "local_detail_chars": local_detail_chars,
            "context_chars": 0,
            "compression_ratio": 0.0,
            "context_vs_local_ratio": 0.0,
            "first_failure": first_failure,
        },
    }
    summary = _summary_text(receipt, quiet, verbose)
    receipt["summary"]["context_chars"] = len(summary)
    receipt["summary"]["compression_ratio"] = round(full_chars / max(1, len(summary)), 2)
    receipt["summary"]["context_vs_local_ratio"] = round(local_detail_chars / max(1, len(summary)), 2)
    # second pass: the printed summary itself contains its own context size
    summary = _summary_text(receipt, quiet, verbose)
    receipt["summary"]["context_chars"] = len(summary)
    receipt["summary"]["compression_ratio"] = round(full_chars / max(1, len(summary)), 2)
    receipt["summary"]["context_vs_local_ratio"] = round(local_detail_chars / max(1, len(summary)), 2)
    try:
        out_path.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except OSError as exc:
        print(f"cmd-driver cannot write receipt: {exc}", file=sys.stderr)
        return 2, receipt
    if not quiet or verdict != "ok":
        print(summary)
    if not pre_ok:
        return 2, receipt
    return (0 if verdict in ("ok", "dry_run") else 1), receipt


# ---------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------

def _plan(name: str, steps: list[dict[str, Any]], **kw: Any) -> dict[str, Any]:
    base = {"schema": 1, "name": name, "steps": steps}
    base.update(kw)
    return base


def self_test() -> int:
    passed = failed = 0
    details: list[str] = []

    def check(cond: bool, label: str, detail: str = "") -> None:
        nonlocal passed, failed
        if cond:
            passed += 1
        else:
            failed += 1
            details.append(f"{label}: {detail}")

    tmp = Path(tempfile.mkdtemp(prefix="cmd-driver-selftest-"))
    out = tmp / "receipt.json"

    def run(name: str, steps: list[dict[str, Any]], **kw: Any) -> tuple[int, dict[str, Any]]:
        return execute_plan(_plan(name, steps, **kw), Path.cwd(), out, quiet=True, min_free_mb=1)

    # 1. two successful steps, verification pass, no retries
    p = _plan("ok", [
        {"id": "a", "argv": [sys.executable, "-c", "print('alpha')"], "verify": [{"type": "stdout_contains", "value": "alpha"}]},
        {"id": "b", "argv": [sys.executable, "-c", "print('beta')"], "verify": [{"type": "stdout_contains", "value": "beta"}]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 0 and r["summary"]["verdict"] == "ok", "success plan", str(r.get("summary")))
    check(r["summary"]["steps"] == 2 and r["summary"]["passed_steps"] == 2, "success step count")
    check(r["summary"]["retries"] == 0 and all(s["attempts"] == 1 for s in r["steps"]), "no retries")
    check(r["summary"]["verification_passed"] == 2, "verification count")

    # 2. nonzero rc stops the plan
    p = _plan("nonzero", [
        {"id": "a", "argv": [sys.executable, "-c", "print('ok')"]},
        {"id": "b", "argv": [sys.executable, "-c", "import sys; sys.exit(9)"]},
        {"id": "c", "argv": [sys.executable, "-c", "print('should not run')"]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 1 and r["summary"]["verdict"] == "failed", "nonzero plan failed")
    check(r["summary"]["first_failure"]["error_class"] == "nonzero", "nonzero class")
    check(r["summary"]["executed"] == 2 and len(r["steps"]) == 2, "stop after failure")
    check(r["steps"][-1]["attempts"] == 1, "failure no retry")

    # 3. verification failure is a quality failure, not a pass
    p = _plan("verify-fail", [
        {"id": "a", "argv": [sys.executable, "-c", "print('bad')"],
         "verify": [{"type": "stdout_contains", "value": "good"}]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 1 and r["summary"]["first_failure"]["error_class"] == "verify_failed", "verify failure")
    check(r["summary"]["verification_catches"] == 1, "verify catches")

    # 4. missing executable is caught before execution
    p = _plan("missing-exe", [{"id": "a", "argv": ["definitely-no-such-cmd-xyz"]}])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 2 and r["summary"]["verdict"] == "failed", "missing exe preflight")
    check(any(c["error_class"] == "not_found" for c in r["preflight"]), "missing exe class")
    check(r["summary"]["executed"] == 0, "missing exe no execution")

    # 5. bad cwd is caught before execution
    p = _plan("bad-cwd", [{"id": "a", "argv": [sys.executable, "-c", "print('x')"], "cwd": str(tmp / "no-such-dir")}])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 2 and any(c["error_class"] == "bad_cwd" for c in r["preflight"]), "bad cwd")
    check(r["summary"]["executed"] == 0, "bad cwd no execution")

    # 6. timeout is uncertain and must not be retried
    p = _plan("timeout", [{"id": "a", "argv": [sys.executable, "-c", "import time; time.sleep(3)"], "timeout_s": 0.5}])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 1 and r["summary"]["first_failure"]["error_class"] == "timeout", "timeout class")
    check(r["summary"]["uncertain"] == 1, "timeout uncertain")
    check(r["summary"]["retries"] == 0 and r["steps"][0]["attempts"] == 1, "timeout no retry")

    # 7. low disk is a preflight failure
    p = _plan("low-disk", [{"id": "a", "argv": [sys.executable, "-c", "print('x')"]}])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=10**9)
    check(rc == 2 and any(c["error_class"] == "disk_low" for c in r["preflight"]), "low disk")
    check(r["summary"]["executed"] == 0, "low disk no execution")

    # 8. JSON field verification pass
    j = tmp / "ok.json"
    p = _plan("json-ok", [
        {"id": "write", "argv": [sys.executable, "-c", f"import json; json.dump({{'a': {{'b': 3}}}}, open(r'{j}', 'w'))"]},
        {"id": "verify", "argv": [sys.executable, "-c", "print('done')"],
         "verify": [{"type": "json_field", "path": str(j), "field": "a.b", "equals": 3}]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 0 and r["steps"][1]["quality"] is True, "json field pass")

    # 9. JSON field failure is a verify failure
    p = _plan("json-fail", [
        {"id": "write", "argv": [sys.executable, "-c", f"import json; json.dump({{'a': 2}}, open(r'{j}', 'w'))"],
         "verify": [{"type": "json_field", "path": str(j), "field": "a", "equals": 3}]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 1 and r["summary"]["first_failure"]["error_class"] == "verify_failed", "json field fail")

    # 10. context summary is much smaller than full logs
    p = _plan("compression", [
        {"id": "noisy", "argv": [sys.executable, "-c", "print('x' * 5000)"],
         "verify": [{"type": "stdout_contains", "value": "x"}]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 0 and r["summary"]["full_chars"] > 4000, "noisy output stored")
    check(r["summary"]["context_chars"] < 500, "summary short")
    check(r["summary"]["compression_ratio"] >= 8, "compression ratio")

    # 11. expected nonzero is a pass, not a runtime error
    p = _plan("expected-rc", [
        {"id": "a", "argv": [sys.executable, "-c", "import sys; print('expected'); sys.exit(5)"], "expect_rc": [5],
         "verify": [{"type": "stdout_contains", "value": "expected"}]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 0 and r["summary"]["verdict"] == "ok", "expected rc pass")
    check(not r["summary"]["error_classes"], "expected rc no error")

    # 12. duplicate ids are invalid
    p = _plan("dup", [{"id": "a", "argv": [sys.executable, "-c", "print(1)"]}, {"id": "a", "argv": [sys.executable, "-c", "print(2)"]}])
    rc, _ = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 2, "duplicate id rejected")

    # 13. unknown verify type is invalid
    p = _plan("unknown-verify", [{"id": "a", "argv": [sys.executable, "-c", "print(1)"], "verify": [{"type": "magic"}]}])
    rc, _ = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 2, "unknown verify rejected")

    # 14. retry is rejected at plan level
    p = _plan("retry", [{"id": "a", "argv": [sys.executable, "-c", "print(1)"], "retry": 3}])
    rc, _ = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 2, "blind retry rejected")

    # 15. dry-run performs preflight only
    marker = tmp / "dry-run-marker.txt"
    p = _plan("dry", [{"id": "a", "argv": [sys.executable, "-c", f"open(r'{marker}', 'w').write('x')"]}])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1, dry_run=True)
    check(rc == 0 and r["summary"]["verdict"] == "dry_run", "dry run verdict")
    check(not marker.exists(), "dry run did not execute")

    # 16. file verifications
    f = tmp / "artifact.txt"
    f.write_text("hello", encoding="utf-8")
    p = _plan("files", [
        {"id": "a", "argv": [sys.executable, "-c", "print('ok')"],
         "verify": [
             {"type": "file_exists", "path": str(f)},
             {"type": "file_nonempty", "path": str(f)},
             {"type": "file_min_bytes", "path": str(f), "value": 3},
             {"type": "file_count", "path": str(tmp), "pattern": "*.txt", "value": 1},
         ]},
    ])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 0 and r["summary"]["verification_passed"] == 4, "file verifications")

    # 17. stderr clean check
    p = _plan("stderr", [{"id": "a", "argv": [sys.executable, "-c", "print('ok')"],
                          "verify": [{"type": "stderr_not_contains", "value": "ERROR"}]}])
    rc, r = execute_plan(p, Path.cwd(), out, quiet=True, min_free_mb=1)
    check(rc == 0 and r["summary"]["verification_passed"] == 1, "stderr clean")

    shutil.rmtree(tmp, ignore_errors=True)
    print(f"cmd-driver self-test: {passed}/{passed + failed}")
    for d in details[:20]:
        print("  -", d)
    if len(details) > 20:
        print(f"  ... {len(details)-20} more")
    return 0 if failed == 0 else 1


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="PTC-style preflight/execute/verify command driver")
    ap.add_argument("-Plan", default=None, help="plan JSON file")
    ap.add_argument("-SelfTest", action="store_true")
    ap.add_argument("-Out", default=None, help="receipt JSON path")
    ap.add_argument("-MinFreeMB", type=int, default=64)
    ap.add_argument("-DryRun", action="store_true")
    ap.add_argument("-Quiet", action="store_true")
    ap.add_argument("-Verbose", action="store_true")
    args = ap.parse_args(argv)
    if args.SelfTest:
        return self_test()
    if not args.Plan:
        ap.print_help()
        return 2
    plan_path = Path(args.Plan)
    if not plan_path.exists():
        print(f"plan not found: {plan_path}", file=sys.stderr)
        return 2
    try:
        plan = json.loads(plan_path.read_text(encoding="utf-8-sig"))
    except Exception as exc:
        print(f"cannot read plan: {exc}", file=sys.stderr)
        return 2
    root = Path(__file__).resolve().parents[1]
    out = Path(args.Out) if args.Out else plan_path.parent / "cmd-driver-receipt.json"
    plan.setdefault("timeout_s", 120)
    return execute_plan(plan, root, out, args.MinFreeMB, args.Quiet, args.Verbose, args.DryRun)[0]


if __name__ == "__main__":
    raise SystemExit(main())
