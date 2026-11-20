#!/usr/bin/env python3
"""checks: a problem is not recorded until it is executable.

WHY THIS EXISTS
  docs/ERRORS.md is an index, not the authority. An entry that only says "this was a bug"
  is a claim nobody rechecks: six months later it reads as fact and is quietly stale, and
  the next reader has to ask a human "is this still a problem?". So the registry
  (.agent/checks/registry.json) is the authority and the ledger is its table of contents.
  An entry without a check is DEBT, counted and printed, not silently tolerated.

WHY IT IS LANGUAGE-AGNOSTIC
  The unit of work is either (command -> exit code) or (pattern -> hit count). Nothing here
  knows what cargo, pytest or npm is, so the same mechanism guards a Python service, a Go
  CLI, a front-end build, or this kernel itself. forbid/require are pure text and cost
  milliseconds, which is why they are the default tier: a check that is expensive to run
  is a check that gets skipped, and a skipped check is worse than no check because it lies.

FAIL-CLOSED, TWICE
  1. A forbid/require check whose scope matches no file is NOT a pass. Deleting or renaming
     the guarded file would otherwise turn every rule about it green. It is a FAIL unless
     the check explicitly says allow_skip, and allow_skip is printed as SKIP, never PASS.
  2. Audit exits non-zero under --strict as soon as debt exists, so CI can refuse a ledger
     that grew a prose-only entry.

MODES
  add     record a check (id must be new; covers must name existing ledger entries)
  run     execute; PASS/FAIL/SKIP per check; any FAIL -> exit 1
  audit   registry integrity + ledger coverage (--strict makes debt fatal)
  list    what is registered, and which ledger entries it covers

EXIT  0 ok; 1 a check failed or audit found a gap; 2 bad invocation
"""

import argparse
import fnmatch
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime

KINDS = {"cmd", "forbid", "require"}
MODES = {"add", "run", "audit", "list", "selftest"}
TIERS = {"fast", "heavy"}

# Ledger rows look like:  | L01 | 现象 | 根因 | 修法 | I12、M10、M11 |
LEDGER_ROW = re.compile(r"^\|\s*(L\d+)\s*\|(.*)$")
# Selftest group ids (I12/M10/N3/Q5/T10) plus new-style check ids (C001).
FIXED_ID = re.compile(r"\b([A-Z]\d+|C\d{3})\b")
DEBT_MARK = re.compile(r"^\s*(—|-|未固化|none)\s*$")


def now():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def paths(root, ledger, registry):
    reg = registry or os.path.join(root, ".agent", "checks", "registry.json")
    led = ledger or os.path.join(root, "docs", "ERRORS.md")
    return reg, led


def load_json(path):
    if not os.path.exists(path):
        return None
    with open(path, "r", encoding="utf-8-sig") as fh:
        return json.load(fh)


def save_json(path, doc):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False, indent=2)
        fh.write("\n")


def parse_ledger(text):
    """Return {L-id: {'fix': [...ids], 'debt': bool, 'title': str}}.

    The last cell of a row is the 固化 column. A row whose 固化 cell is empty or says
    '—'/'未固化' is debt: it describes a problem nothing can re-detect.
    """
    out = {}
    for line in text.split("\n"):
        m = LEDGER_ROW.match(line)
        if not m:
            continue
        lid, rest = m.group(1), m.group(2)
        cells = [c.strip() for c in rest.split("|")]
        # rest excludes the leading '|' but keeps the trailing one; drop empties at the end
        while cells and not cells[-1]:
            cells.pop()
        if len(cells) < 4:
            continue
        fix_cell = cells[-1]
        ids = FIXED_ID.findall(fix_cell) if not DEBT_MARK.match(fix_cell) else []
        out[lid] = {
            "title": cells[0][:60],
            "fix": ids,
            "debt": DEBT_MARK.match(fix_cell) is not None or not fix_cell,
            "fix_text": fix_cell,
        }
    return out


