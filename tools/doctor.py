#!/usr/bin/env python3
"""doctor: project health snapshot -- the agent self-awareness entry point.

    python tools/doctor.py [--root DIR] [--json]

Aggregates only evidence-backed faces. Every face carries an evidence pointer and a
collection timestamp; nothing is inferred, nothing is diagnosed here. "Does the project
have a problem" is answered with a chain of evidence, not a vibe:

  selftest  last selftest record on disk (.agent/last-selftest.json, written by
            tools/selftest.ps1 after each run: summary line + UTC timestamp). Missing or
            corrupt -> UNAVAILABLE with the rerun command. doctor never runs the
            selftest itself -- it only reads what fell on disk.
  ledger    docs/ERRORS.md vs tools/selftest.ps1 + .agent/checks/registry.json. For each
            L-row outside section D the LAST cell is the 固化 column. An entry is pinned
            when that cell carries an assertion id that really exists in selftest.ps1
            (or a C0xx id that exists in the registry); an entry is debt when the cell
            is empty/dash/prose/group-reference ("N 组" is a claim about a group, not a
            pin); an id that exists nowhere is a GHOST -- the ledger claims a proof that
            does not exist, which is worse than open debt. Reverse direction: every Lxx
            id referenced in selftest.ps1 must exist in the ledger, otherwise the ledger
            went thin under our feet.
  registry  .agent/checks/registry.json: C0xx total and which checks currently cannot
            run (missing command on PATH / missing scope file). Existence probes only --
            no check is executed here.
  backlog   docs/ERRORS.md section D rows plus any row marked 欠账/待办/未固化.
  git       dirty files (git status --porcelain) and local branches whose tip commit is
            on no remote ref. Collected from git; UNAVAILABLE outside a repository.
  gates     scripts/privacy_scan.py and scripts/docs_hygiene.py, run in place (both are
            seconds-fast). Their exit codes are the evidence; doctor adds no opinion.

Statuses: OK / WARN / FAIL / UNAVAILABLE. A face degrades to UNAVAILABLE when its
evidence source is missing, unreadable, or structurally unrecognizable -- a broken
source must never masquerade as a green face.

Honest boundary: doctor only aggregates sourced state; it performs no diagnostic
reasoning and runs no heavy commands (no cargo, no selftest). It is an observation
surface, NOT a gate: a FAIL face never changes the exit code -- 0 whenever a report is
produced, 2 only for usage errors (bad --root). Division of labor: policy/constraints
block, doctor shows.
"""

import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ID_RX = re.compile(r"(?<![A-Za-z0-9])([A-Z]{1,2}\d{1,2}[a-z]?)(?![0-9A-Za-z])")
CID_RX = re.compile(r"(?<![A-Za-z0-9])(C\d{3})(?![0-9A-Za-z])")
ROW_RX = re.compile(r"^\|\s*(L\d+)\s*\|")
SECTION_RX = re.compile(r"^## ([A-Z])[.]")
LEDGER_REF_RX = re.compile(r"(?<![A-Za-z0-9])(L\d{2,3})(?![0-9])")
BACKLOG_KW = ("欠账", "待办", "未固化")
GATE_SCRIPTS = (("privacy", "scripts/privacy_scan.py"), ("docs", "scripts/docs_hygiene.py"))


def mk_face(name, status, evidence, detail):
    return {"face": name, "status": status, "evidence": evidence, "detail": detail}


def read_text(path):
    return path.read_text(encoding="utf-8", errors="replace")


