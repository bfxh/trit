#!/usr/bin/env python3
"""agent-metrics: turn agent-verify receipts into auditable run metrics.

WHY THIS EXISTS
  agent_verify writes a receipt per run, but a directory of receipts is not a
  dashboard: counts, rejection stages, wall/token/cost distributions and per-step
  failure rates have to be recomputed by hand. This tool reads full and compact
  receipts (plus malformed/delta files, which are counted rather than guessed at)
  and emits one JSON metrics object.

WHAT IT IS
  - Input: one or more receipt files, a directory, or a glob.
  - Output: counts and rates by verdict/class/exit, per-step verdicts and seconds,
    per-command exits, changes, budget, cost, claims/handoff, receipt sizes,
    wall/step/run percentiles, and per-agent/lang/profile groups.
  - It never copies output tails, claim text, paths or tokens into the metrics.
  - It is an observation surface, not a gate: -Strict only makes malformed input
    fail the command, it does not turn a low success rate into a refusal.

HONEST LIMITS
  - A receipt is the tool layer's record. Token numbers are only what the caller
    declared; this tool does not meter model-side tokens.
  - Percentiles over a small or biased set are descriptive, not SLO evidence.
  - Full/compact receipt shape is read defensively; malformed files are counted,
    not silently dropped.
"""

import argparse
import glob
import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
AV = os.path.join(ROOT, "tools", "agent_verify.py")
SCHEMA = 1
VERDICT_CLASSES = ("passed", "failed", "refused", "suspended", "dry_run", "rejected", "other")
BAD_STEP_VERDICTS = {"failed", "refused", "rejected"}


def _round(x):
    return round(float(x), 4)


def _safe_div(num, den):
    return float(num) / float(den) if den else 0.0


def _percentile(values, q):
    if not values:
        return 0.0
    vals = sorted(float(x) for x in values)
    if len(vals) == 1:
        return vals[0]
    pos = (len(vals) - 1) * q
    lo = int(pos)
    hi = min(lo + 1, len(vals) - 1)
    return vals[lo] + (vals[hi] - vals[lo]) * (pos - lo)


def _stats(values):
    vals = [float(x) for x in values if isinstance(x, (int, float))]
    return {
        "count": len(vals),
        "min": _round(min(vals)) if vals else 0.0,
        "max": _round(max(vals)) if vals else 0.0,
        "mean": _round(_safe_div(sum(vals), len(vals))) if vals else 0.0,
        "p50": _round(_percentile(vals, 0.50)),
        "p95": _round(_percentile(vals, 0.95)),
        "p99": _round(_percentile(vals, 0.99)),
    }


def _field(doc, key, default=None):
    if key in doc:
        return doc[key]
    vol = doc.get("volatile")
    if isinstance(vol, dict) and key in vol:
        return vol[key]
    return default


def _class_of(verdict, exit_code):
    v = str(verdict or "")
    if v == "dry-run":
        return "dry_run"
    if v.startswith("passed"):
        return "passed"
    if v == "command failed":
        return "failed"
    if v.startswith("refused"):
        return "refused"
    if v.startswith("suspended"):
        return "suspended"
    if v.startswith("rejected"):
        return "rejected"
    if v.startswith("ok") or v.startswith("changed"):
        return "passed"
    return "other"


def _requires_run(verdict):
    """A refusal/suspension/dry-run may legitimately end before any command ran."""
    v = str(verdict or "")
    return not (v == "dry-run" or v.startswith("refused") or
                v.startswith("suspended") or v.startswith("rejected"))