def select_files(root, scope):
    """Expand globs against the working tree. Returns existing files only."""
    hits = []
    for pat in scope:
        if pat.startswith("~"):
            full = os.path.expanduser(pat)
            if os.path.isfile(full):
                hits.append(full)
            continue
        if os.path.isabs(pat):
            if os.path.isfile(pat):
                hits.append(pat)
            continue
        if any(ch in pat for ch in "*?["):
            for base, _dirs, names in os.walk(root):
                for n in names:
                    full = os.path.join(base, n)
                    rel = os.path.relpath(full, root).replace(os.sep, "/")
                    if fnmatch.fnmatch(rel, pat) or fnmatch.fnmatch(n, pat):
                        hits.append(full)
        else:
            full = os.path.join(root, pat)
            if os.path.isfile(full):
                hits.append(full)
    return sorted(set(hits))


def grep_files(files, pattern, cut=None, exclude_line=None, literal=False):
    """Count hits. `cut` stops scanning a file at the first line matching it.

    WHY: a forbid rule about production code must not be fired by the very tests that
    assert the rule holds. Rust puts tests after `#[cfg(test)]`, so the cut is where the
    test module starts -- without it every forbid rule would be red from day one and
    would be switched off, which is the failure mode this registry exists to prevent.

    WHY literal: a pattern like `rev-list "$local_sha" --not --remotes` read as a regex
    matches nothing, because `$` is an end-of-line anchor. A rule that can never fire is
    indistinguishable from a rule that always passes, so callers that mean a literal
    string say so. WHY exclude_line: the comment that explains why a forbidden form is
    not used would otherwise count as using it.
    """
    rx = re.compile(re.escape(pattern) if literal else pattern)
    cut_rx = re.compile(cut) if cut else None
    ex_rx = re.compile(exclude_line) if exclude_line else None
    hits = []
    for f in files:
        try:
            with open(f, "r", encoding="utf-8-sig", errors="replace") as fh:
                for no, line in enumerate(fh, 1):
                    if cut_rx and cut_rx.search(line):
                        break
                    if rx.search(line):
                        if ex_rx and ex_rx.search(line):
                            continue
                        hits.append((os.path.basename(f), no, line.rstrip()[:120]))
        except OSError as exc:
            hits.append((f, 0, f"unreadable: {exc}"))
    return hits


def pick_bash():
    """Prefer Git for Windows by PATH, never the WSL stub.

    WHY: `shutil.which("bash")` on Windows can resolve to System32\\bash.exe, which is
    WSL -- a different filesystem where D:\\ is /mnt/d. A check that runs there fails for
    reasons unrelated to the code being checked (L44 in the ledger).
    """
    if os.name != "nt":
        # POSIX：bash 就在 PATH 里，也不存在 WSL 那种「另一个文件系统」的陷阱。
        return shutil.which("bash")
    candidates = [os.environ.get("BASH", ""),
                  r"C:\Program Files\Git\bin\bash.exe",
                  r"C:\Program Files (x86)\Git\bin\bash.exe"]
    for c in candidates:
        if c and os.path.isfile(c):
            return c
    git = os.environ.get("GIT_EXE", "")
    for g in ([git] if git else []) + [r"C:\Program Files\Git\cmd\git.exe"]:
        if os.path.isfile(g):
            cand = os.path.join(os.path.dirname(os.path.dirname(g)), "bin", "bash.exe")
            if os.path.isfile(cand):
                return cand
    return None