def parse_ledger(text):
    """Rows keyed by L-id. Returns (entries, d_rows): entries = (lid, fix_cell, cells)
    for tables with a 固化 column (last cell), d_rows = (lid, cells) for section D."""
    entries, d_rows = [], []
    section = None
    for line in text.splitlines():
        m = SECTION_RX.match(line.strip())
        if m:
            section = m.group(1)
            continue
        m = ROW_RX.match(line)
        if not m:
            continue
        tmp = line.replace("\\|", "\x00")
        cells = [c.strip().replace("\x00", "|") for c in tmp.split("|")]
        if cells and cells[0] == "":
            cells = cells[1:]
        if cells and cells[-1] == "":
            cells = cells[:-1]
        lid = m.group(1)
        if section == "D":
            d_rows.append((lid, cells))
        else:
            # 5-column tables carry 固化 in the last cell; rows that lost the column
            # entirely (exactly 4 cells) are treated as unpinned, not as prose pins.
            fix = cells[-1] if len(cells) >= 5 else ""
            entries.append((lid, fix, cells))
    return entries, d_rows


def id_exists(token, selftest_text):
    return re.search(r"(?<![A-Za-z0-9])" + re.escape(token) + r"(?![0-9A-Za-z])", selftest_text)


def face_selftest(root):
    path = root / ".agent" / "last-selftest.json"
    rel = ".agent/last-selftest.json"
    rerun = "rerun: pwsh tools/selftest.ps1"
    if not path.is_file():
        return mk_face("selftest", "UNAVAILABLE", rel, "no record on disk; " + rerun)
    try:
        doc = json.loads(read_text(path))
        passed = int(doc["passed"])
        failed = int(doc["failed"])
    except (OSError, ValueError, KeyError, TypeError) as e:
        return mk_face("selftest", "UNAVAILABLE", rel,
                       "unreadable (%s: %.60s); %s" % (type(e).__name__, str(e), rerun))
    at = str(doc.get("at", "?"))
    detail = "passed=%d failed=%d skipped=%s at=%s" % (
        passed, failed, doc.get("skipped", "?"), at)
    return mk_face("selftest", "FAIL" if failed > 0 else "OK",
                   rel + " at=" + at, detail)


def face_ledger(entries, d_ids, selftest_text, registry_ids):
    if entries is None:
        return mk_face("ledger", "UNAVAILABLE", "docs/ERRORS.md",
                       "missing or no parseable rows (truncated?)")
    if selftest_text is None:
        return mk_face("ledger", "UNAVAILABLE", "tools/selftest.ps1",
                       "selftest source missing; claimed assertion ids unverifiable")
    all_ids = set(d_ids) | {lid for lid, _, _ in entries}
    ghost, unpinned, group_only = [], [], []
    pinned = 0
    unverified_c = 0
    for lid, fix, _cells in entries:
        toks = [t for t in ID_RX.findall(fix)]
        cids = CID_RX.findall(fix)
        bad = [t for t in toks if not id_exists(t, selftest_text)]
        bad += [c for c in cids if registry_ids is not None and c not in registry_ids]
        verified = any(id_exists(t, selftest_text) for t in toks) or \
            any(c in registry_ids for c in cids if registry_ids is not None)
        if registry_ids is None:
            unverified_c += len(cids)
        if bad:
            ghost.append((lid, bad))
        elif verified:
            pinned += 1
        else:
            unpinned.append(lid)
            if re.search(r"(?<![A-Za-z0-9])[A-Z]{1,2}\s*组", fix):
                group_only.append(lid)
    refs = set(LEDGER_REF_RX.findall(selftest_text))
    reverse_missing = sorted(r for r in refs if r not in all_ids)
    if ghost or reverse_missing:
        status = "FAIL"
    elif unpinned:
        status = "WARN"
    else:
        status = "OK"
    detail = ("entries=%d pinned=%d unpinned=%d ghost=%d group_only=%d reverse_missing=%d"
              % (len(entries), pinned, len(unpinned), len(ghost), len(group_only),
                 len(reverse_missing)))
    if registry_ids is None:
        detail += " registry_unavailable_c_ids_unverified=%d" % unverified_c
    if ghost:
        detail += " ghost=" + ";".join("%s:%s" % (l, ",".join(b)) for l, b in ghost[:8])
    if unpinned:
        detail += " gap_first10=" + ",".join(unpinned[:10])
    if reverse_missing:
        detail += " reverse=" + ",".join(reverse_missing[:8])
    return mk_face("ledger", status,
                   "docs/ERRORS.md rows=%d vs tools/selftest.ps1" % len(entries), detail)