def load_receipt(path):
    """One receipt -> a flat record. Never invent a verdict from exit code."""
    try:
        nbytes = os.path.getsize(path)
    except OSError:
        nbytes = 0
    rec = {"path": path, "bytes": nbytes, "error": "", "delta": False,
           "missing_verdict": False, "missing_steps": False, "missing_run": False}
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            doc = json.load(fh)
    except (OSError, ValueError, json.JSONDecodeError) as e:
        rec["error"] = f"bad JSON: {e}"
        return rec
    if not isinstance(doc, dict):
        rec["error"] = "receipt is not a JSON object"
        return rec
    if ("delta" in doc or "against" in doc) and "verdict" not in doc:
        rec["delta"] = True
        return rec
    if "steps" not in doc:
        rec["missing_steps"] = True
    if "run" not in doc:
        rec["missing_run"] = _requires_run(doc.get("verdict"))
    if "verdict" not in doc:
        rec["missing_verdict"] = True
    steps = doc.get("steps") or []
    if not isinstance(steps, list):
        rec["error"] = "steps is not a list"
        return rec
    run = doc.get("run") or []
    if not isinstance(run, list):
        rec["error"] = "run is not a list"
        return rec
    steps = [s for s in steps if isinstance(s, dict)]
    run = [r for r in run if isinstance(r, dict)]
    step_secs = _field(doc, "steps_seconds", None)
    if not isinstance(step_secs, list) or len(step_secs) != len(steps):
        step_secs = [s.get("seconds", 0) for s in steps]
    run_secs = _field(doc, "run_seconds", None)
    if not isinstance(run_secs, list) or len(run_secs) != len(run):
        run_secs = [r.get("seconds", 0) for r in run]
    changes = doc.get("changes", 0)
    if isinstance(changes, list):
        change_count = len(changes)
    elif isinstance(changes, int):
        change_count = changes
    else:
        change_count = 0
    claim = doc.get("claim")
    if not isinstance(claim, dict):
        claim = {}
    handoff = doc.get("handoff")
    if not isinstance(handoff, dict):
        handoff = {}
    cost = _field(doc, "cost", {})
    if not isinstance(cost, dict):
        cost = {}
    budget = _field(doc, "budget", {})
    if not isinstance(budget, dict):
        budget = {}
    plan = doc.get("plan")
    if not isinstance(plan, dict):
        plan = {}
    proven = plan.get("proven", doc.get("proven"))
    rec.update({
        "mode": "compact" if isinstance(doc.get("changes"), int) and all(
            "output" not in s for s in steps) else "full",
        "verdict": doc.get("verdict"),
        "exit": doc.get("exit"),
        "vclass": _class_of(doc.get("verdict"), doc.get("exit")),
        "wall_s": _field(doc, "wall_s", 0) or 0,
        "steps": [{"step": str(s.get("step") or "?"), "verdict": str(s.get("verdict") or ""),
                   "exit": s.get("exit"), "seconds": sec}
                  for s, sec in zip(steps, step_secs)],
        "run": [{"command": str(r.get("command") or ""), "exit": r.get("exit"),
                 "seconds": sec} for r, sec in zip(run, run_secs)],
        "changes": change_count,
        "proven": bool(proven) if proven is not None else None,
        "agent_id": str(doc.get("agent_id") or "(none)"),
        "lang": str(doc.get("lang") or "(none)"),
        "profile": str(doc.get("profile") or "(none)"),
        "cost": {"tokens": cost.get("tokens"), "recorded": cost.get("recorded"),
                 "overhead_s": cost.get("overhead_s", 0), "scope": cost.get("scope")},
        "budget": {"declared": budget.get("declared"), "gate": budget.get("gate"),
                   "remaining": budget.get("remaining")},
        "claim": {"verdict": claim.get("verdict"),
                  "paths": len(claim.get("paths", [])) if isinstance(claim.get("paths"), list)
                  else int(claim.get("paths") or 0)},
        "handoff": {"verdict": handoff.get("verdict")},
        "at": _field(doc, "at", ""),
    })
    return rec


def collect(paths):
    out = []
    for spec in paths:
        if os.path.isdir(spec):
            out.extend(glob.glob(os.path.join(spec, "**", "*.json"), recursive=True))
        elif glob.glob(spec):
            out.extend(glob.glob(spec))
        elif os.path.exists(spec):
            out.append(spec)
        else:
            out.append(spec)
    # Deterministic order and no duplicate reads after an overlapping dir/glob.
    seen, uniq = set(), []
    for p in sorted(out):
        ap = os.path.abspath(p)
        if ap not in seen:
            seen.add(ap)
            uniq.append(p)
    return uniq