def run_cmd(chk, root):
    expect = int(chk.get("expect_exit") or 0)
    timeout = int(chk.get("timeout_s") or 120)
    shell_mode = chk.get("shell") or ""
    argv = chk["run"]
    shell = True
    if shell_mode == "bash":
        bash = pick_bash()
        if not bash:
            return "FAIL", "shell=bash requested but no Git Bash found (set BASH)"
        argv = [bash, "-lc", chk["run"]]
        shell = False
    elif shell_mode:
        shell = shell_mode
    try:
        p = subprocess.run(argv, shell=shell, cwd=root, timeout=timeout,
                           capture_output=True, text=True,
                           encoding="utf-8", errors="replace")
    except subprocess.TimeoutExpired:
        return "FAIL", f"timeout after {timeout}s"
    except OSError as exc:
        return "FAIL", f"cannot execute: {exc}"
    out = ((p.stdout or "") + (p.stderr or "")).strip().splitlines()
    tail = out[-3:] if out else []
    detail = " | ".join(tail)[:200]
    if p.returncode != expect:
        return "FAIL", f"exit {p.returncode} (expected {expect}) {detail}"
    return "PASS", detail


def evaluate(chk, root):
    kind = chk.get("kind")
    if kind == "cmd":
        return run_cmd(chk, root)

    files = select_files(root, chk.get("scope") or [])
    if not files:
        if chk.get("allow_skip"):
            return "SKIP", "scope matches no file (allow_skip)"
        return "FAIL", "scope matches no file -- the rule cannot be proven, so it is not green"

    hits = grep_files(files, chk["pattern"], chk.get("cut"),
                      chk.get("exclude_line"), bool(chk.get("literal")))
    if kind == "forbid":
        if hits:
            first = hits[0]
            return "FAIL", f"{len(hits)} hit(s); first {first[0]}:{first[1]}: {first[2]}"
        return "PASS", f"{len(files)} file(s), 0 hit"
    if kind == "require":
        need = int(chk.get("min") or 1)
        if len(hits) < need:
            return "FAIL", f"{len(hits)} hit(s), need >= {need}"
        return "PASS", f"{len(hits)} hit(s), need >= {need}"
    return "FAIL", f"unknown kind {kind}"


def cmd_add(a, reg_path, registry):
    for key in ("id", "title", "kind"):
        if not a.get(key):
            print(f"checks: -{key.capitalize()} is required")
            return 2
    if a["kind"] not in KINDS:
        print(f"checks: -Kind must be one of {sorted(KINDS)}")
        return 2
    if a["kind"] == "cmd" and not a.get("run"):
        print("checks: -Run is required for kind=cmd")
        return 2
    if a["kind"] in ("forbid", "require") and not a.get("pattern"):
        print("checks: -Pattern is required for forbid/require")
        return 2
    if not a.get("scope"):
        print("checks: -Scope is required (forbid/require need files to prove anything)")
        return 2

    doc = registry or {"version": 1, "owner_root": a["root_abs"], "baseline": a["since"], "checks": []}
    if any(c.get("id") == a["id"] for c in doc["checks"]):
        print(f"checks: id '{a['id']}' already exists -- ids are never reused")
        return 1

    entry = {"id": a["id"], "title": a["title"], "kind": a["kind"], "covers": a["covers"],
             "scope": a["scope"], "since": a["since"], "tier": a["tier"], "added": now()}
    if a["kind"] == "cmd":
        entry["run"] = a["run"]
        entry["shell"] = a["shell"] or None
        entry["expect_exit"] = a["expect_exit"]
    else:
        entry["pattern"] = a["pattern"]
        entry["min"] = a["min_count"]
        entry["allow_skip"] = a["allow_skip"]
        entry["literal"] = a["literal"]
        entry["exclude_line"] = a["exclude_line"] or None
        entry["cut"] = a["cut"] or None
    doc["checks"].append(entry)
    save_json(reg_path, doc)
    print(f"checks: added {a['id']} [{a['kind']}] {a['title']}")
    return 0


