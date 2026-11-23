#!/usr/bin/env python3
"""Deterministic work-task benchmark for agent deliverables.

Unlike a code benchmark, a work task is judged by the artifact it leaves behind:
a document, JSON record, CSV table, native XLSX workbook or a file tree.  The
same task therefore has a reference artifact, a deliberately defective negative
artifact, and a small set of deterministic checks.  No model call is involved.
"""
from __future__ import annotations

import argparse
import csv
import glob
import io
import json
import os
import re
import shutil
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path
from typing import Any

SUITE = "work-bench"
ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TASKS = ROOT / "testdata" / "work-bench" / "tasks.json"
CHECK_TYPES = {
    "md_sections", "md_phrases", "md_min_chars", "md_forbidden",
    "json_keys", "json_value", "json_min_chars", "json_contains", "json_forbidden",
    "json_list_len", "json_list_field_min", "json_no_overlap", "json_dag",
    "csv_columns", "csv_row_count", "csv_no_nulls", "csv_lookup",
    "xlsx_sheet", "xlsx_cell", "xlsx_formula",
    "files_exist", "files_absent",
}


def _p(value: Any) -> str:
    return str(value)


def _safe_join(root: Path, rel: str) -> Path:
    rel = rel.replace("\\", "/")
    if rel.startswith("/") or ":" in rel or ".." in Path(rel).parts:
        raise ValueError(f"unsafe artifact path: {rel!r}")
    target = (root / rel).resolve()
    if root.resolve() not in target.parents and target != root.resolve():
        raise ValueError(f"artifact escapes root: {rel!r}")
    return target


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8-sig", errors="replace")


def _nonspace_len(text: str) -> int:
    return len(re.findall(r"\S", text))


def _equal(actual: Any, expected: Any) -> bool:
    if expected is None:
        return actual is None
    if isinstance(expected, bool) or isinstance(actual, bool):
        return actual is expected
    if isinstance(expected, (int, float)) and isinstance(actual, (int, float)):
        return float(actual) == float(expected)
    return actual == expected


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
        return True, json.loads(_read(path)), ""
    except Exception as exc:
        return False, None, f"invalid JSON: {exc}"


# ---------------------------------------------------------------------------
# Artifact materialization
# ---------------------------------------------------------------------------

def write_xlsx(path: Path, spec: dict[str, Any]) -> None:
    try:
        from openpyxl import Workbook
    except ImportError as exc:  # pragma: no cover - depends on host
        raise RuntimeError("openpyxl is required for native XLSX tasks") from exc
    path.parent.mkdir(parents=True, exist_ok=True)
    wb = Workbook()
    wb.remove(wb.active)
    sheets = spec.get("sheets")
    if isinstance(sheets, dict):
        sheets = [{"name": name, "rows": rows} for name, rows in sheets.items()]
    if not sheets:
        raise ValueError("xlsx spec needs sheets")
    for sheet in sheets:
        ws = wb.create_sheet(str(sheet.get("name") or "Sheet1"))
        for row in sheet.get("rows", []):
            ws.append(row)
    wb.save(path)


def materialize(root: Path, rel: str, spec: Any) -> None:
    target = _safe_join(root, rel)
    target.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(spec, dict) and spec.get("format") == "xlsx":
        write_xlsx(target, spec)
    elif isinstance(spec, str):
        target.write_text(spec, encoding="utf-8", newline="\n")
    else:
        raise ValueError(f"unsupported artifact spec for {rel}: {type(spec).__name__}")


def materialize_inputs(task: dict[str, Any], root: Path) -> None:
    for rel, content in (task.get("inputs") or {}).items():
        materialize(root, rel, content)


# ---------------------------------------------------------------------------
# Check evaluators
# ---------------------------------------------------------------------------