def build_metrics(records):
    valid = [r for r in records if not r.get("error") and not r.get("delta") and
             not r.get("missing_verdict") and not r.get("missing_steps") and
             not r.get("missing_run")]
    malformed = [r for r in records if r.get("error") or r.get("missing_verdict") or
                 r.get("missing_steps") or r.get("missing_run")]
    delta = [r for r in records if r.get("delta")]
    runs = [r for r in valid if r.get("verdict") is not None]
    verdicts, classes, exits = {}, {}, {}
    for r in runs:
        v = str(r["verdict"])
        verdicts[v] = verdicts.get(v, 0) + 1
        c = r["vclass"]
        classes[c] = classes.get(c, 0) + 1
        e = str(r.get("exit", "-"))
        exits[e] = exits.get(e, 0) + 1
    step_stats, failures, first_failure = {}, {}, {}
    all_step_secs, all_run_secs, wall_secs = [], [], []
    total_steps = total_runs = 0
    for r in runs:
        wall_secs.append(r.get("wall_s", 0))
        total_steps += len(r["steps"])
        total_runs += len(r["run"])
        first = None
        for i, s in enumerate(r["steps"]):
            name = s["step"]
            g = step_stats.setdefault(name, {"count": 0, "exits": {}, "verdicts": {},
                                             "seconds": []})
            g["count"] += 1
            g["exits"][str(s.get("exit"))] = g["exits"].get(str(s.get("exit")), 0) + 1
            g["verdicts"][s["verdict"]] = g["verdicts"].get(s["verdict"], 0) + 1
            if isinstance(s.get("seconds"), (int, float)):
                g["seconds"].append(s["seconds"])
            all_step_secs.append(s.get("seconds", 0))
            if first is None and (s["verdict"] in BAD_STEP_VERDICTS or
                                  (s.get("exit") not in (0, None) and s["step"] != "journal-end")):
                first = name
                failures[name] = failures.get(name, 0) + 1
        if first:
            first_failure[first] = first_failure.get(first, 0) + 1
        for x in r["run"]:
            all_run_secs.append(x.get("seconds", 0))
    per_step = {}
    for name, g in sorted(step_stats.items()):
        per_step[name] = {
            "count": g["count"], "exits": dict(sorted(g["exits"].items())),
            "verdicts": dict(sorted(g["verdicts"].items())),
            "failures": sum(g["verdicts"].get(v, 0) for v in BAD_STEP_VERDICTS),
            "seconds": _stats(g["seconds"]),
        }
    top_slow = sorted(((name, g["seconds"]) for name, g in step_stats.items()),
                      key=lambda x: _percentile(x[1], 0.95), reverse=True)[:5]
    top_slow = [{"step": name, "p95_s": _round(_percentile(secs, 0.95)),
                 "count": len(secs)} for name, secs in top_slow]

    def group(field):
        buckets = {}
        for r in runs:
            key = str(r.get(field) or "(none)")
            buckets.setdefault(key, []).append(r)
        out = {}
        for key, rs in sorted(buckets.items()):
            n = len(rs)
            c = {x: sum(1 for r in rs if r["vclass"] == x) for x in VERDICT_CLASSES}
            out[key] = {
                "runs": n, "passed": c["passed"], "failed": c["failed"],
                "refused": c["refused"], "suspended": c["suspended"],
                "dry_run": c["dry_run"], "success_rate": _round(_safe_div(c["passed"], n)),
                "wall_mean_s": _round(_safe_div(sum(r.get("wall_s", 0) for r in rs), n)),
            }
        return out

    cost = [r for r in runs if isinstance(r["cost"].get("tokens"), (int, float))]
    tokens = [r["cost"]["tokens"] for r in runs if isinstance(r["cost"].get("tokens"), (int, float))]
    overhead = [r["cost"]["overhead_s"] for r in runs
                if isinstance(r["cost"].get("overhead_s"), (int, float))]
    budgeted = [r for r in runs if isinstance(r["budget"].get("declared"), (int, float))]
    remaining = [r["budget"]["remaining"] for r in runs
                 if isinstance(r["budget"].get("remaining"), (int, float))]
    claimed = [r for r in runs if r["claim"].get("verdict") is not None]
    handed = [r for r in runs if r["handoff"].get("verdict") is not None]
    full_bytes = [r["bytes"] for r in valid if r.get("mode") == "full"]
    compact_bytes = [r["bytes"] for r in valid if r.get("mode") == "compact"]
    changes = [r["changes"] for r in runs]
    n = len(runs)
    class_counts = {x: classes.get(x, 0) for x in VERDICT_CLASSES}
    return {
        "schema": SCHEMA,
        "receipts": {
            "files": len(records), "valid": len(valid), "malformed": len(malformed),
            "delta": len(delta), "full": len(full_bytes), "compact": len(compact_bytes),
            "bytes": _stats([r["bytes"] for r in records]),
            "full_bytes": _stats(full_bytes), "compact_bytes": _stats(compact_bytes),
            "compact_ratio": _round(_safe_div(sum(compact_bytes), sum(full_bytes))) if full_bytes else None,
        },
        "verdicts": {
            "counts": dict(sorted(verdicts.items())),
            "rates": {k: _round(_safe_div(v, n)) for k, v in sorted(verdicts.items())},
            "classes": class_counts,
            "class_rates": {k: _round(_safe_div(v, n)) for k, v in class_counts.items()},
        },
        "quality": {
            "runs": n, "passed": class_counts["passed"], "failed": class_counts["failed"],
            "refused": class_counts["refused"], "suspended": class_counts["suspended"],
            "dry_run": class_counts["dry_run"], "rejected": class_counts["rejected"],
            "success_rate": _round(_safe_div(class_counts["passed"], n)),
            "failure_rate": _round(_safe_div(class_counts["failed"], n)),
            "refusal_rate": _round(_safe_div(class_counts["refused"], n)),
            "suspension_rate": _round(_safe_div(class_counts["suspended"], n)),
        },
        "exit_codes": dict(sorted(exits.items())),
        "steps": {
            "total": total_steps, "per_step": per_step,
            "failures": failures, "first_failure": first_failure,
            "seconds": _stats(all_step_secs), "top_slow": top_slow,
        },
        "run": {
            "total": total_runs,
            "exits": dict(sorted({str(x.get("exit")): sum(
                1 for r in runs for rr in r["run"] if str(rr.get("exit")) == str(x.get("exit")))
                for x in [rr for r in runs for rr in r["run"]]}.items())),
            "nonzero": sum(1 for r in runs for rr in r["run"] if rr.get("exit") not in (0, None)),
            "seconds": _stats(all_run_secs),
        },
        "changes": {
            "runs_with_changes": sum(1 for x in changes if x > 0),
            "total": sum(changes), "stats": _stats(changes),
        },
        "budget": {
            "with_budget": len(budgeted),
            "declared_total": sum(r["budget"]["declared"] for r in budgeted),
            "declared_mean": _round(_safe_div(sum(r["budget"]["declared"] for r in budgeted),
                                               len(budgeted))),
            "remaining_total": sum(remaining),
            "remaining_mean": _round(_safe_div(sum(remaining), len(remaining))),
            "gates": dict(sorted({str(r["budget"].get("gate")): sum(
                1 for x in runs if str(x["budget"].get("gate")) == str(r["budget"].get("gate")))
                for r in budgeted}.items())),
        },
        "cost": {
            "with_tokens": len(cost), "tokens_total": sum(tokens),
            "tokens_mean": _round(_safe_div(sum(tokens), len(tokens))),
            "tokens_stats": _stats(tokens),
            "overhead_total_s": _round(sum(overhead)),
            "overhead_mean_s": _round(_safe_div(sum(overhead), len(overhead))),
            "not_recorded": sum(1 for r in runs if r["cost"].get("recorded") is False),
        },
        "claims": {
            "with_claim": len(claimed),
            "claim_verdicts": dict(sorted({str(r["claim"].get("verdict")): sum(
                1 for x in claimed if str(x["claim"].get("verdict")) == str(r["claim"].get("verdict")))
                for r in claimed}.items())),
            "claimed_paths_total": sum(r["claim"].get("paths", 0) for r in claimed),
            "with_handoff": len(handed),
            "handoff_verdicts": dict(sorted({str(r["handoff"].get("verdict")): sum(
                1 for x in handed if str(x["handoff"].get("verdict")) == str(r["handoff"].get("verdict")))
                for r in handed}.items())),
        },
        "timing": {
            "wall_s": _stats(wall_secs), "wall_total_s": _round(sum(wall_secs)),
            "steps_total_s": _round(sum(all_step_secs)),
            "run_total_s": _round(sum(all_run_secs)),
        },
        "groups": {
            "by_agent": group("agent_id"), "by_lang": group("lang"),
            "by_profile": group("profile"),
        },
        "integrity": {
            "malformed": len(malformed), "delta_skipped": len(delta),
            "missing_verdict": sum(1 for r in records if r.get("missing_verdict")),
            "missing_steps": sum(1 for r in records if r.get("missing_steps")),
            "missing_run": sum(1 for r in records if r.get("missing_run")),
            "strict_ok": not malformed and all(
                not r.get("missing_verdict") and not r.get("missing_steps") and
                not r.get("missing_run") for r in records if not r.get("delta")),
        },
    }