def probe_check(root, chk):
    if chk.get("kind") == "cmd":
        parts = str(chk.get("run") or "").split()
        if not parts:
            return False, "no run"
        tok = parts[0]
        if "/" in tok or "\\" in tok:
            p = Path(tok.replace("~", str(Path.home())))
            if not p.is_absolute():
                p = root / p
            return p.exists(), "file missing"
        return shutil.which(tok) is not None, "command missing"
    scopes = chk.get("scope") or []
    if not scopes:
        return False, "no scope"
    for pat in scopes:
        pp = str(pat).replace("~", str(Path.home()))
        if any(ch in pp for ch in "*?["):
            if glob.glob(pp) or glob.glob(str(root / pp)):
                return True, ""
        else:
            p = Path(pp)
            if not p.is_absolute():
                p = root / p
            if p.exists():
                return True, ""
    return False, "scope files missing"


def face_registry(root, doc):
    if doc is None:
        return mk_face("registry", "UNAVAILABLE", ".agent/checks/registry.json",
                       "missing or unreadable (bad JSON?)")
    raw = doc.get("checks") or []
    if not isinstance(raw, list) or any(not isinstance(c, dict) for c in raw):
        return mk_face("registry", "UNAVAILABLE", ".agent/checks/registry.json",
                       "malformed shape (checks must be a list of objects)")
    checks = raw
    notrun = []
    for chk in checks:
        ok, why = probe_check(root, chk)
        if not ok:
            notrun.append("%s:%s" % (chk.get("id", "?"), why))
    status = "OK" if not notrun else "WARN"
    detail = "checks=%d runnable=%d not_runnable=%d" % (
        len(checks), len(checks) - len(notrun), len(notrun))
    if notrun:
        detail += " [" + ",".join(notrun[:10]) + "]"
    return mk_face("registry", status, ".agent/checks/registry.json", detail)


def face_backlog(entries, d_rows):
    if entries is None:
        return mk_face("backlog", "UNAVAILABLE", "docs/ERRORS.md",
                       "missing or no parseable rows (truncated?)")
    ids = [lid for lid, _ in d_rows]
    keyword = []
    for lid, _fix, cells in entries:
        joined = " ".join(cells)
        if any(k in joined for k in BACKLOG_KW):
            keyword.append(lid)
    total_ids = ids + keyword
    detail = "backlog=%d d_section=%d keyword=%d" % (len(total_ids), len(ids), len(keyword))
    if total_ids:
        detail += " ids=" + ",".join(total_ids[:10])
        status = "WARN"
    else:
        status = "OK"
    return mk_face("backlog", status, "docs/ERRORS.md", detail)


def face_git(root):
    def run(args):
        return subprocess.run(["git"] + args, cwd=str(root), capture_output=True,
                              text=True, encoding="utf-8", errors="replace", timeout=60)
    try:
        chk = run(["rev-parse", "--is-inside-work-tree"])
    except (OSError, subprocess.SubprocessError) as e:
        return mk_face("git", "UNAVAILABLE", "git", "git unavailable (%s)" % type(e).__name__)
    if chk.returncode != 0 or chk.stdout.strip() != "true":
        return mk_face("git", "UNAVAILABLE", "git", "not a git work tree")
    ev = []
    try:
        st = run(["status", "--porcelain"])
        dirty = len([l for l in st.stdout.splitlines() if l.strip()])
        ev.append("status exit=%d" % st.returncode)
        remote = set(run(["for-each-ref", "--format=%(objectname)",
                          "refs/remotes"]).stdout.split())
        bl = run(["for-each-ref", "--format=%(objectname)|%(refname:short)", "refs/heads"])
        ev.append("for-each-ref exit=%d" % bl.returncode)
        branches, unpushed = 0, []
        for line in bl.stdout.splitlines():
            if "|" not in line:
                continue
            sha, name = line.split("|", 1)
            branches += 1
            if sha not in remote:
                unpushed.append(name)
    except (OSError, subprocess.SubprocessError) as e:
        return mk_face("git", "UNAVAILABLE", "git", "git command failed (%s)" % type(e).__name__)
    detail = "dirty=%d branches=%d unpushed=%d" % (dirty, branches, len(unpushed))
    if unpushed:
        detail += " first=" + ",".join(unpushed[:5])
    status = "WARN" if (dirty or unpushed) else "OK"
    return mk_face("git", status, "; ".join(ev), detail)