def main(argv):
    p = argparse.ArgumentParser(prog="checks", add_help=False)
    p.add_argument("mode", nargs="?", default="run")
    p.add_argument("-Root", dest="root", default="")
    p.add_argument("-Registry", dest="registry", default="")
    p.add_argument("-Ledger", dest="ledger", default="")
    p.add_argument("-Id", dest="id", default="")
    p.add_argument("-Title", dest="title", default="")
    p.add_argument("-Kind", dest="kind", default="")
    p.add_argument("-Run", dest="run", default="")
    p.add_argument("-Shell", dest="shell", default="")
    p.add_argument("-Pattern", dest="pattern", default="")
    p.add_argument("-Cut", dest="cut", default="")
    p.add_argument("-ExcludeLine", dest="exclude_line", default="")
    p.add_argument("-Literal", dest="literal", action="store_true")
    p.add_argument("-Scope", dest="scope", default="")
    p.add_argument("-Covers", dest="covers", default="")
    p.add_argument("-Since", dest="since", default="")
    p.add_argument("-Tier", dest="tier", default="fast")
    # 只在 CI 用：跳过 registry 里标了 local_only 的检查（它们依赖仓库外的本机文件）。
    p.add_argument("-Ci", dest="ci", action="store_true")
    p.add_argument("-Min", dest="min_count", default="")
    p.add_argument("-ExpectExit", dest="expect_exit", default="")
    p.add_argument("-Only", dest="only", default="")
    p.add_argument("-Strict", dest="strict", action="store_true")
    p.add_argument("-AllowSkip", dest="allow_skip", action="store_true")
    a = vars(p.parse_args(argv))

    if a["mode"] not in MODES:
        print(f"checks: -Mode must be one of {sorted(MODES)}")
        return 2
    root = os.path.abspath(a["root"] or os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    if not os.path.isdir(root):
        print("checks: -Root must be an existing directory")
        return 2
    a["root_abs"] = root
    a["scope"] = [x for x in re.split(r"[,;]", a["scope"]) if x.strip()]
    a["covers"] = [x for x in re.split(r"[,;]", a["covers"]) if x.strip()]
    a["since"] = a["since"] or now()[:10]
    a["expect_exit"] = int(a["expect_exit"] or 0)
    a["min_count"] = int(a["min_count"] or 1)
    reg_path, led_path = paths(root, a["ledger"], a["registry"])
    registry = load_json(reg_path)

    if a["mode"] == "add":
        return cmd_add(a, reg_path, registry)

    if a["mode"] == "selftest":
        # The guard must be guarded: these tests assert the mechanism (literal patterns,
        # cut, comment exclusion, fail-closed), not the project's current state.
        import unittest
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import checks_selftest
        suite = unittest.TestLoader().loadTestsFromModule(checks_selftest)
        result = unittest.TextTestRunner(verbosity=1, buffer=True).run(suite)
        return 0 if result.wasSuccessful() else 1

    if not registry:
        print(f"checks: no registry at {reg_path} (run add first)")
        return 2

    if a["mode"] == "list":
        for c in registry.get("checks", []):
            cov = ",".join(c.get("covers") or []) or "-"
            print(f"{c['id']:<6} {c['kind']:<8} {c.get('tier','fast'):<6} covers={cov:<12} {c['title']}")
        print(f"checks: {len(registry.get('checks', []))} registered")
        return 0

    ledger = {}
    if os.path.exists(led_path):
        with open(led_path, "r", encoding="utf-8-sig") as fh:
            ledger = parse_ledger(fh.read())

    if a["mode"] == "audit":
        return cmd_audit(registry, ledger, a)

    return cmd_run(registry, root, a)


def cmd_audit(registry, ledger, a):
    bad = []
    ids = {}
    for c in registry.get("checks", []):
        cid = c.get("id")
        if not cid:
            bad.append("check without id")
            continue
        if cid in ids:
            bad.append(f"duplicate check id {cid}")
        ids[cid] = c
        if c.get("kind") not in KINDS:
            bad.append(f"{cid}: bad kind {c.get('kind')}")
        if c.get("kind") == "cmd" and not c.get("run"):
            bad.append(f"{cid}: kind=cmd without run")
        if c.get("kind") in ("forbid", "require") and not c.get("pattern"):
            bad.append(f"{cid}: kind={c['kind']} without pattern")
        if not c.get("scope"):
            bad.append(f"{cid}: no scope")
        for lid in c.get("covers") or []:
            if lid not in ledger:
                bad.append(f"{cid}: covers {lid}, which is not in the ledger")

    debt, covered, legacy = [], [], []
    # Anything numbered at or above ledger_from MUST have a Cxxx check. Without this,
    # "write it in the ledger" quietly returns as the cheap way to close an issue, and
    # the next reader is back to asking a human whether it is still true.
    floor = registry.get("ledger_from") or ""
    floor_n = int(floor[1:]) if re.fullmatch(r"L\d+", floor) else None
    fresh_debt = []
    for lid, row in sorted(ledger.items()):
        if row["debt"]:
            debt.append(lid)
            if floor_n is not None and int(lid[1:]) >= floor_n:
                fresh_debt.append(lid)
            continue
        new = [i for i in row["fix"] if re.fullmatch(r"C\d{3}", i)]
        old = [i for i in row["fix"] if re.fullmatch(r"[A-Z]\d+", i) and i not in new]
        if new:
            for i in new:
                if i not in ids:
                    bad.append(f"{lid}: points at {i}, which is not registered")
            covered.append(lid)
        elif old:
            legacy.append(lid)
        else:
            debt.append(lid)
            if floor_n is not None and int(lid[1:]) >= floor_n:
                fresh_debt.append(lid)

    for b in bad:
        print(f"checks: AUDIT FAIL {b}")
    if fresh_debt:
        print(f"checks: AUDIT FAIL new entries without a check (>= {floor}): "
              f"{', '.join(fresh_debt)} -- a problem is not recorded until it is executable")
    print(f"checks: ledger {len(ledger)} entries | new-style {len(covered)} | "
          f"legacy selftest {len(legacy)} | DEBT {len(debt)}")
    if debt:
        shown = ", ".join(debt[:12]) + ("..." if len(debt) > 12 else "")
        print(f"checks: DEBT (prose only, nothing can re-detect them): {shown}")
    if legacy:
        print("checks: legacy entries are run by selftest.ps1, not by this registry "
              "-- migrate them to Cxxx when touching them")
    if bad or fresh_debt or (a["strict"] and debt):
        return 1
    return 0


def cmd_run(registry, root, a):
    only = [x.strip() for x in a["only"].split(",") if x.strip()]
    rows, counts = [], {"PASS": 0, "FAIL": 0, "SKIP": 0}
    for c in registry.get("checks", []):
        cid = c.get("id", "?")
        if only and cid not in only and not set(only) & set(c.get("covers") or []):
            continue
        # 本机专属：scope 指向仓库外的绝对路径（例如版权门禁脚本），CI 上必然
        # 匹配不到文件。这类检查只在本机有意义 —— CI 里显式跳过，而不是判红。
        if a.get("ci") and c.get("local_only"):
            counts["SKIP"] += 1
            rows.append(("SKIP", cid, c.get("title", ""), "local-only, CI 跳过"))
            continue
        if a["tier"] == "fast" and c.get("tier") == "heavy":
            counts["SKIP"] += 1
            rows.append(("SKIP", cid, c.get("title", ""), "tier=heavy, use -Tier heavy"))
            continue
        status, detail = evaluate(c, root)
        counts[status] += 1
        rows.append((status, cid, c.get("title", ""), detail))

    for status, cid, title, detail in rows:
        print(f"checks: {status} {cid} {title}")
        if status != "PASS" and detail:
            print(f"        {detail}")
    print(f"checks: {counts['PASS']} passed, {counts['FAIL']} failed, {counts['SKIP']} skipped")
    return 1 if counts["FAIL"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