def write_json(path, doc):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False, indent=2, sort_keys=True)
        fh.write("\n")


def run(files, out, strict, quiet):
    records = [load_receipt(p) for p in files]
    m = build_metrics(records)
    if out:
        write_json(out, m)
    if not quiet:
        q = m["quality"]
        print(f"agent-metrics: files={m['receipts']['files']} runs={q['runs']} "
              f"passed={q['passed']} failed={q['failed']} refused={q['refused']} "
              f"suspended={q['suspended']} dry_run={q['dry_run']} "
              f"wall_p95_s={m['timing']['wall_s']['p95']}")
        if m["receipts"]["malformed"]:
            print(f"agent-metrics: malformed={m['receipts']['malformed']} "
                  f"delta_skipped={m['receipts']['delta']} (counted, not dropped silently)")
        if out:
            print(f"agent-metrics: wrote {out}")
    if strict and not m["integrity"]["strict_ok"]:
        return 1
    return 0


def _call(args, cwd=None):
    return subprocess.run([sys.executable] + args, capture_output=True, text=True,
                          encoding="utf-8", errors="replace", cwd=cwd)


def _write(path, doc):
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False)


def _receipt(i, full=True):
    """Deterministic receipt family: 10 each of six verdict classes."""
    classes = [
        ("passed on proven subset", 0),
        ("command failed", 1),
        ("refused by policy", 1),
        ("suspended (budget) before run", 1),
        ("dry-run", 0),
        ("rejected: fabricated claims", 1),
    ]
    verdict, exit_code = classes[i % 6]
    n = i % 3
    steps = [
        {"step": "memory-brief", "verdict": "skipped", "exit": 0, "output": ["skip"]},
        {"step": "preflight", "verdict": "ok", "exit": 0, "output": ["ok"]},
        {"step": "policy", "verdict": ("refused" if i % 6 == 2 else "ok"),
         "exit": (1 if i % 6 == 2 else 0), "output": ["policy"]},
        {"step": "run", "verdict": ("failed" if i % 6 == 1 else "ok"),
         "exit": (1 if i % 6 == 1 else 0), "output": ["run"]},
        {"step": "journal-end", "verdict": "changed", "exit": 1, "output": ["changed"]},
    ]
    run = [
        {"command": "true", "exit": 0, "tail": ["true"]},
        {"command": "false" if i % 6 == 1 else "true", "exit": (1 if i % 6 == 1 else 0),
         "tail": ["cmd"]},
    ]
    doc = {
        "root": f"R{i % 3}", "lang": ["python", "rust"][i % 2],
        "profile": ["workspace-write", "read-only"][i % 2],
        "changed": ["pkg/core.py"], "agent_id": f"a{i % 3}",
        "plan": {"proven": bool(i % 4 == 0), "commands": ["true", "true"], "unproven": []},
        "steps": steps, "run": run, "changes": ["modified pkg/core.py"] if i % 4 == 0 else [],
        "verdict": verdict, "exit": exit_code,
        "budget": {"declared": 100 + i, "gate": "ok", "remaining": 90 + i},
        "cost": {"tokens": 10 + i, "recorded": True, "overhead_s": 0.1 + n * 0.01,
                 "scope": "task"},
        "wall_s": 0.5 + n * 0.1,
    }
    if i % 5 == 0:
        doc["claim"] = {"who": "u", "verdict": "true", "paths": ["pkg/core.py"], "ledger": "x"}
    if i % 7 == 0:
        doc["handoff"] = {"id": "h", "verdict": "true", "base_commit": "abc", "state_hash": "H"}
    if not full:
        doc["changes"] = len(doc["changes"])
        doc["steps"] = [{k: v for k, v in s.items() if k != "output"} for s in steps]
        doc["run"] = [{"command": r["command"], "exit": r["exit"]} for r in run]
        doc["proven"] = bool(doc["plan"].get("proven"))
        doc["volatile"] = {
            "steps_seconds": [0.01, 0.02, 0.03, 0.04, 0.05],
            "run_seconds": [0.11, 0.12],
            "wall_s": doc.pop("wall_s"),
            "cost": doc.pop("cost"),
            "budget": doc.pop("budget"),
            "at": "2026-01-01T00:00:00+00:00",
        }
        doc.pop("plan", None)
    else:
        doc["volatile"] = {
            "steps_seconds": [0.01, 0.02, 0.03, 0.04, 0.05],
            "run_seconds": [0.11, 0.12],
            "wall_s": doc.pop("wall_s"),
            "cost": doc.pop("cost"),
            "budget": doc.pop("budget"),
            "at": "2026-01-01T00:00:00+00:00",
        }
    return doc