def eval_check(artifact: Path, check: dict[str, Any]) -> tuple[bool, str]:
    if not artifact.exists():
        return False, f"missing artifact: {artifact.name}"
    kind = check.get("type")
    values = check.get("values") or []
    try:
        if kind == "md_sections":
            text = _read(artifact)
            lines = [ln.strip() for ln in text.splitlines()]
            missing = [v for v in values if not any(ln.startswith(v) for ln in lines)]
            return (not missing), ("sections ok" if not missing else "missing sections: " + ", ".join(missing))
        if kind == "md_phrases":
            text = _read(artifact)
            missing = [v for v in values if v not in text]
            return (not missing), ("phrases ok" if not missing else "missing phrases: " + ", ".join(missing))
        if kind == "md_min_chars":
            n = _nonspace_len(_read(artifact))
            want = int(check.get("value", 0))
            return n >= want, f"non-space chars={n} need>={want}"
        if kind == "md_forbidden":
            text = _read(artifact)
            bad = [v for v in values if v in text]
            return (not bad), ("forbidden absent" if not bad else "forbidden present: " + ", ".join(bad))

        if kind == "json_keys":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            if not isinstance(data, dict):
                return False, "JSON root is not an object"
            missing = [v for v in values if v not in data]
            return (not missing), ("keys ok" if not missing else "missing keys: " + ", ".join(missing))
        if kind == "json_value":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            found, actual = _lookup(data, str(check.get("path", "")))
            if not found:
                return False, f"missing JSON path: {check.get('path')}"
            expected = check.get("equals")
            return _equal(actual, expected), f"{check.get('path')}={actual!r} need {expected!r}"
        if kind == "json_min_chars":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            found, actual = _lookup(data, str(check.get("path", "")))
            if not found:
                return False, f"missing JSON path: {check.get('path')}"
            n = _nonspace_len(str(actual))
            want = int(check.get("value", 0))
            return n >= want, f"{check.get('path')} chars={n} need>={want}"
        if kind in ("json_contains", "json_forbidden"):
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            blob = json.dumps(data, ensure_ascii=False, sort_keys=True)
            found = [v for v in values if v in blob]
            if kind == "json_contains":
                missing = [v for v in values if v not in blob]
                return (not missing), ("values present" if not missing else "missing values: " + ", ".join(missing))
            return (not found), ("forbidden absent" if not found else "forbidden present: " + ", ".join(found))
        if kind == "json_list_len":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            found, actual = _lookup(data, str(check.get("path", "")))
            if not found or not isinstance(actual, list):
                return False, f"not a list: {check.get('path')}"
            n = len(actual)
            if "equals" in check:
                return n == int(check["equals"]), f"list len={n} need={check['equals']}"
            lo = int(check.get("min", 0))
            hi = int(check["max"]) if "max" in check else 10**9
            return lo <= n <= hi, f"list len={n} need[{lo},{hi}]"
        if kind == "json_list_field_min":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            found, actual = _lookup(data, str(check.get("path", "")))
            if not found or not isinstance(actual, list):
                return False, f"not a list: {check.get('path')}"
            field = str(check.get("field"))
            want = int(check.get("min", 1))
            bad = [i for i, item in enumerate(actual) if not isinstance(item, dict) or not isinstance(item.get(field), list) or len(item[field]) < want]
            return (not bad), ("nested lists ok" if not bad else f"slides/items too short: {bad}")
        if kind == "json_no_overlap":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            found, actual = _lookup(data, str(check.get("path", "")))
            if not found or not isinstance(actual, list):
                return False, f"not a list: {check.get('path')}"
            intervals = []
            sf, ef = str(check.get("start_field")), str(check.get("end_field"))
            for item in actual:
                if not isinstance(item, dict) or sf not in item or ef not in item:
                    return False, "schedule item missing start/end"
                start = datetime.fromisoformat(str(item[sf]).replace("Z", "+00:00"))
                end = datetime.fromisoformat(str(item[ef]).replace("Z", "+00:00"))
                if end <= start:
                    return False, f"non-positive interval: {item}"
                intervals.append((start, end, item))
            intervals.sort(key=lambda x: x[0])
            for a, b in zip(intervals, intervals[1:]):
                if a[1] > b[0]:
                    return False, f"overlap: {a[2]} and {b[2]}"
            return True, f"no overlap among {len(intervals)} intervals"
        if kind == "json_dag":
            ok, data, err = _load_json(artifact)
            if not ok:
                return False, err
            found, actual = _lookup(data, str(check.get("path", "")))
            if not found or not isinstance(actual, list):
                return False, f"not a list: {check.get('path')}"
            ids = {str(t.get("id")) for t in actual if isinstance(t, dict)}
            required = [str(x) for x in check.get("required") or []]
            missing = [x for x in required if x not in ids]
            if missing:
                return False, "missing task ids: " + ", ".join(missing)
            deps: dict[str, set[str]] = {}
            for item in actual:
                if not isinstance(item, dict) or "id" not in item:
                    return False, "task item missing id"
                tid = str(item["id"])
                ds = item.get("deps") or []
                if not isinstance(ds, list):
                    return False, f"{tid} deps is not a list"
                for dep in ds:
                    if str(dep) not in ids:
                        return False, f"{tid} references missing dep {dep}"
                deps[tid] = {str(x) for x in ds}
            state: dict[str, int] = {tid: 0 for tid in deps}

            def visit(tid: str) -> bool:
                if state[tid] == 1:
                    return False
                if state[tid] == 2:
                    return True
                state[tid] = 1
                for dep in deps[tid]:
                    if not visit(dep):
                        return False
                state[tid] = 2
                return True

            for tid in deps:
                if not visit(tid):
                    return False, f"dependency cycle involving {tid}"
            return True, f"DAG ok ({len(deps)} tasks)"

        if kind in ("csv_columns", "csv_row_count", "csv_no_nulls", "csv_lookup"):
            text = _read(artifact)
            rows = list(csv.DictReader(io.StringIO(text)))
            if kind == "csv_columns":
                got = list(rows[0].keys()) if rows else []
                return got == values, f"columns={got} need={values}"
            if kind == "csv_row_count":
                n = len(rows)
                if "equals" in check:
                    want = int(check["equals"])
                    return n == want, f"rows={n} need={want}"
                lo = int(check.get("min", 0))
                hi = int(check["max"]) if "max" in check else 10**9
                return lo <= n <= hi, f"rows={n} need[{lo},{hi}]"
            if kind == "csv_no_nulls":
                cols = values or list(rows[0].keys() if rows else [])
                bad = [(i, c) for i, row in enumerate(rows) for c in cols if row.get(c) is None or str(row.get(c)).strip() == ""]
                return (not bad), ("no nulls" if not bad else f"nulls at rows/cols: {bad}")
            if kind == "csv_lookup":
                key, key_value = str(check.get("key")), str(check.get("key_value"))
                col, expected = str(check.get("column")), check.get("equals")
                for row in rows:
                    if str(row.get(key, "")) == key_value:
                        return _equal(str(row.get(col, "")), str(expected)), f"{key}={key_value} -> {col}={row.get(col)!r}"
                return False, f"row not found: {key}={key_value}"

        if kind in ("xlsx_sheet", "xlsx_cell", "xlsx_formula"):
            try:
                from openpyxl import load_workbook
            except ImportError:
                return False, "openpyxl unavailable"
            wb = load_workbook(artifact, data_only=False)
            if kind == "xlsx_sheet":
                missing = [v for v in values if v not in wb.sheetnames]
                return (not missing), ("sheets ok" if not missing else "missing sheets: " + ", ".join(missing))
            sheet = wb[str(check.get("sheet"))] if check.get("sheet") in wb.sheetnames else None
            if sheet is None:
                return False, f"missing sheet: {check.get('sheet')}"
            cell = str(check.get("cell"))
            actual = sheet[cell].value
            expected = check.get("equals")
            if kind == "xlsx_cell":
                return _equal(actual, expected), f"{cell}={actual!r} need {expected!r}"
            return str(actual) == str(expected), f"{cell} formula={actual!r} need {expected!r}"

        if kind in ("files_exist", "files_absent"):
            base = artifact.parent if artifact else None
            # Artifact "" means task root; the caller passes root as artifact.
            base = artifact
            if kind == "files_exist":
                missing = [v for v in values if not _safe_join(base, v).exists()]
                return (not missing), ("files ok" if not missing else "missing files: " + ", ".join(missing))
            present = [v for v in values if _safe_join(base, v).exists()]
            return (not present), ("files absent" if not present else "unexpected files: " + ", ".join(present))

        return False, f"unknown check type: {kind}"
    except Exception as exc:
        return False, f"{kind} raised {type(exc).__name__}: {exc}"