CACHE_REL = ".agent/doctor-cache.json"
# same skip discipline as tools/journal.py: local build/interpreter artifacts are not
# gate inputs, and counting them makes the fingerprint churn every time a gate runs
# (a gate that writes __pycache__ would invalidate its own cache).
SCAN_SKIP = {".git", ".journal", ".agent", "target", "node_modules", ".trash", ".qoder",
             "__pycache__", ".venv", "dist", "build", "obj", ".pytest_cache",
             ".ruff_cache", ".mypy_cache", ".turbo", ".next", "coverage"}


def tree_fingerprint(root):
    """git-index discipline: (rel, mtime_ns, size) stat walk over the scanned tree.
    mtime+size can theoretically miss an edit that preserves both -- --refresh
    re-verifies on demand, same trade git itself makes with its index."""
    h = hashlib.sha256()
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SCAN_SKIP)
        for fn in sorted(filenames):
            p = Path(dirpath) / fn
            try:
                st = p.stat()
                h.update(("%s|%d|%d\n" % (p.relative_to(root).as_posix(),
                                          st.st_mtime_ns, st.st_size)).encode())
            except OSError:
                h.update(("%s|gone\n" % p.relative_to(root).as_posix()).encode())
    return h.hexdigest()[:16]


def load_gate_cache(root):
    try:
        doc = json.loads((root / CACHE_REL).read_text(encoding="utf-8"))
        if isinstance(doc, dict) and isinstance(doc.get("faces"), dict):
            return doc
    except (OSError, ValueError):
        pass
    return None


def save_gate_cache(root, fp, faces):
    (root / CACHE_REL).parent.mkdir(parents=True, exist_ok=True)
    (root / CACHE_REL).write_text(json.dumps({"fp": fp, "faces": faces}, sort_keys=True),
                                  encoding="utf-8")


def face_gates(root, refresh=False):
    fp = tree_fingerprint(root)
    cache = None if refresh else load_gate_cache(root)
    if cache and cache.get("fp") == fp and "gates" in cache.get("faces", {}):
        f = cache["faces"]["gates"]
        return mk_face("gates", f["status"], f["evidence"],
                       f["detail"] + " [cached fp=%s --refresh to re-verify]" % fp[:8])
    results, missing = [], []
    for name, rel in GATE_SCRIPTS:
        script = root / rel
        if not script.is_file():
            missing.append(name)
            continue
        try:
            p = subprocess.run([sys.executable, str(script)], cwd=str(root),
                               capture_output=True, timeout=120)
            results.append((name, p.returncode))
        except (OSError, subprocess.SubprocessError):
            results.append((name, -1))
    if not results:
        return mk_face("gates", "UNAVAILABLE", "scripts/privacy_scan.py, scripts/docs_hygiene.py",
                       "no gate script present at this root")
    ev = "; ".join("%s exit=%d" % (n, c) for n, c in results)
    detail = ev
    if missing:
        detail += "; missing: " + ",".join(missing)
    failed = [n for n, c in results if c != 0]
    if failed:
        rerun = "privacy_scan.py" if failed[0] == "privacy" else "docs_hygiene.py"
        detail += "; rerun: python scripts/" + rerun
        face = mk_face("gates", "FAIL", ev, detail)
    else:
        face = mk_face("gates", "OK", ev, detail)
    save_gate_cache(root, fp, {"gates": {"status": face["status"], "evidence": face["evidence"],
                                         "detail": detail}})
    return face


