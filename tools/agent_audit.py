#!/usr/bin/env python3
"""One-shot audit over the receipts a task already produced.

The model-facing receipt is a routing view; this tool is the opposite surface:
it reads the full audit artifacts, hashes them, checks that a full (not compact)
receipt exists, and folds every security report into one PASS/FAIL answer.

    python tools/agent_audit.py --root . --receipt out/full.json \
        --scan out/security-gate.json --out out/audit-once.json --json

Fail-closed rules:
  * malformed JSON / missing full receipt / root mismatch -> fail, not "skip";
  * a receipt with exit != 0 -> fail (a refusal is evidence, not a pass);
  * a compact-only receipt when audit.require_full is true -> fail;
  * a security report with high/critical findings or a failed verdict -> fail;
  * a delta envelope is a routing artifact, not an audit artifact -> fail with
    "rebuild the full receipt first" (the caller can pass the rebuilt file).

This tool does not re-run commands.  It verifies the evidence that exists and
makes the "can I trust this task in one look?" answer explicit.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from datetime import datetime

SCHEMA = "agent-audit/1"
FAIL_ON_DEFAULT = ("critical", "high")


def sha256_file(path):
    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest().upper()


def load_json(path):
    with open(path, "rb") as fh:
        raw = fh.read()
    return json.loads(raw.decode("utf-8-sig")), raw


def shape_of(doc):
    if not isinstance(doc, dict):
        return "invalid"
    if doc.get("delta") is True or doc.get("schema") == "av-delta/1":
        return "delta"
    if "plan" in doc and "proven" not in doc:
        return "full"
    if "proven" in doc and "plan" not in doc:
        return "compact"
    if "verdict" in doc and "steps" in doc:
        return "full"
    return "unknown"


def same_root(a, b):
    if not a or not b:
        return True  # absent root is not a mismatch; the receipt may be portable
    try:
        return os.path.normcase(os.path.realpath(str(a))) == os.path.normcase(os.path.realpath(str(b)))
    except OSError:
        return os.path.normcase(os.path.abspath(str(a))) == os.path.normcase(os.path.abspath(str(b)))


def add(check, verdict, detail, inputs=None):
    row = {"check": check, "verdict": verdict, "detail": detail}
    if inputs:
        row["inputs"] = inputs
    return row


def audit_receipt(path, root="", require_full=True):
    checks = []
    base = os.path.basename(path)
    try:
        doc, raw = load_json(path)
    except (OSError, UnicodeDecodeError, ValueError) as e:
        return [add(f"receipt:{base}", "false", f"unreadable: {e}")], {"path": path, "error": str(e)}
    shape = shape_of(doc)
    meta = {"path": path, "sha256": hashlib.sha256(raw).hexdigest().upper(),
            "shape": shape, "bytes": len(raw)}
    if isinstance(doc, dict):
        meta["root"] = doc.get("root")
        sec = doc.get("security") or {}
        if isinstance(sec, dict):
            if sec.get("report"):
                meta["security_report"] = sec.get("report")
            if sec.get("sha256"):
                meta["security_sha256"] = str(sec.get("sha256")).upper()
    if not isinstance(doc, dict):
        checks.append(add(f"receipt:{base}", "false", "receipt JSON root is not an object"))
        return checks, meta
    if shape == "delta":
        checks.append(add(f"receipt:{base}", "false",
                          "delta envelope is not an audit artifact; rebuild the full receipt first"))
        return checks, meta
    if shape == "compact" and require_full:
        checks.append(add(f"receipt:{base}", "false",
                          "compact model view cannot satisfy audit.require_full; keep the full receipt"))
    elif shape not in ("full", "compact"):
        checks.append(add(f"receipt:{base}", "false", "not an agent-verify receipt"))
    else:
        checks.append(add(f"receipt:{base}", "true", f"{shape} receipt parsed"))

    if not same_root(doc.get("root"), root):
        checks.append(add(f"receipt:{base}:root", "false",
                          f"root mismatch: receipt={doc.get('root')!r} expected={root!r}"))
    else:
        checks.append(add(f"receipt:{base}:root", "true", "root matches" if root else "root not constrained"))

    verdict = str(doc.get("verdict", "")).lower()
    exit_code = doc.get("exit")
    meta["verdict"] = verdict
    meta["exit"] = exit_code
    if exit_code != 0:
        checks.append(add(f"receipt:{base}:exit", "false",
                          f"receipt exit={exit_code!r} verdict={verdict!r}"))
    else:
        checks.append(add(f"receipt:{base}:exit", "true", f"exit=0 verdict={verdict or '(none)'}"))

    steps = doc.get("steps")
    if not isinstance(steps, list):
        checks.append(add(f"receipt:{base}:steps", "false", "no steps[] array"))
    else:
        bad = [s for s in steps if not isinstance(s, dict)
               or not isinstance(s.get("step"), str) or "verdict" not in s or "exit" not in s]
        checks.append(add(f"receipt:{base}:steps", "false" if bad else "true",
                          f"{len(steps)} steps, {len(bad)} malformed"))
        meta["steps"] = len(steps)

    # A full receipt must carry the run evidence that makes the verdict auditable.
    if shape == "full":
        missing = [k for k in ("changed", "run", "changes") if k not in doc]
        if verdict and not verdict.startswith(("refused", "suspended", "dry-run")):
            missing += [k for k in ("plan", "policy") if k not in doc]
        checks.append(add(f"receipt:{base}:full", "false" if missing else "true",
                          f"missing full fields: {missing}" if missing else "audit fields present"))
    if "security" in doc:
        sec = doc.get("security") or {}
        sv = str(sec.get("verdict", "")).lower()
        if sv not in ("pass", "ok", "true") or sec.get("exit") not in (0, None):
            checks.append(add(f"receipt:{base}:security", "false",
                              f"embedded security verdict={sv!r} exit={sec.get('exit')!r}"))
        else:
            checks.append(add(f"receipt:{base}:security", "true", "embedded security pass"))
    return checks, meta


def audit_scan(path, root="", fail_on=FAIL_ON_DEFAULT, expected_sha256=""):
    base = os.path.basename(path)
    try:
        doc, raw = load_json(path)
    except (OSError, UnicodeDecodeError, ValueError) as e:
        return [add(f"scan:{base}", "false", f"unreadable: {e}")], {"path": path, "error": str(e)}
    meta = {"path": path, "sha256": hashlib.sha256(raw).hexdigest().upper(),
            "schema": doc.get("schema") if isinstance(doc, dict) else None}
    checks = []
    if expected_sha256 and meta["sha256"] != str(expected_sha256).upper():
        checks.append(add(f"scan:{base}:sha256", "false",
                          f"sha256 mismatch: report={meta['sha256']} receipt={str(expected_sha256).upper()}"))
    else:
        checks.append(add(f"scan:{base}:sha256", "true", "report hash matches receipt" if expected_sha256 else "hash recorded"))
    if not isinstance(doc, dict) or doc.get("schema") not in ("security-gate/1", "security-gate/0"):
        checks.append(add(f"scan:{base}", "false", "not a security-gate report"))
        return checks, meta
    if not same_root(doc.get("root"), root):
        checks.append(add(f"scan:{base}:root", "false",
                          f"root mismatch: report={doc.get('root')!r} expected={root!r}"))
    else:
        checks.append(add(f"scan:{base}:root", "true", "root matches" if root else "root not constrained"))
    verdict = str(doc.get("verdict", "")).lower()
    meta["verdict"] = verdict
    if verdict != "pass" or doc.get("exit") != 0:
        checks.append(add(f"scan:{base}:verdict", "false",
                          f"verdict={verdict!r} exit={doc.get('exit')!r}"))
    else:
        checks.append(add(f"scan:{base}:verdict", "true", "security gate pass"))
    findings = doc.get("findings") or []
    hits = [f for f in findings if isinstance(f, dict)
            and str(f.get("severity", "")).lower() in set(fail_on)]
    meta["findings"] = len(findings)
    meta["blocking"] = len(hits)
    if hits:
        checks.append(add(f"scan:{base}:findings", "false",
                          f"{len(hits)} finding(s) at/above {list(fail_on)}"))
    else:
        checks.append(add(f"scan:{base}:findings", "true", f"0 blocking among {len(findings)} findings"))
    gaps = doc.get("gaps") or []
    if gaps:
        checks.append(add(f"scan:{base}:gaps", "false", f"{len(gaps)} required scanner gap(s)"))
    else:
        checks.append(add(f"scan:{base}:gaps", "true", "no required scanner gaps"))
    return checks, meta


def summarize(checks, receipts, scans, root):
    false = [c for c in checks if c["verdict"] == "false"]
    unknown = [c for c in checks if c["verdict"] == "unknown"]
    verdict = "fail" if false else "unknown" if unknown else "pass"
    fallback_root = ""
    if receipts and receipts[0].get("path"):
        fallback_root = os.path.dirname(os.path.abspath(receipts[0]["path"]))
    doc = {
        "schema": SCHEMA,
        "root": root or fallback_root,
        "verdict": verdict,
        "exit": 0 if verdict == "pass" else 1,
        "at": datetime.now().astimezone().isoformat(timespec="microseconds"),
        "receipts": receipts,
        "scans": scans,
        "checks": checks,
        "summary": {
            "checks": len(checks),
            "passed": len([c for c in checks if c["verdict"] == "true"]),
            "failed": len(false),
            "unknown": len(unknown),
        },
    }
    return doc


def self_test():
    import tempfile
    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    with tempfile.TemporaryDirectory() as td:
        root = os.path.join(td, "repo")
        os.makedirs(root)
        full = {
            "root": root, "lang": "rust", "profile": "workspace-write",
            "changed": ["a.rs"], "plan": {"proven": True}, "policy": [],
            "steps": [{"step": "preflight", "verdict": "ok", "exit": 0}],
            "run": [{"command": "true", "exit": 0}], "changes": ["a.rs"],
            "verdict": "ok", "exit": 0,
            "settings": {"name": "deepseek-strict", "source": "builtin", "policy_profile": "workspace-write"},
            "security": {"verdict": "pass", "exit": 0},
        }
        p_full = os.path.join(td, "full.json")
        with open(p_full, "w", encoding="utf-8") as fh:
            json.dump(full, fh)
        c, m = audit_receipt(p_full, root)
        ok(all(x["verdict"] == "true" for x in c), "full good receipt passes")
        ok(m["shape"] == "full" and len(m["sha256"]) == 64, "full shape/hash")

        compact = {"root": root, "lang": "rust", "profile": "workspace-write",
                   "verdict": "ok", "exit": 0, "proven": True,
                   "steps": [{"step": "preflight", "verdict": "ok", "exit": 0}],
                   "run": [{"command": "true", "exit": 0}], "changes": 1,
                   "settings": {"name": "deepseek-strict", "source": "builtin",
                                "policy_profile": "workspace-write"}}
        p_compact = os.path.join(td, "compact.json")
        with open(p_compact, "w", encoding="utf-8") as fh:
            json.dump(compact, fh)
        c, m = audit_receipt(p_compact, root, require_full=True)
        ok(any(x["verdict"] == "false" and "require_full" in x["detail"] for x in c),
           "compact fails require_full")
        c, m = audit_receipt(p_compact, root, require_full=False)
        ok(not any(x["verdict"] == "false" for x in c), "compact allowed when require_full false")

        bad = dict(full, exit=1, verdict="refused by policy")
        p_bad = os.path.join(td, "bad.json")
        with open(p_bad, "w", encoding="utf-8") as fh:
            json.dump(bad, fh)
        c, m = audit_receipt(p_bad, root)
        ok(any(x["verdict"] == "false" and "exit=1" in x["detail"] for x in c), "nonzero exit fails")

        wrong = dict(full, root=os.path.join(td, "other"))
        p_wrong = os.path.join(td, "wrong.json")
        with open(p_wrong, "w", encoding="utf-8") as fh:
            json.dump(wrong, fh)
        c, m = audit_receipt(p_wrong, root)
        ok(any(x["verdict"] == "false" and "root mismatch" in x["detail"] for x in c), "root mismatch fails")

        delta = {"delta": True, "schema": "av-delta/1", "verdict": "ok", "exit": 0}
        p_delta = os.path.join(td, "delta.json")
        with open(p_delta, "w", encoding="utf-8") as fh:
            json.dump(delta, fh)
        c, m = audit_receipt(p_delta, root)
        ok(any(x["verdict"] == "false" and "rebuild" in x["detail"] for x in c), "delta is not audit artifact")

        p_mal = os.path.join(td, "mal.json")
        with open(p_mal, "w", encoding="utf-8") as fh:
            fh.write("{")
        c, m = audit_receipt(p_mal, root)
        ok(any(x["verdict"] == "false" and "unreadable" in x["detail"] for x in c), "malformed receipt fails")

        scan = {"schema": "security-gate/1", "root": root, "verdict": "pass", "exit": 0,
                "findings": [], "gaps": []}
        p_scan = os.path.join(td, "scan.json")
        with open(p_scan, "w", encoding="utf-8") as fh:
            json.dump(scan, fh)
        c, m = audit_scan(p_scan, root)
        ok(not any(x["verdict"] == "false" for x in c), "clean scan passes")
        c, m = audit_scan(p_scan, root, expected_sha256="0" * 64)
        ok(any(x["verdict"] == "false" and "sha256 mismatch" in x["detail"] for x in c),
           "receipt/report hash mismatch fails")

        scan_high = dict(scan, verdict="fail", exit=1,
                         findings=[{"severity": "high", "id": "S1"}])
        p_scan_high = os.path.join(td, "scan-high.json")
        with open(p_scan_high, "w", encoding="utf-8") as fh:
            json.dump(scan_high, fh)
        c, m = audit_scan(p_scan_high, root)
        ok(any(x["verdict"] == "false" and "finding" in x["detail"] for x in c), "high finding fails")

        # End-to-end summary: one full receipt + one clean scan is PASS; the
        # same receipt with a high scan is FAIL.
        checks_all = audit_receipt(p_full, root)[0] + audit_scan(p_scan, root)[0]
        doc = summarize(checks_all, [m], [m], root)
        ok(doc["verdict"] == "pass" and doc["exit"] == 0, "one-shot pass")
        checks_bad = audit_receipt(p_full, root)[0] + audit_scan(p_scan_high, root)[0]
        doc2 = summarize(checks_bad, [m], [m], root)
        ok(doc2["verdict"] == "fail" and doc2["exit"] == 1, "one-shot fail")
        ok(doc2["summary"]["failed"] >= 1, "failed count")

    print(f"agent-audit self-test: {checks}/{checks}")
    return 0


def parse(argv):
    out = {"root": "", "receipt": [], "scan": [], "out": "", "json": False,
           "allow_unknown": False, "require_full": True}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("--self-test",):
            out["self_test"] = True
            i += 1
        elif a in ("--json",):
            out["json"] = True
            i += 1
        elif a in ("--allow-unknown",):
            out["allow_unknown"] = True
            i += 1
        elif a in ("--no-require-full",):
            out["require_full"] = False
            i += 1
        elif a in ("--root", "-Root"):
            out["root"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--receipt", "-Receipt"):
            out["receipt"].append(argv[i + 1] if i + 1 < len(argv) else "")
            i += 2
        elif a in ("--scan", "-Scan"):
            out["scan"].append(argv[i + 1] if i + 1 < len(argv) else "")
            i += 2
        elif a in ("--out", "-Out"):
            out["out"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--help", "-h"):
            out["help"] = True
            i += 1
        else:
            i += 1
    return out


def main(argv):
    a = parse(argv)
    if a.get("self_test"):
        return self_test()
    if a.get("help"):
        print("agent-audit: --root DIR --receipt full.json [--receipt ...] "
              "[--scan security.json ...] [--out report.json] [--json] [--allow-unknown]")
        return 0
    if not a["receipt"] and not a["scan"]:
        print("agent-audit: need at least one --receipt or --scan")
        return 2
    receipts, scans = [], []
    checks = []
    for path in a["receipt"]:
        c, m = audit_receipt(path, a["root"], a["require_full"])
        checks.extend(c)
        receipts.append(m)

    # A full receipt already records the security report path and hash.  Pull it
    # in automatically so "--receipt full.json" is genuinely one-shot; an
    # explicit --scan still wins, and a stale/missing report is a hard failure.
    scan_jobs = []
    scan_sha = {}
    seen = set()

    def add_scan(path, expected_sha=""):
        key = os.path.normcase(os.path.abspath(path))
        if key in seen:
            return
        seen.add(key)
        scan_jobs.append(path)
        if expected_sha:
            scan_sha[key] = expected_sha

    for path in a["scan"]:
        add_scan(path)
    if not a["scan"]:
        for m in receipts:
            report = m.get("security_report")
            if not report:
                continue
            base_root = a["root"] or m.get("root") or ""
            path = report if os.path.isabs(report) else os.path.join(base_root, report)
            add_scan(path, m.get("security_sha256", ""))

    fail_on = list(FAIL_ON_DEFAULT)
    scan_cfg = (a["root"] and os.path.join(a["root"], ".agent", "settings.json"))
    if scan_cfg and os.path.isfile(scan_cfg):
        try:
            with open(scan_cfg, "r", encoding="utf-8-sig") as fh:
                cfg = json.load(fh)
            fail_on = list((cfg.get("security", {}) or {}).get("fail_on", fail_on))
        except (OSError, ValueError):
            pass
    for path in scan_jobs:
        key = os.path.normcase(os.path.abspath(path))
        c, m = audit_scan(path, a["root"], fail_on, scan_sha.get(key, ""))
        checks.extend(c)
        scans.append(m)
    doc = summarize(checks, receipts, scans, a["root"])
    if a["allow_unknown"] and doc["verdict"] == "unknown":
        doc["verdict"] = "pass"
        doc["exit"] = 0
    if a["out"]:
        os.makedirs(os.path.dirname(os.path.abspath(a["out"])), exist_ok=True)
        with open(a["out"], "w", encoding="utf-8", newline="\n") as fh:
            json.dump(doc, fh, ensure_ascii=True, indent=2, sort_keys=True)
            fh.write("\n")
    if a["json"]:
        print(json.dumps(doc, ensure_ascii=True, sort_keys=True))
    else:
        print(f"agent-audit: {doc['verdict'].upper()} receipts={len(receipts)} scans={len(scans)} "
              f"checks={doc['summary']['checks']} failed={doc['summary']['failed']} "
              f"unknown={doc['summary']['unknown']}")
        for c in doc["checks"]:
            if c["verdict"] != "true":
                print(f"  {c['verdict'].upper():7} {c['check']}: {c['detail']}")
    return doc["exit"]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