def self_test():
    passed = 0
    failed = []

    def check(name, cond, detail=""):
        nonlocal passed
        if cond:
            passed += 1
        else:
            failed.append(name + (": " + detail if detail else ""))

    with tempfile.TemporaryDirectory(prefix="agent-metrics-selftest-") as td:
        for i in range(60):
            path = os.path.join(td, f"rc-{i:03d}.json")
            _write(path, _receipt(i, full=(i % 2 == 0)))
        for name, body in [
            ("bad.json", "{not-json"),
            ("array.json", "[]"),
            ("missing-verdict.json", json.dumps({"steps": [], "run": []})),
            ("missing-steps.json", json.dumps({"verdict": "passed", "run": []})),
            ("missing-run.json", json.dumps({"verdict": "passed", "steps": []})),
            ("delta.json", json.dumps({"against": "x", "added": {}})),
        ]:
            path = os.path.join(td, name)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(body)
        files = collect([td])
        records = [load_receipt(p) for p in files]
        m = build_metrics(records)
        check("collect files", len(files) == 66, str(len(files)))
        check("valid count", m["receipts"]["valid"] == 60)
        check("malformed count", m["receipts"]["malformed"] == 5)
        check("delta count", m["receipts"]["delta"] == 1)
        check("mode split", m["receipts"]["full"] == 30 and m["receipts"]["compact"] == 30)
        expected_classes = ("passed", "failed", "refused", "suspended", "dry_run", "rejected")
        valid = [r for r in records if not r.get("error") and not r.get("delta") and
                 not r.get("missing_verdict") and not r.get("missing_steps") and
                 not r.get("missing_run")]
        for r in valid:
            base = int(os.path.basename(r["path"])[3:6])
            check(f"record class {base}", r["vclass"] == expected_classes[base % 6],
                  r["vclass"])
            check(f"record mode {base}", r["mode"] == ("full" if base % 2 == 0 else "compact"),
                  r["mode"])
        check("verdict count", m["quality"]["runs"] == 60)
        check("class counts", all(m["verdicts"]["classes"][k] == 10 for k in
                                  ("passed", "failed", "refused", "suspended", "dry_run", "rejected")))
        check("class rates", m["verdicts"]["class_rates"]["passed"] == 0.1667)
        check("exit codes", m["exit_codes"].get("0") == 20 and m["exit_codes"].get("1") == 40)
        check("quality rates", m["quality"]["success_rate"] == 0.1667 and
              m["quality"]["failure_rate"] == 0.1667 and
              m["quality"]["refusal_rate"] == 0.1667)
        check("steps total", m["steps"]["total"] == 300)
        check("run total", m["run"]["total"] == 120)
        check("steps per step", all(m["steps"]["per_step"][k]["count"] == 60 for k in
                                    ("memory-brief", "preflight", "policy", "run", "journal-end")))
        check("steps failures", m["steps"]["failures"].get("policy") == 10 and
              m["steps"]["failures"].get("run") == 10)
        check("first failure", m["steps"]["first_failure"].get("policy", 0) >= 1)
        check("step seconds", m["steps"]["seconds"]["count"] == 300 and
              m["steps"]["seconds"]["p95"] == 0.05)
        check("slow list", len(m["steps"]["top_slow"]) <= 5)
        check("run nonzero", m["run"]["nonzero"] == 10)
        check("run seconds", m["run"]["seconds"]["count"] == 120 and
              m["run"]["seconds"]["p95"] == 0.12)
        check("changes", m["changes"]["runs_with_changes"] == 15 and
              m["changes"]["total"] == 15)
        check("budget", m["budget"]["with_budget"] == 60 and
              m["budget"]["declared_total"] == 60 * 100 + sum(range(60)))
        check("cost", m["cost"]["with_tokens"] == 60 and
              m["cost"]["tokens_total"] == 60 * 10 + sum(range(60)))
        check("cost mean", m["cost"]["tokens_mean"] == 39.5)
        check("overhead", m["cost"]["overhead_total_s"] > 0)
        check("claims", m["claims"]["with_claim"] == 12 and
              m["claims"]["claimed_paths_total"] == 12)
        check("handoff", m["claims"]["with_handoff"] == 9)
        check("timing", m["timing"]["wall_s"]["count"] == 60 and
              m["timing"]["wall_total_s"] > 0)
        check("bytes stats", m["receipts"]["bytes"]["count"] == 66 and
              m["receipts"]["bytes"]["p95"] > 0)
        check("groups agents", len(m["groups"]["by_agent"]) == 3 and
              sum(x["runs"] for x in m["groups"]["by_agent"].values()) == 60)
        check("groups langs", sum(x["runs"] for x in m["groups"]["by_lang"].values()) == 60)
        check("groups profiles", sum(x["runs"] for x in m["groups"]["by_profile"].values()) == 60)
        check("integrity missing", m["integrity"]["missing_verdict"] == 1 and
              m["integrity"]["missing_steps"] == 1 and m["integrity"]["missing_run"] == 1)
        check("strict false", m["integrity"]["strict_ok"] is False)
        check("compact ratio", m["receipts"]["compact_ratio"] is not None)

        out = os.path.join(td, "metrics.json")
        code = run(files, out, strict=False, quiet=True)
        check("run rc", code == 0)
        with open(out, encoding="utf-8") as fh:
            written = json.load(fh)
        check("out schema", written["schema"] == SCHEMA and written["quality"]["runs"] == 60)
        check("strict rc", run(files, "", strict=True, quiet=True) == 1)

        # Real dry-run receipt: validates the reader against agent_verify's current shape.
        real = os.path.join(td, "real")
        os.makedirs(os.path.join(real, "pkg"))
        with open(os.path.join(real, "pkg", "core.py"), "w", encoding="utf-8") as fh:
            fh.write("def f():\n    return 1\n")
        _call(["git", "init", "-q"], cwd=real)
        _call(["git", "add", "-A"], cwd=real)
        _call(["git", "-c", "user.email=metrics@test", "-c", "user.name=metrics",
               "commit", "-qm", "init"], cwd=real)
        plan = os.path.join(real, "plan.json")
        _write(plan, {"proven": True, "commands": ["true"], "unproven": []})
        receipt = os.path.join(real, "receipt.json")
        av = _call([AV, "-Root", real, "-Changed", "pkg/core.py", "-Lang", "python",
                    "-Plan", plan, "-Store", os.path.join(real, "store"),
                    "-Receipt", receipt, "-DryRun", "--no-claim-gate"])
        check("real agent_verify rc", av.returncode == 0, av.stdout + av.stderr)
        real_rec = load_receipt(receipt) if os.path.exists(receipt) else {"error": "no receipt"}
        check("real receipt parse", not real_rec.get("error"), str(real_rec.get("error")))
        real_m = build_metrics([real_rec])
        check("real receipt runs", real_m["quality"]["runs"] == 1)
        check("real receipt dry-run", real_m["quality"]["dry_run"] == 1)
        check("real receipt steps", real_m["steps"]["total"] >= 3)

        # Live scenario matrix: six real agent_verify runs, all through the same
        # fixture, covering pass/fail/refuse/suspend/dry-run/compact receipt shapes.
        plan_true = os.path.join(real, "plan-true.json")
        plan_false = os.path.join(real, "plan-false.json")
        plan_deny = os.path.join(real, "plan-deny.json")
        _write(plan_true, {"proven": True, "commands": ["true"], "unproven": []})
        _write(plan_false, {"proven": True, "commands": ["false"], "unproven": []})
        _write(plan_deny, {"proven": True, "commands": ["rm -rf ."], "unproven": []})
        store = os.path.join(real, "store")
        live = [real_rec]
        for name, plan, extra, want_rc, want_class in [
            ("full", plan_true, (), 0, "passed"),
            ("false", plan_false, (), 1, "failed"),
            ("budget", plan_true, ("--budget-token", "0"), 1, "suspended"),
            ("deny", plan_deny, (), 1, "refused"),
            ("compact", plan_true, ("--compact",), 0, "passed"),
        ]:
            rc_path = os.path.join(real, f"rc-{name}.json")
            res = _call([AV, "-Root", real, "-Changed", "pkg/core.py", "-Lang", "python",
                         "-Plan", plan, "-Store", store, "-Receipt", rc_path,
                         "--no-claim-gate"] + list(extra))
            check(f"live {name} rc", res.returncode == want_rc, res.stdout + res.stderr)
            rec = load_receipt(rc_path) if os.path.exists(rc_path) else {"error": "no receipt"}
            check(f"live {name} receipt", not rec.get("error") and rec.get("vclass") == want_class,
                  str(rec.get("error") or rec.get("vclass")))
            live.append(rec)
        live_m = build_metrics(live)
        check("live aggregate runs", live_m["quality"]["runs"] == 6)
        check("live aggregate classes", live_m["quality"]["passed"] == 2 and
              live_m["quality"]["failed"] == 1 and live_m["quality"]["refused"] == 1 and
              live_m["quality"]["suspended"] == 1 and live_m["quality"]["dry_run"] == 1)
        check("live aggregate run exits", live_m["run"]["nonzero"] == 1)
        check("live aggregate compact", live_m["receipts"]["compact"] == 1 and
              live_m["receipts"]["full"] == 5)
        check("live aggregate strict", live_m["integrity"]["strict_ok"] is True)
        check("live aggregate first failure", live_m["steps"]["first_failure"].get("policy", 0) == 1 and
              live_m["steps"]["first_failure"].get("run", 0) == 1)

    total = passed + len(failed)
    print(f"agent-metrics self-test: {passed}/{total}")
    if failed:
        for f in failed:
            print("FAIL " + f)
        return 1
    return 0


def main(argv):
    p = argparse.ArgumentParser(prog="agent-metrics", add_help=True)
    p.add_argument("-Receipts", dest="receipts", default="")
    p.add_argument("-Out", dest="out", default="")
    p.add_argument("-Strict", dest="strict", action="store_true")
    p.add_argument("-Quiet", dest="quiet", action="store_true")
    p.add_argument("-SelfTest", dest="selftest", action="store_true")
    a = p.parse_args(argv)
    if a.selftest:
        return self_test()
    if not a.receipts:
        print("agent-metrics: -Receipts is required (file, directory, glob, or comma list)")
        return 2
    files = collect([x.strip() for x in a.receipts.split(",") if x.strip()])
    if not files:
        print("agent-metrics: no receipt files matched")
        return 2
    return run(files, a.out, a.strict, a.quiet)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