def safe(builder, *args):
    try:
        return builder(*args)
    except Exception as e:  # a face must never take the whole report down
        return mk_face("?", "UNAVAILABLE", "", "face crashed: %s: %.80s" % (type(e).__name__, e))


def main(argv):
    ap = argparse.ArgumentParser(prog="doctor", add_help=True,
                                 description="project health snapshot (observation, not a gate)")
    ap.add_argument("--root", default="", help="project root (default: this repo)")
    ap.add_argument("--refresh", action="store_true",
                    help="re-run gates even when the tree fingerprint is unchanged")
    ap.add_argument("--json", action="store_true", help="one ASCII JSON line")
    a = ap.parse_args(argv)
    root = Path(a.root) if a.root else Path(__file__).resolve().parents[1]
    if not root.is_dir():
        print("doctor: --root is not a directory: %s" % a.root)
        return 2

    entries = d_rows = None
    ledger_path = root / "docs" / "ERRORS.md"
    if ledger_path.is_file():
        try:
            entries, d_rows = parse_ledger(read_text(ledger_path))
        except Exception:
            entries = d_rows = None
    d_ids = [lid for lid, _ in (d_rows or [])]

    selftest_text = None
    st_path = root / "tools" / "selftest.ps1"
    if st_path.is_file():
        try:
            selftest_text = read_text(st_path)
        except Exception:
            selftest_text = None

    registry_ids = None
    registry_doc = None
    reg_path = root / ".agent" / "checks" / "registry.json"
    if reg_path.is_file():
        try:
            doc = json.loads(read_text(reg_path))
            checks = doc.get("checks") if isinstance(doc, dict) else doc
            # both shapes are accepted: the real registry is a dict with "checks",
            # a bare list of check objects is tolerated; anything else stays unread.
            if isinstance(checks, list) and all(isinstance(c, dict) for c in checks):
                registry_doc = {"checks": checks}
                registry_ids = {c.get("id") for c in checks}
        except (OSError, ValueError, TypeError):
            registry_doc = None
            registry_ids = None

    faces = [
        safe(face_selftest, root),
        safe(face_ledger, entries, d_ids, selftest_text, registry_ids),
        safe(face_registry, root, registry_doc),
        safe(face_backlog, entries, d_rows),
        safe(face_git, root),
        safe(face_gates, root, bool(a.refresh)),
    ]
    for f in faces:
        if f["face"] == "?":
            f["face"] = "unknown"
    at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    if a.json:
        print(json.dumps({"tool": "doctor", "at": at, "root": a.root or ".",
                          "faces": faces}, ensure_ascii=True))
        return 0
    counts = {s: 0 for s in ("OK", "WARN", "FAIL", "UNAVAILABLE")}
    for f in faces:
        counts[f["status"]] = counts.get(f["status"], 0) + 1
    print("doctor: project health snapshot at=%s (observation, not a gate)" % at)
    print("face      status       evidence")
    print("--------  -----------  ---------------------------------------------------------")
    for f in faces:
        print("%-9s %-11s %s" % (f["face"], f["status"], f["evidence"]))
    print("summary: faces=%d ok=%d warn=%d fail=%d unavail=%d" % (
        len(faces), counts["OK"], counts["WARN"], counts["FAIL"], counts["UNAVAILABLE"]))
    for f in faces:
        if f["status"] != "OK":
            print("  [%s] %s" % (f["status"], f["face"]))
            print("    %s" % f["detail"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