def run_checks(task: dict[str, Any], root: Path) -> list[dict[str, Any]]:
    result = []
    for check in task.get("checks") or []:
        rel = str(check.get("artifact") or "")
        artifact = root if not rel else _safe_join(root, rel)
        ok, detail = eval_check(artifact, check)
        result.append({
            "artifact": rel,
            "type": check.get("type"),
            "ok": ok,
            "detail": detail,
        })
    return result


# ---------------------------------------------------------------------------
# Suite loading / validation
# ---------------------------------------------------------------------------

def load_suite(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception as exc:
        raise SystemExit(f"cannot read tasks: {path}: {exc}")
    if not isinstance(data, dict) or not isinstance(data.get("tasks"), list):
        raise SystemExit(f"bad suite schema: {path}")
    return data


def validate_suite(data: dict[str, Any]) -> list[str]:
    errs: list[str] = []
    tasks = data.get("tasks") or []
    if len(tasks) < 20:
        errs.append(f"need >=20 tasks, got {len(tasks)}")
    if len({t.get("id") for t in tasks}) != len(tasks):
        errs.append("task ids are not unique")
    cats = {t.get("category") for t in tasks}
    if len(cats) < 10:
        errs.append(f"need >=10 categories, got {len(cats)}")
    for i, t in enumerate(tasks, 1):
        tid = t.get("id")
        for field in ("id", "title", "category", "source", "prompt", "inputs", "artifacts", "negative", "checks"):
            if field not in t:
                errs.append(f"task {i}: missing {field}")
        if not isinstance(t.get("artifacts"), dict) or not t["artifacts"]:
            errs.append(f"task {tid}: artifacts must be non-empty")
        if not isinstance(t.get("negative"), dict) or not t["negative"]:
            errs.append(f"task {tid}: negative must be non-empty")
        if not isinstance(t.get("checks"), list) or len(t["checks"]) < 2:
            errs.append(f"task {tid}: need >=2 checks")
        for check in t.get("checks") or []:
            ctype = check.get("type")
            if ctype not in CHECK_TYPES:
                errs.append(f"task {tid}: unknown check type {ctype!r}")
            rel = str(check.get("artifact") or "")
            if rel and rel not in t["artifacts"]:
                errs.append(f"task {tid}: check artifact {rel!r} not in artifacts")
            if not rel and ctype not in ("files_exist", "files_absent"):
                errs.append(f"task {tid}: empty artifact only valid for files_*")
        # reference and negative must cover the same artifact paths that checks touch
        for check in t.get("checks") or []:
            rel = str(check.get("artifact") or "")
            if rel and rel not in t["negative"]:
                errs.append(f"task {tid}: negative lacks artifact {rel!r}")
    return errs


def _artifact_format(task: dict[str, Any]) -> str:
    for spec in (task.get("artifacts") or {}).values():
        if isinstance(spec, dict) and spec.get("format") == "xlsx":
            return "xlsx"
        suffix = Path(str(next(iter(task.get("artifacts") or {})))).suffix.lower()
        return suffix.lstrip(".") or "none"
    return "none"


def _remove_paths(task: dict[str, Any], root: Path) -> None:
    for rel in task.get("remove_inputs") or []:
        target = _safe_join(root, rel)
        if target.is_file():
            target.unlink(missing_ok=True)
        elif target.is_dir():
            shutil.rmtree(target, ignore_errors=True)


def _materialize_reference(task: dict[str, Any], root: Path) -> None:
    materialize_inputs(task, root)
    for rel, spec in (task.get("artifacts") or {}).items():
        materialize(root, rel, spec)
    _remove_paths(task, root)


def _materialize_negative(task: dict[str, Any], root: Path) -> None:
    materialize_inputs(task, root)
    for rel, spec in (task.get("negative") or {}).items():
        materialize(root, rel, spec)


def _copy_candidate(task: dict[str, Any], candidate_root: Path, root: Path) -> None:
    materialize_inputs(task, root)
    task_root = candidate_root / str(task["id"])
    for rel in (task.get("artifacts") or {}):
        src = _safe_join(task_root, rel)
        if src.exists():
            dst = _safe_join(root, rel)
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
    _remove_paths(task, root)


# ---------------------------------------------------------------------------
# Running
# ---------------------------------------------------------------------------

def run_one(task: dict[str, Any], root: Path, mode: str) -> dict[str, Any]:
    if mode == "reference":
        _materialize_reference(task, root)
    elif mode == "negative":
        _materialize_negative(task, root)
    checks = run_checks(task, root)
    passed = sum(1 for c in checks if c["ok"])
    return {
        "id": task.get("id"),
        "title": task.get("title"),
        "category": task.get("category"),
        "source": task.get("source"),
        "mode": mode,
        "checks": len(checks),
        "passed": passed,
        "failed": len(checks) - passed,
        "ok": passed == len(checks),
        "details": checks,
    }


def _new_root(prefix: str, keep: bool) -> Path:
    return Path(tempfile.mkdtemp(prefix=f"work-bench-{prefix}-"))


def run_suite(tasks_path: Path, mode: str, candidates: str | None, keep: bool, quiet: bool,
              data: dict[str, Any] | None = None) -> dict[str, Any]:
    if data is None:
        data = load_suite(tasks_path)
        errs = validate_suite(data)
        if errs:
            raise SystemExit("suite validation failed:\n- " + "\n- ".join(errs))
    tasks = data["tasks"]
    rows = []
    wall0 = time.perf_counter()
    root = _new_root(mode, keep) if not candidates else _new_root("candidate", keep)
    try:
        for task in tasks:
            subset = root / str(task["id"])
            subset.mkdir(parents=True, exist_ok=True)
            if mode == "candidate":
                _copy_candidate(task, Path(candidates), subset)
                checks = run_checks(task, subset)
                row = {
                    "id": task.get("id"), "title": task.get("title"),
                    "category": task.get("category"), "source": task.get("source"),
                    "mode": "candidate", "checks": len(checks),
                    "passed": sum(1 for c in checks if c["ok"]),
                    "failed": sum(1 for c in checks if not c["ok"]),
                    "ok": all(c["ok"] for c in checks), "details": checks,
                }
            else:
                row = run_one(task, subset, mode)
            rows.append(row)
            if not quiet:
                print(f"{row['id']:<22} {row['category']:<11} {row['passed']}/{row['checks']} {'PASS' if row['ok'] else 'FAIL'}")
    finally:
        if not keep:
            shutil.rmtree(root, ignore_errors=True)
    wall = round(time.perf_counter() - wall0, 3)
    checks = sum(r["checks"] for r in rows)
    passed = sum(r["passed"] for r in rows)
    metrics = {
        "tasks": len(rows),
        "checks": checks,
        "passed": passed,
        "failed": checks - passed,
        "pass_rate": round(passed / checks, 4) if checks else 0.0,
        "categories": len({r["category"] for r in rows}),
        "artifacts": sum(len((t.get("artifacts") or {})) for t in tasks),
        "wall_s": wall,
        "by_category": {
            cat: {
                "tasks": sum(1 for r in rows if r["category"] == cat),
                "checks": sum(r["checks"] for r in rows if r["category"] == cat),
                "passed": sum(r["passed"] for r in rows if r["category"] == cat),
            }
            for cat in sorted({r["category"] for r in rows})
        },
        "by_check_type": {},
    }
    for r in rows:
        for c in r["details"]:
            key = str(c["type"])
            m = metrics["by_check_type"].setdefault(key, {"checks": 0, "passed": 0})
            m["checks"] += 1
            m["passed"] += 1 if c["ok"] else 0
    return {
        "schema": 1,
        "suite": SUITE,
        "tasks_path": str(tasks_path),
        "mode": mode,
        "wall_s": wall,
        "metrics": metrics,
        "results": rows,
    }


def run_negative_matrix(data: dict[str, Any], keep: bool, quiet: bool) -> dict[str, Any]:
    rows = []
    root = _new_root("negative", keep)
    try:
        for task in data["tasks"]:
            subset = root / str(task["id"])
            subset.mkdir(parents=True, exist_ok=True)
            row = run_one(task, subset, "negative")
            row["caught"] = not row["ok"]
            rows.append(row)
            if not quiet:
                print(f"neg {row['id']:<18} caught={row['caught']} failed_checks={row['failed']}/{row['checks']}")
    finally:
        if not keep:
            shutil.rmtree(root, ignore_errors=True)
    return {
        "tasks": len(rows),
        "negative_caught": sum(1 for r in rows if r["caught"]),
        "negative_total": len(rows),
        "negative_detection_rate": round(sum(1 for r in rows if r["caught"]) / len(rows), 4) if rows else 0.0,
        "results": rows,
    }


def self_test(tasks_path: Path = DEFAULT_TASKS) -> int:
    data = load_suite(tasks_path)
    errs = validate_suite(data)
    structure = 1 + len(data["tasks"]) * 4
    passed = 0
    failed = 0
    total = structure
    details = []
    if errs:
        failed += 1
        details.append(f"suite validation: {errs}")
    else:
        passed += 1
    for task in data["tasks"]:
        # reference must pass every check
        root = Path(tempfile.mkdtemp(prefix="work-bench-selftest-ref-"))
        try:
            _materialize_reference(task, root)
            checks = run_checks(task, root)
            total += len(checks)
            passed += sum(1 for c in checks if c["ok"])
            failed += sum(1 for c in checks if not c["ok"])
            if not all(c["ok"] for c in checks):
                details.append(f"{task['id']}: reference failed: " + "; ".join(c["detail"] for c in checks if not c["ok"]))
        except Exception as exc:
            failed += 1
            total += len(task.get("checks") or [])
            details.append(f"{task['id']}: reference raised {type(exc).__name__}: {exc}")
        finally:
            shutil.rmtree(root, ignore_errors=True)
        # negative must be caught
        root = Path(tempfile.mkdtemp(prefix="work-bench-selftest-neg-"))
        total += 1
        try:
            _materialize_negative(task, root)
            checks = run_checks(task, root)
            if not all(c["ok"] for c in checks):
                passed += 1
            else:
                failed += 1
                details.append(f"{task['id']}: negative was not caught")
        except Exception as exc:
            failed += 1
            details.append(f"{task['id']}: negative raised {type(exc).__name__}: {exc}")
        finally:
            shutil.rmtree(root, ignore_errors=True)
        # structural invariants per task
        for label, cond in (
            ("artifacts", bool(task.get("artifacts"))),
            ("negative", bool(task.get("negative"))),
            ("checks>=2", len(task.get("checks") or []) >= 2),
            ("negative covers checks", all(str(c.get("artifact") or "") in (task.get("negative") or {}) for c in task.get("checks") or [] if str(c.get("artifact") or ""))),
        ):
            if cond:
                passed += 1
            else:
                failed += 1
                details.append(f"{task.get('id')}: structure {label}")
    print(f"work-bench self-test: {passed}/{total}")
    if details:
        for item in details[:20]:
            print("  -", item)
        if len(details) > 20:
            print(f"  ... {len(details)-20} more")
    return 0 if failed == 0 else 1


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Deterministic work-task benchmark")
    ap.add_argument("-Tasks", default=str(DEFAULT_TASKS), help="tasks.json")
    ap.add_argument("-SelfTest", action="store_true")
    ap.add_argument("-Run", action="store_true")
    ap.add_argument("-Candidates", default=None, help="candidate artifact root")
    ap.add_argument("-Out", default=None)
    ap.add_argument("-Task", default=None, help="task id filter")
    ap.add_argument("-Category", default=None, help="category filter")
    ap.add_argument("-Repeat", type=int, default=1)
    ap.add_argument("-Strict", action="store_true")
    ap.add_argument("-Quiet", action="store_true")
    ap.add_argument("-Keep", action="store_true")
    ap.add_argument("-List", action="store_true")
    args = ap.parse_args(argv)

    tasks_path = Path(args.Tasks)
    if args.SelfTest:
        return self_test(tasks_path)
    if not tasks_path.exists():
        print(f"tasks not found: {tasks_path}", file=sys.stderr)
        return 2
    if args.List:
        data = load_suite(tasks_path)
        for t in data["tasks"]:
            print(f"{t['id']:<22} {t['category']:<12} {t['source']}")
        return 0
    if not args.Run:
        ap.print_help()
        return 2

    # filter copies so the suite file remains untouched
    data = load_suite(tasks_path)
    errs = validate_suite(data)
    if errs:
        raise SystemExit("suite validation failed:\n- " + "\n- ".join(errs))
    if args.Task:
        data["tasks"] = [t for t in data["tasks"] if t["id"] == args.Task]
    if args.Category:
        data["tasks"] = [t for t in data["tasks"] if t["category"] == args.Category]
    if not data["tasks"]:
        print("no tasks selected", file=sys.stderr)
        return 2

    mode = "candidate" if args.Candidates else "reference"
    runs = []
    wall0 = time.perf_counter()
    for i in range(max(1, args.Repeat)):
        if i:
            print(f"repeat {i+1}/{args.Repeat}")
        runs.append(run_suite(tasks_path, mode, args.Candidates, args.Keep, args.Quiet, data))
    # Negative matrix is always run from the built-in negative artifacts.
    neg = run_negative_matrix(data, args.Keep, args.Quiet)
    wall = round(time.perf_counter() - wall0, 3)

    latest = runs[-1]
    metrics = latest["metrics"]
    metrics.update({
        "negative_caught": neg["negative_caught"],
        "negative_total": neg["negative_total"],
        "negative_detection_rate": neg["negative_detection_rate"],
        "repeat": args.Repeat,
        "runs": len(runs),
    })
    report = {
        "schema": 1,
        "suite": SUITE,
        "tasks_path": str(tasks_path),
        "mode": mode,
        "repeat": args.Repeat,
        "wall_s": wall,
        "metrics": metrics,
        "reference": {"passed": latest["metrics"]["passed"], "total": latest["metrics"]["checks"]},
        "negative": {"caught": neg["negative_caught"], "total": neg["negative_total"]},
        "runs": [
            {
                "wall_s": r["wall_s"],
                "tasks": r["metrics"]["tasks"],
                "checks": r["metrics"]["checks"],
                "passed": r["metrics"]["passed"],
                "failed": r["metrics"]["failed"],
                "ok": r["metrics"]["failed"] == 0,
            }
            for r in runs
        ],
        "results": latest["results"],
        "negative_results": [{"id": r["id"], "caught": r["caught"], "failed_checks": r["failed"]} for r in neg["results"]],
    }
    if args.Repeat > 1:
        report["repeat_all_ok"] = all(r["metrics"]["failed"] == 0 for r in runs)
    if args.Out:
        out = Path(args.Out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        if not args.Quiet:
            print(f"report: {out}")
    if not args.Quiet:
        print(f"work-bench: {metrics['tasks']} tasks / {metrics['checks']} checks / "
              f"reference {metrics['passed']}/{metrics['checks']} / "
              f"negative {neg['negative_caught']}/{neg['negative_total']} / wall {wall}s")
    bad = metrics["failed"] > 0 or neg["negative_caught"] < neg["negative_total"]
    return 0 if (not args.Strict or not bad) else 1


if __name__ == "__main__":
    raise SystemExit(main())
