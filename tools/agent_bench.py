#!/usr/bin/env python3
"""Agent benchmark: run real code-task fixtures through agent_verify.

This is a deterministic, local slice of the public agent-benchmark landscape:
each task is a small repository fixture with a *gold* patch, a *negative*
(behaviorally wrong) patch, and a real command that must pass/fail.  The tool
does not generate code and does not call an LLM; it executes a candidate patch
(default: the gold patch) through the same `agent_verify` chain used by the
agents, then reports whether the guard/run/journal/receipt loop agrees with the
fixture's ground truth.

Usage:
  python tools/agent_bench.py -List
  python tools/agent_bench.py -SelfTest
  python tools/agent_bench.py -Run -Out bench.json
  python tools/agent_bench.py -Run -Candidates E:\\tmp\\solutions
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(TOOLS)
DEFAULT_TASKS = os.path.join(REPO, "testdata", "agent-bench", "tasks.json")
AV = os.path.join(TOOLS, "agent_verify.py")
sys.path.insert(0, TOOLS)
import agent_metrics  # noqa: E402  (same directory, no package marker)


class BenchError(RuntimeError):
    pass


def _now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _norm_rel(rel):
    if not isinstance(rel, str) or not rel.strip():
        raise BenchError(f"empty/relative path: {rel!r}")
    if os.path.isabs(rel):
        raise BenchError(f"absolute path not allowed: {rel}")
    p = os.path.normpath(rel).replace("\\", "/")
    if p == ".." or p.startswith("../") or p.startswith("/"):
        raise BenchError(f"path escapes fixture: {rel}")
    return p


def _write(root, rel, content):
    rel = _norm_rel(rel)
    path = os.path.join(root, rel.replace("/", os.sep))
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(content)
    return path


def _read(root, rel):
    rel = _norm_rel(rel)
    with open(os.path.join(root, rel.replace("/", os.sep)), "r", encoding="utf-8") as f:
        return f.read()


def _load_json(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except OSError as e:
        raise BenchError(f"cannot read {path}: {e}") from e
    except json.JSONDecodeError as e:
        raise BenchError(f"bad JSON in {path}: {e}") from e


def _validate_task(t):
    tid = t.get("id")
    if not isinstance(tid, str) or not tid.strip():
        raise BenchError("task missing id")
    if "/" in tid or "\\" in tid or tid.startswith("."):
        raise BenchError(f"task id is not a safe name: {tid}")
    kind = t.get("kind", "patch")
    if kind not in ("patch", "compact", "policy", "budget", "dry_run"):
        raise BenchError(f"{tid}: unknown kind {kind!r}")
    if not isinstance(t.get("commands"), list) or not t["commands"]:
        raise BenchError(f"{tid}: commands must be a non-empty list")
    for c in t["commands"]:
        if not isinstance(c, str) or not c.strip():
            raise BenchError(f"{tid}: empty command")
    if not isinstance(t.get("extra", []), list):
        raise BenchError(f"{tid}: extra must be a list")
    files = t.get("files") or {}
    if not isinstance(files, dict):
        raise BenchError(f"{tid}: files must be an object")
    for rel in files:
        _norm_rel(rel)
    if kind in ("patch", "compact"):
        changed = t.get("changed") or []
        if not isinstance(changed, list) or not changed:
            raise BenchError(f"{tid}: patch task needs changed paths")
        gold = t.get("gold") or {}
        negative = t.get("negative") or {}
        if not isinstance(gold, dict) or not isinstance(negative, dict):
            raise BenchError(f"{tid}: gold/negative must be objects")
        if not gold or not negative:
            raise BenchError(f"{tid}: patch task needs gold and negative")
        neg_changed = t.get("negative_changed") or changed
        if not isinstance(neg_changed, list) or not neg_changed:
            raise BenchError(f"{tid}: bad negative_changed")
        for rel in list(changed) + list(neg_changed):
            _norm_rel(rel)
            if rel not in files:
                raise BenchError(f"{tid}: changed path {rel} not in fixture files")
        for rel in list(gold) + list(negative):
            _norm_rel(rel)
            if rel not in files:
                raise BenchError(f"{tid}: patch path {rel} not in fixture files")
        exp = t.get("expect") or {}
        if exp.get("gold") not in ("passed", "failed") or exp.get("negative") not in ("passed", "failed"):
            raise BenchError(f"{tid}: bad expect for patch")
    else:
        exp = t.get("expect") or {}
        if exp.get("run") not in ("passed", "failed", "refused", "suspended", "dry_run", "rejected"):
            raise BenchError(f"{tid}: bad expect.run")


def load_suite(path=DEFAULT_TASKS):
    doc = _load_json(path)
    if not isinstance(doc, dict) or not isinstance(doc.get("tasks"), list):
        raise BenchError("suite must be {schema, tasks:[...]}")
    if doc.get("schema") != 1:
        raise BenchError(f"unsupported suite schema: {doc.get('schema')!r}")
    ids = set()
    for t in doc["tasks"]:
        if not isinstance(t, dict):
            raise BenchError("task is not an object")
        _validate_task(t)
        tid = t["id"]
        if tid in ids:
            raise BenchError(f"duplicate task id: {tid}")
        ids.add(tid)
    if not doc["tasks"]:
        raise BenchError("suite has no tasks")
    return doc


def _all_tasks(doc):
    return doc["tasks"]


def select_tasks(doc, task_id=None, category=None):
    tasks = _all_tasks(doc)
    if task_id:
        tasks = [t for t in tasks if t["id"] == task_id]
        if not tasks:
            raise BenchError(f"no task with id {task_id!r}")
    if category:
        tasks = [t for t in tasks if t.get("category") == category]
        if not tasks:
            raise BenchError(f"no task in category {category!r}")
    return tasks


def _replace_py(command):
    return command.replace("{py}", f'"{sys.executable}"')


def _plan(t, root):
    plan = {
        "proven": True,
        "commands": [_replace_py(c) for c in t["commands"]],
        "unproven": [],
    }
    path = os.path.join(root, ".bench", "plan.json")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(plan, f, ensure_ascii=False, indent=2)
        f.write("\n")
    return path


def _materialize(t, root):
    for rel, content in (t.get("files") or {}).items():
        _write(root, rel, content)


def _apply_files(root, files):
    for rel, content in files.items():
        _write(root, rel, content)


def _copy_candidate(root, candidate_dir):
    if not os.path.isdir(candidate_dir):
        raise BenchError(f"candidate directory missing: {candidate_dir}")
    for dirpath, dirnames, filenames in os.walk(candidate_dir):
        dirnames[:] = sorted(d for d in dirnames if d not in (".git", ".bench"))
        for name in sorted(filenames):
            src = os.path.join(dirpath, name)
            if os.path.islink(src):
                raise BenchError(f"candidate symlink not allowed: {src}")
            rel = os.path.relpath(src, candidate_dir).replace(os.sep, "/")
            _norm_rel(rel)
            with open(src, "r", encoding="utf-8") as f:
                _write(root, rel, f.read())


def _receipt_of(path):
    try:
        rec = agent_metrics.load_receipt(path)
    except Exception as e:  # noqa: BLE001 - surface as data, not a crash
        return {"error": f"{type(e).__name__}: {e}", "raw": {}}
    return rec


def _run_one(t, variant, parent, candidate_root=None, timeout=120):
    """Materialize, apply patch, run agent_verify. Return a result dict."""
    root = tempfile.mkdtemp(prefix=f"{t['id']}-{variant}-", dir=parent)
    result = {
        "task": t["id"],
        "variant": variant,
        "root": root,
        "ok": False,
        "rc": None,
        "vclass": None,
        "mode": None,
        "expected": None,
        "duration_s": 0.0,
        "error": None,
        "stdout": "",
        "stderr": "",
    }
    started = time.time()
    try:
        _materialize(t, root)
        if variant == "negative":
            _apply_files(root, t.get("negative") or {})
        elif variant == "candidate":
            cdir = os.path.join(candidate_root, t["id"]) if candidate_root else ""
            _copy_candidate(root, cdir)
        elif variant in ("gold", "reference"):
            _apply_files(root, t.get("gold") or {})
        # For special tasks no patch is applied; for a compact task the extra
        # flag is carried by t["extra"].
        plan = _plan(t, root)
        changed = t.get("negative_changed") if variant == "negative" else t.get("changed")
        changed = changed or t.get("changed") or []
        receipt = os.path.join(root, ".bench", f"receipt-{variant}.json")
        store = os.path.join(root, ".bench", "store")
        args = [
            sys.executable, AV,
            "-Root", root,
            "-Changed", ",".join(changed),
            "-Lang", t.get("lang", "python"),
            "-Plan", plan,
            "-Store", store,
            "-Receipt", receipt,
            "-Agent-ID", f"bench:{t['id']}",
            "--no-claim-gate",
        ]
        if t.get("profile"):
            args += ["-Profile", t["profile"]]
        args += list(t.get("extra") or [])
        proc = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        result["rc"] = proc.returncode
        result["stdout"] = proc.stdout[-2500:]
        result["stderr"] = proc.stderr[-2500:]
        if os.path.exists(receipt):
            rec = _receipt_of(receipt)
            result["receipt"] = receipt
            result["vclass"] = rec.get("vclass")
            result["mode"] = rec.get("mode")
            result["record"] = rec
            result["expected"] = _expected_class(t, variant)
        else:
            result["error"] = "receipt missing"
    except subprocess.TimeoutExpired:
        result["error"] = f"timeout after {timeout}s"
    except Exception as e:  # noqa: BLE001 - one task's setup failure must not abort the suite
        result["error"] = f"{type(e).__name__}: {e}"
    finally:
        result["duration_s"] = round(time.time() - started, 3)
    return result


def _expected_class(t, variant):
    if variant == "negative":
        return t.get("expect", {}).get("negative")
    if t.get("kind") in ("patch", "compact"):
        return t.get("expect", {}).get("gold")
    return t.get("expect", {}).get("run")


def _expected_rc(expected):
    return {"passed": 0, "dry_run": 0}.get(expected, 1)


def _raw_steps(rec):
    steps = (rec or {}).get("steps") or []
    return steps if isinstance(steps, list) else []


def _raw_runs(rec):
    runs = (rec or {}).get("run") or []
    return runs if isinstance(runs, list) else []


def _check_result(t, variant, result):
    """Return [(name, ok, detail)] for one run."""
    checks = []
    expected = result.get("expected")
    rec = result.get("record") or {}
    checks.append(("receipt", bool(rec) and not rec.get("error"), rec.get("error")))
    if expected:
        checks.append(("exit code", result.get("rc") == _expected_rc(expected),
                       f"rc={result.get('rc')} want={_expected_rc(expected)}"))
        checks.append(("verdict class", result.get("vclass") == expected,
                       f"class={result.get('vclass')} want={expected}"))
    if t.get("kind") == "compact":
        checks.append(("compact mode", result.get("mode") == "compact", f"mode={result.get('mode')}"))
    if t.get("kind") in ("patch", "compact"):
        runs = _raw_runs(rec)
        checks.append(("run present", bool(runs), f"run entries={len(runs)}"))
        if runs:
            nonzero = sum(1 for r in runs if int(r.get("exit", 0) or 0) != 0)
            if variant == "negative":
                checks.append(("negative fails command", nonzero >= 1, f"nonzero={nonzero}"))
            else:
                checks.append(("gold passes command", nonzero == 0, f"nonzero={nonzero}"))
        changed = (t.get("negative_changed") if variant == "negative" else t.get("changed")) or []
        if changed:
            try:
                content = _read(result["root"], changed[0])
                want = ((t.get("negative") if variant == "negative" else t.get("gold")) or {}).get(changed[0])
                if want is not None:
                    checks.append(("patch landed", content == want, f"{changed[0]}"))
            except Exception as e:  # noqa: BLE001
                checks.append(("patch landed", False, str(e)))
    if t.get("kind") == "policy":
        policy = [s for s in _raw_steps(rec) if s.get("step") == "policy"]
        checks.append(("policy refused", bool(policy) and policy[-1].get("verdict") == "refused",
                       str(policy[-1].get("verdict") if policy else "missing")))
        checks.append(("policy no run", not _raw_runs(rec), f"runs={len(_raw_runs(rec))}"))
    if t.get("kind") == "budget":
        budget = rec.get("budget") or {}
        checks.append(("budget declared zero", budget.get("declared") == 0, str(budget.get("declared"))))
        checks.append(("budget no run", not _raw_runs(rec), f"runs={len(_raw_runs(rec))}"))
    if t.get("kind") == "dry_run":
        checks.append(("dry-run no run", not _raw_runs(rec), f"runs={len(_raw_runs(rec))}"))
    return checks


def _run_checks(t, variant, result):
    checks = _check_result(t, variant, result)
    result["checks"] = [{"name": n, "ok": bool(ok), "detail": str(detail)} for n, ok, detail in checks]
    result["ok"] = all(ok for _, ok, _ in checks)
    return result


def _evaluate(t, variant, parent, candidate_root=None, timeout=120):
    result = _run_one(t, variant, parent, candidate_root, timeout)
    result = _run_checks(t, variant, result)
    return result


def _metrics(records):
    try:
        return agent_metrics.build_metrics(records)
    except Exception as e:  # noqa: BLE001
        return {"error": f"{type(e).__name__}: {e}"}


def self_test(path=DEFAULT_TASKS, keep=False):
    suite = load_suite(path)
    tasks = _all_tasks(suite)
    checks = []
    failures = []

    def check(name, ok, detail=""):
        checks.append(bool(ok))
        if not ok:
            failures.append(f"{name}: {detail}")

    check("suite has 20+ tasks", len(tasks) >= 20, str(len(tasks)))
    check("suite has 8+ categories", len({t.get("category") for t in tasks}) >= 8,
          str(len({t.get("category") for t in tasks})))
    check("suite has 4+ languages", len({t.get("lang", "python") for t in tasks}) >= 4,
          str(len({t.get("lang", "python") for t in tasks})))
    check("suite has patch tasks", sum(1 for t in tasks if t.get("kind") in ("patch", "compact")) >= 10)
    check("suite has negative patches", all(bool(t.get("negative")) for t in tasks
                                            if t.get("kind") in ("patch", "compact")))
    check("suite has special tasks", any(t.get("kind") != "patch" for t in tasks))
    check("unique ids", len({t["id"] for t in tasks}) == len(tasks))

    parent = tempfile.mkdtemp(prefix="agent-bench-selftest-")
    records = []
    try:
        for t in tasks:
            if t.get("kind") in ("patch", "compact"):
                for variant in ("gold", "negative"):
                    result = _evaluate(t, variant, parent)
                    records.append((result.get("record") or {"error": result.get("error")}, result))
                    for n, ok, detail in _check_result(t, variant, result):
                        check(f"{t['id']} {variant} {n}", ok, detail)
            else:
                result = _evaluate(t, "reference", parent)
                records.append((result.get("record") or {"error": result.get("error")}, result))
                for n, ok, detail in _check_result(t, "reference", result):
                    check(f"{t['id']} {n}", ok, detail)
    finally:
        if keep:
            print(f"agent-bench: kept {parent}")
        else:
            shutil.rmtree(parent, ignore_errors=True)

    metrics = _metrics([r for r, _ in records])
    check("metrics present", not metrics.get("error"), str(metrics.get("error") or ""))
    if not metrics.get("error"):
        check("metrics receipt count", metrics.get("receipts", {}).get("files") == len(records),
              f"{metrics.get('receipts', {}).get('files')} vs {len(records)}")
        check("metrics malformed zero", metrics.get("receipts", {}).get("malformed") == 0,
              str(metrics.get("receipts", {}).get("malformed")))
        check("metrics strict ok", (metrics.get("integrity") or {}).get("strict_ok") is True,
              str((metrics.get("integrity") or {}).get("strict_ok")))

    negatives = [r for _, r in records if r.get("variant") == "negative"]
    golds = [r for _, r in records if r.get("variant") in ("gold", "reference")]
    check("negative detection", all(r.get("vclass") == "failed" for r in negatives),
          str([(r.get("task"), r.get("vclass")) for r in negatives if r.get("vclass") != "failed"]))
    check("gold pass", all(r.get("vclass") == "passed" for r in golds
                           if next(t for t in tasks if t["id"] == r["task"]).get("kind") in ("patch", "compact")),
          str([(r.get("task"), r.get("vclass")) for r in golds
               if next(t for t in tasks if t["id"] == r["task"]).get("kind") in ("patch", "compact")
               and r.get("vclass") != "passed"]))
    check("negative count", len(negatives) >= 10, str(len(negatives)))
    check("gold count", len(golds) >= 10, str(len(golds)))

    total = len(checks)
    passed = sum(1 for c in checks if c)
    print(f"agent-bench self-test: {passed}/{total}")
    for f in failures[:20]:
        print(f"  FAIL {f}")
    if failures:
        print(f"  ... {len(failures)} failures")
    return 0 if not failures else 1


def list_tasks(path=DEFAULT_TASKS):
    doc = load_suite(path)
    print(f"{'id':32} {'kind':9} {'category':12} {'lang':8} {'difficulty':10} source")
    for t in _all_tasks(doc):
        print(f"{t['id']:32} {t.get('kind', 'patch'):9} {t.get('category', ''):12} "
              f"{t.get('lang', 'python'):8} {t.get('difficulty', ''):10} {t.get('source', '')}")
    print(f"agent-bench: {len(_all_tasks(doc))} tasks")


def run_bench(path=DEFAULT_TASKS, task_id=None, category=None, out=None, candidates=None,
              repeat=1, timeout=120, strict=False, keep=False, quiet=False):
    doc = load_suite(path)
    tasks = select_tasks(doc, task_id, category)
    parent = tempfile.mkdtemp(prefix="agent-bench-run-")
    started = time.time()
    results = []
    try:
        for t in tasks:
            if t.get("kind") in ("patch", "compact"):
                variant = "candidate" if candidates else "reference"
                for n in range(repeat):
                    result = _evaluate(t, variant, parent, candidates, timeout)
                    result["repeat"] = n
                    results.append(result)
                for n in range(repeat):
                    result = _evaluate(t, "negative", parent, candidates, timeout)
                    result["repeat"] = n
                    results.append(result)
            else:
                for n in range(repeat):
                    result = _evaluate(t, "reference", parent, candidates, timeout)
                    result["repeat"] = n
                    results.append(result)
    finally:
        if keep:
            print(f"agent-bench: kept {parent}")
        else:
            shutil.rmtree(parent, ignore_errors=True)

    records = []
    for r in results:
        rec = r.get("record") or {"error": r.get("error"), "raw": {}}
        rec["bench"] = {"task": r["task"], "variant": r["variant"]}
        records.append(rec)
    metrics = _metrics(records)

    reference = [r for r in results if r.get("variant") in ("reference", "candidate") and
                 next(t for t in tasks if t["id"] == r["task"]).get("kind") in ("patch", "compact")]
    negatives = [r for r in results if r.get("variant") == "negative"]
    specials = [r for r in results if next(t for t in tasks if t["id"] == r["task"]).get("kind")
                not in ("patch", "compact")]

    def rate(rows, want="passed"):
        ok = sum(1 for r in rows if r.get("vclass") == want)
        return {"passed": ok, "total": len(rows), "rate": round(ok / len(rows), 4) if rows else None}

    summary = {
        "tasks": len(tasks),
        "runs": len(results),
        "repeat": repeat,
        "reference": rate(reference, "passed"),
        "negative": {
            "detected": sum(1 for r in negatives if r.get("vclass") == "failed"),
            "total": len(negatives),
            "false_accept": sum(1 for r in negatives if r.get("vclass") == "passed"),
        },
        "special": {"passed": sum(1 for r in specials if r.get("ok")), "total": len(specials)},
        "coverage": {
            "categories": sorted({t.get("category") for t in tasks}),
            "kinds": sorted({t.get("kind", "patch") for t in tasks}),
            "languages": sorted({t.get("lang", "python") for t in tasks}),
        },
    }
    if repeat > 1:
        by_task = {}
        for r in results:
            if r.get("variant") in ("reference", "candidate"):
                by_task.setdefault(r["task"], []).append(r.get("ok"))
        summary["repeat_all_ok"] = {
            "passed": sum(1 for v in by_task.values() if v and all(v)),
            "total": len(by_task),
        }

    report = {
        "schema": 1,
        "suite": "agent-bench",
        "generated_at": _now(),
        "tasks": len(tasks),
        "runs": len(results),
        "summary": summary,
        "metrics": metrics,
        "results": [{k: v for k, v in r.items() if k not in ("record", "stdout", "stderr")}
                    for r in results],
    }
    if out:
        parent_dir = os.path.dirname(os.path.abspath(out))
        os.makedirs(parent_dir, exist_ok=True)
        with open(out, "w", encoding="utf-8") as f:
            json.dump(report, f, ensure_ascii=False, indent=2)
            f.write("\n")
        if not quiet:
            print(f"agent-bench: wrote {out}")

    bad = [r for r in results if not r.get("ok")]
    if not quiet:
        print(f"agent-bench: tasks={len(tasks)} runs={len(results)} "
              f"reference={summary['reference']['passed']}/{summary['reference']['total']} "
              f"negative={summary['negative']['detected']}/{summary['negative']['total']} "
              f"special={summary['special']['passed']}/{summary['special']['total']} "
              f"wall={round(time.time() - started, 2)}s")
        if bad:
            print(f"agent-bench: {len(bad)} unexpected result(s)")
            for r in bad[:10]:
                print(f"  {r['task']} {r['variant']} rc={r.get('rc')} class={r.get('vclass')} "
                      f"{r.get('error') or ''}")
    if strict and bad:
        return 1
    return 0


def main(argv):
    p = argparse.ArgumentParser(prog="agent-bench", description="agent_verify real-task benchmark")
    p.add_argument("-Tasks", default=DEFAULT_TASKS, help="task suite JSON")
    p.add_argument("-List", action="store_true", help="list tasks")
    p.add_argument("-SelfTest", action="store_true", help="run all fixtures gold+negative")
    p.add_argument("-Run", action="store_true", help="run the benchmark")
    p.add_argument("-Task", default=None, help="single task id")
    p.add_argument("-Category", default=None, help="filter category")
    p.add_argument("-Candidates", default=None, help="candidate root: <task-id>/<files>")
    p.add_argument("-Repeat", type=int, default=1, help="repeat each variant (pass^k)")
    p.add_argument("-Out", default=None, help="write JSON report")
    p.add_argument("-Strict", action="store_true", help="exit 1 on unexpected result")
    p.add_argument("-Quiet", action="store_true")
    p.add_argument("-Keep", action="store_true", help="keep temporary roots")
    p.add_argument("-Timeout", type=int, default=120, help="per-run timeout seconds")
    a = p.parse_args(argv)
    try:
        if a.List:
            list_tasks(a.Tasks)
            return 0
        if a.SelfTest:
            return self_test(a.Tasks, a.Keep)
        if a.Repeat < 1:
            raise BenchError("-Repeat must be >= 1")
        return run_bench(a.Tasks, a.Task, a.Category, a.Out, a.Candidates,
                         a.Repeat, a.Timeout, a.Strict, a.Keep, a.Quiet)
    except BenchError as e:
        print(f"agent-bench: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
