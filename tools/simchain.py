#!/usr/bin/env python3
"""simchain: black-box deterministic simulation testing for the whole agent_verify chain.

    simchain.py simulate --seed 0 --faults kill,corrupt,concurrent,truncate --runs 50
    simchain.py simulate --seed 9000 --faults corrupt --runs 4 --report <path> --keep

WHY (the industry lineage, stated as lineage not as authority)
  The industry benchmark for deterministic simulation testing is FoundationDB
  (SIGMOD 2021: every new feature ran under a simulated storm of fault combinations
  before it ever shipped, and passed Jepsen afterwards; a single-process seeded
  fault-injection harness made every bug reproducible from its seed). WarpStream
  later applied the same discipline to an entire running SaaS. This tool imports the
  discipline, not the code: seed the faults, run the REAL tool chain as a black box
  in a disposable fixture, assert global invariants after every scenario.

BLACK-BOX DISCIPLINE (the whole point)
  The simulator never patches, imports or monkey-patches the tools under test. Every
  scenario drives the real binaries from outside -- agent_verify.py (the full gate
  chain), attest.py, ledger.py, journal.py, resume.py, trash.py -- as subprocesses in
  a throwaway fixture repo, then asserts invariants on the artifacts the chain left
  behind. agent_verify.py / mre.py / policy.py are read-only to this tool by design:
  if a simulation finds a bug, the fix lands in the tool, pinned by a selftest
  assertion, and the seed that found it stays reproducible.

SEED SPACE (how many faults exist and where determinism comes from)
  A scenario is one (seed, family) pair. Every decision inside a scenario -- which
  kill hook, which store to corrupt, which corruption mode, which byte offset, which
  truncation fraction -- is drawn from ONE private random.Random(f"simchain:{seed}:
  {family}") stream, seeded from the string (sha512-based seeding, stable across
  processes and CPython versions). Scenarios share no state, so the default sweep
  (--runs 50 over the four families) explores 200 points of a seed space that is
  reproducible forever: same seed + same family = same fault schedule = same
  assertions = same report, byte for byte (modulo the top-level wall-clock fields).

DETERMINISM MECHANICS (honest scope)
  Fault SCHEDULES are fully seed-determined. Kill faults fire on durable artifacts
  (chain.lock appearing / the journal manifest appearing / a run-command marker file
  appearing) whose observation windows are wide (0.3s+) compared with kill latency,
  so the logical kill point is stable. Scenario REPORTS record only race-independent
  classifications (booleans and seed-derived choices) -- never wall-clock values --
  so two runs of the same seed produce identical scenario arrays. What is honestly
  NOT deterministic: which of two concurrent chains wins (recorded as one-winner,
  never which), and the wall-clock duration fields (top-level only, excluded from
  comparisons).

FAULT FAMILIES
  kill        a real agent_verify chain is killed at a seed-chosen point (after the
              chain lock appears / after the journal snapshot appears / after run
              command k of 6). Recovery path follows the AF-group mechanism: the
              leftover chain.lock must be diagnosed by the next run, the snapshot
              must be restorable, and the tree must accept a fresh run afterwards.
  corrupt     one store (journal manifest / attest chain / cost ledger / resume
              session) is corrupted in one mode (bad JSON overwrite / half-line or
              mid-document truncation / byte flip). The invariant: every reader
              either proceeds on verified data or refuses LOUDLY -- never a silent
              bad book, never a traceback where a refusal belongs.
  concurrent  two chains race the same root; exactly one wins, the loser is refused
              by chain.lock with a receipt, and the winner's stores verify clean.
  truncate    a ledger file (attest chain / cost ledger) is cut at a seed-chosen
              byte offset (power-loss model). Load #1 must quarantine the bad tail
              into the recycle bin and continue on the intact prefix (or refuse
              cleanly when no intact prefix exists); load #2 must return exactly
              what load #1 returned; the recovered chain must be a byte-honest
              prefix of the original; appends must work again afterwards.

GLOBAL INVARIANTS (asserted after EVERY scenario)
  1. attest verify passes, or refuses explicitly -- never silently blesses a broken
     chain (a bit flip that changes any parsed field must be caught).
  2. journal snapshot is restorable, or its loss is reported explicitly (exit 2).
  3. no half receipts: a receipt file either parses as complete JSON or does not
     exist; a half-written receipt is a violation.
  4. chain.lock is recoverable: while present, the next run refuses; after clearing
     it the snapshot restores and the root accepts a fresh run.
  5. resume sessions have no half state: a session file loads fully or refuses
     cleanly (exit 2), and begin refuses to overwrite a corrupt session.
  6. the evidence chain carries no fabricated bookkeeping: whatever the recovery
     scan leaves behind is exactly a PREFIX of the pre-fault entries (bytes either
     kept in place or quarantined whole into the recycle bin), and a receipt that
     claims changes can have those changes re-verified by re-running journal end.
  Plus, everywhere: a reader crash (Python traceback) where a clean refusal belongs
  is itself a violation.

RECOVERY SCAN vs VERIFY (the division of labor)
  The recovery scans in attest.py / ledger.py repair PHYSICS ONLY: a physically
  truncated tail (an unparseable last JSONL line without its newline; a JSON
  document missing its closing structure) is quarantined whole into the recycle bin
  (<store>/trash via trash.put_paths, never raw-deleted) and the file is truncated
  to its intact prefix. LOGIC stays fail-closed: any corruption a hash chain can see
  (mid-file flips, altered payloads, broken linkage) still refuses through
  verify_chain / the budget gate, and stores with no integrity chain at all (the
  cost ledger's entry VALUES, resume session contents) document that boundary here:
  a value flip that keeps their shape parseable is undetectable by design -- the
  tamper-evident book of record is the attest chain, which the budget gate consumes
  whenever it exists.

DURABILITY BOUNDARY (Windows, stated not hidden)
  attest.py appends and ledger.py saves flush + os.fsync their file handles.
  os.fsync on Windows is FlushFileBuffers on the handle -- the closest stdlib
  equivalent of fsync(2). A directory-level fsync does not exist in the Windows
  stdlib surface (no openable directory fd), so name-entry durability after a real
  power cut is a stated boundary. Process kills lose no OS-buffered writes, so
  in-simulator kill scenarios do exercise "a confirmed entry survives the crash".

CONSUMER INTERFACE (doctor：Rust `trit-doctor` 主入口，Python `tools/doctor.py` reference)
  Every simulate run writes the report JSON (default <cwd>/.agent/last-simchain.json,
  override with --report) carrying at least: seeds, faults, scenarios, invariants_ok,
  violations, duration_s. Both doctor implementations read this file as-is for the
  simchain face (the selftest face reads .agent/last-selftest.json the same way).
  Machine-local state: keep the path gitignored, like last-selftest.json.

EXIT  simulate: 0 every scenario held its invariants; 1 at least one violation was
      recorded (the report names them); 2 bad invocation. Output is pure ASCII.
"""

import argparse
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
AGENT_VERIFY = os.path.join(TOOLS, "agent_verify.py")
ATTEST = os.path.join(TOOLS, "attest.py")
LEDGER = os.path.join(TOOLS, "ledger.py")
JOURNAL = os.path.join(TOOLS, "journal.py")
RESUME = os.path.join(TOOLS, "resume.py")

FAMILIES = ("kill", "corrupt", "concurrent", "truncate")
DEFAULT_SEEDS = 50
CORRUPT_STORES = ("journal-manifest", "attest-chain", "cost-ledger", "resume-session")
CORRUPT_MODES = ("bad-json", "half-line", "bit-flip")
KILL_HOOKS = ("lock", "begin", "run-1", "run-2", "run-3", "run-4", "run-5", "run-6")
TASK = "sim"

RECOVERED_RX = re.compile(r"recovered: truncated (\d+) bytes \(quarantined at ([^)]+)\)")
CHANGE_RX = re.compile(r"journal:\s+(added|deleted|modified)\s+(.+)")

ASCII_TABLE_HEAD = (f"{'family':<12}{'scenarios':>10}{'invariants_ok':>15}"
                    f"{'violations':>12}{'recovered':>11}{'refused':>9}")


def rng_for(seed, family):
    """One private stream per scenario; string seeding is sha512-based and stable."""
    return random.Random(f"simchain:{seed}:{family}")


def ascii_of(text):
    return str(text).encode("ascii", "replace").decode("ascii")


def tool(args, timeout=120):
    """Run one real tool binary, black box. Returns (exit, stdout, stderr)."""
    try:
        p = subprocess.run([PY] + list(args), capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
        return p.returncode, p.stdout or "", p.stderr or ""
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"


def combined(res):
    return res[1] + res[2]


def has_traceback(res):
    return "Traceback (most recent call last)" in res[2]


# ---------------------------------------------------------------- fixture building

def write_file(path, content):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(content)


def build_fixture(base):
    """The disposable project the chain runs against: one python package, one fast
    plan, one kill plan (markers spaced so kill hooks have wide observation windows),
    a git repo (preflight/restore expect one), and a pre-begun resume session."""
    root = os.path.join(base, "root")
    store = os.path.join(base, "store")
    scen = {"base": base, "root": root, "store": store}
    write_file(os.path.join(root, "pkg", "core.py"), "def f(): return 1\n")
    write_file(os.path.join(root, "pkg", "__init__.py"), "")
    write_file(os.path.join(root, "tests", "test_core.py"),
               "from pkg.core import f\ndef test_f(): assert f() == 1\n")
    write_file(os.path.join(root, "conftest.py"), "")
    # fast plan: one quick command, the whole chain finishes in well under a second
    write_file(os.path.join(root, "plan-fast.json"),
               '{"proven":true,"commands":["echo done > done.txt"],"unproven":[]}')
    # kill plan: 6 marker commands each preceded by a sleep -- the run segment lasts
    # seconds and every marker file is a durable, ordered kill position
    cmds = []
    for k in range(1, 7):
        cmds += ["sleep 0.25", f"echo {k} > run-{k}.txt"]
    write_file(os.path.join(root, "plan-kill.json"),
               json.dumps({"proven": True, "commands": cmds, "unproven": []}))
    scen["plan_fast"] = os.path.join(root, "plan-fast.json")
    scen["plan_kill"] = os.path.join(root, "plan-kill.json")
    g = subprocess.run(["git", "init", "-q"], cwd=root, capture_output=True, text=True)
    if g.returncode != 0:
        raise RuntimeError(f"git init failed: {g.stderr.strip()[:120]}")
    subprocess.run(["git", "add", "-A"], cwd=root, capture_output=True, text=True)
    subprocess.run(["git", "-c", "user.email=a@b", "-c", "user.name=t", "commit", "-qm", "init"],
                   cwd=root, capture_output=True, text=True)
    r = tool([RESUME, "begin", TASK, "-Root", root])
    if r[0] != 0:
        raise RuntimeError(f"resume begin failed: {combined(r).strip()[:120]}")
    scen["session"] = os.path.join(root, ".agent", "resume", f"{TASK}.json")
    scen["manifest"] = os.path.join(store, "latest", "manifest.json")
    scen["attest_path"] = os.path.join(store, "attest-ledger.jsonl")
    scen["ledger_path"] = os.path.join(store, "cost-ledger.json")
    scen["lock"] = os.path.join(root, ".journal", "chain.lock")
    return scen


def chain(scen, receipt_name, plan, extra=(), wait=True):
    """One full agent_verify chain, black box, with every store populated:
    --spent-token lands on the attest chain, the run lands on the cost ledger,
    the snapshot lands in the journal store, the session sits under -Root."""
    args = [AGENT_VERIFY, "-Root", scen["root"], "-Changed",
            os.path.join(scen["root"], "pkg", "core.py"), "-Lang", "python",
            "-Plan", plan, "-Store", scen["store"],
            "-Receipt", os.path.join(scen["base"], receipt_name),
            "--task-id", TASK, "--spent-token", "4200"] + list(extra)
    if not wait:
        return subprocess.Popen([PY] + args, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True,
                                encoding="utf-8", errors="replace")
    return tool(args, timeout=180)


# ---------------------------------------------------------------- shared batteries

def load_json(path):
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def attest_entries_of(path):
    doc = []
    try:
        with open(path, "rb") as fh:
            raw = fh.read()
    except OSError:
        return None
    for piece in raw.split(b"\n"):
        if not piece.strip():
            continue
        try:
            doc.append(json.loads(piece.decode("utf-8-sig")))
        except (ValueError, UnicodeDecodeError):
            return None
    return doc


def check_receipt(scen, name, violations):
    """Invariant 3: a receipt either parses complete or does not exist."""
    path = os.path.join(scen["base"], name)
    if not os.path.exists(path):
        return "absent"
    if load_json(path) is None:
        violations.append(f"receipt-half-written:{name}")
        return "partial"
    return "complete"


def check_attest(scen, violations):
    if not os.path.exists(scen["attest_path"]):
        return {"exit": "absent"}
    r = tool([ATTEST, "verify", "--store", scen["store"]])
    if has_traceback(r):
        violations.append("attest-verify-traceback")
    if r[0] not in (0, 1):
        violations.append(f"attest-verify-exit:{r[0]}")
    return {"exit": r[0], "recovered": "recovered:" in combined(r),
            "out": combined(r)}


def check_journal(scen, violations):
    r = tool([JOURNAL, "-Mode", "status", "-Root", scen["root"], "-Store", scen["store"]])
    text = combined(r)
    if has_traceback(r):
        violations.append("journal-status-traceback")
    # exit 1 is the L124 contract for "manifest parses but entries are damaged",
    # provided the output names the loss. 0 = healthy, 2 = clean refusal (bad JSON /
    # missing manifest). A bare 1 without "damaged" is still the old silent lie.
    loss = r[0] == 1 and "damaged" in text.lower()
    if r[0] not in (0, 2) and not loss:
        violations.append(f"journal-status-exit:{r[0]}")
    return {"exit": r[0], "out": text, "loss_reported": loss}


def check_ledger(scen, violations):
    if not os.path.exists(scen["ledger_path"]):
        return {"exit": "absent"}
    r = tool([LEDGER, "-Mode", "report", "-Ledger", scen["ledger_path"]])
    if has_traceback(r):
        violations.append("ledger-report-traceback")
    if r[0] not in (0, 2):
        violations.append(f"ledger-report-exit:{r[0]}")
    return {"exit": r[0], "recovered": "recovered:" in combined(r),
            "out": combined(r)}


def check_resume(scen, violations):
    r = tool([RESUME, "status", TASK, "-Root", scen["root"]])
    if has_traceback(r):
        violations.append("resume-status-traceback")
    if r[0] not in (0, 2):
        violations.append(f"resume-status-exit:{r[0]}")
    return {"exit": r[0], "out": combined(r)}


def check_changes(scen, receipt_name, violations):
    """Invariant 6: a receipt claiming changes must re-verify by re-running journal
    end on the untouched tree. Skipped (recorded 'unverifiable') when the snapshot
    is lost -- the explicit loss report covers it."""
    doc = load_json(os.path.join(scen["base"], receipt_name))
    j = check_journal(scen, violations)
    if doc is None or j["exit"] != 0:
        return "unverifiable"
    claimed = sorted(f"{m.group(1)} {m.group(2).strip()}"
                     for ln in (doc.get("changes") or [])
                     for m in [CHANGE_RX.search(ln)] if m)
    r = tool([JOURNAL, "-Mode", "end", "-Root", scen["root"], "-Store", scen["store"]])
    if has_traceback(r):
        violations.append("journal-recheck-traceback")
        return "violated"
    seen = sorted(f"{m.group(1)} {m.group(2).strip()}"
                  for ln in combined(r).splitlines()
                  for m in [CHANGE_RX.search(ln)] if m)
    if claimed != seen:
        violations.append(f"changes-unverifiable:{len(claimed)}-vs-{len(seen)}")
        return "violated"
    return "verified"


def check_lock_recovery(scen, violations, plan):
    """Invariant 4: while the lock is present the next run refuses; after clearing
    it the snapshot restores (or is honestly absent) and a fresh run passes."""
    if not os.path.exists(scen["lock"]):
        return {"lock_left": False}
    r = chain(scen, "rc-refused.json", plan)
    refused = r[0] == 1 and "chain.lock" in combined(r)
    if not refused:
        violations.append(f"lock-not-diagnosed:exit-{r[0]}")
    try:
        os.remove(scen["lock"])
    except OSError as ex:
        violations.append(f"lock-unremovable:{ex.__class__.__name__}")
        return {"lock_left": True, "next_refused": refused}
    rs = tool([JOURNAL, "-Mode", "restore", "-Root", scen["root"], "-Store", scen["store"]])
    if has_traceback(rs):
        violations.append("restore-traceback")
    snap = os.path.exists(scen["manifest"])
    if not (rs[0] == 0 or (rs[0] == 2 and not snap)):
        violations.append(f"restore-exit:{rs[0]}-snapshot-{snap}")
    rr = chain(scen, "rc-rerun.json", plan)
    if rr[0] != 0:
        violations.append(f"rerun-failed:exit-{rr[0]}")
    return {"lock_left": True, "next_refused": refused,
            "restore": "ok" if rs[0] == 0 else f"exit-{rs[0]}",
            "rerun": "ok" if rr[0] == 0 else f"exit-{rr[0]}"}


def quarantine_entries(scen):
    m = load_json(os.path.join(scen["store"], "trash", "manifest.json")) or {}
    out = []
    for e in m.get("entries", []):
        if "tail-recovery" in str(e.get("reason", "")):
            blob = os.path.join(scen["store"], "trash",
                                str(e.get("blob", "")).replace("/", os.sep))
            out.append({"id": e.get("id"), "bytes": e.get("bytes"),
                        "blob_exists": os.path.exists(blob)})
    return out


# ---------------------------------------------------------------- family: kill

def wait_for_artifact(proc, path, stabilize):
    """Watch for a durable artifact, then (optionally) wait until its size stops
    moving so the kill lands after the write, never inside it. Deadline is generous
    against slow machines; the artifact itself is the deterministic trigger."""
    t0 = time.time()
    while time.time() - t0 < 90:
        if os.path.exists(path):
            if not stabilize:
                return True
            last = -1
            for _ in range(25):
                time.sleep(0.05)
                if proc.poll() is not None:
                    return True
                try:
                    sz = os.path.getsize(path)
                except OSError:
                    continue
                if sz == last and sz > 0:
                    return True
                last = sz
            return True
        if proc.poll() is not None:
            return False
        time.sleep(0.01)
    return False


def kill_hook_path(scen, hook):
    if hook == "lock":
        return scen["lock"], False
    if hook == "begin":
        return scen["manifest"], True
    k = hook.split("-")[1]
    return os.path.join(scen["root"], f"run-{k}.txt"), True


def scen_kill(seed, base, violations):
    scen = build_fixture(base)
    rng = rng_for(seed, "kill")
    hook = rng.choice(KILL_HOOKS)
    path, stabilize = kill_hook_path(scen, hook)
    proc = chain(scen, "rc-kill.json", scen["plan_kill"], wait=False)
    seen = wait_for_artifact(proc, path, stabilize)
    if seen and proc.poll() is None:
        proc.kill()
    proc.wait(timeout=30)
    receipt = check_receipt(scen, "rc-kill.json", violations)
    a = check_attest(scen, violations)
    j = check_journal(scen, violations)
    d = check_ledger(scen, violations)
    check_resume(scen, violations)
    lock = check_lock_recovery(scen, violations, scen["plan_fast"])
    outcome = {"hook": hook, "artifact_seen": seen, "lock": lock,
               "snapshot": "present" if os.path.exists(scen["manifest"]) else "absent",
               "receipt": receipt,
               "attest": a["exit"], "ledger": d["exit"], "journal": j["exit"]}
    return outcome


# ---------------------------------------------------------------- family: corrupt

def flip_byte(data, pos):
    delta = (1, 2, 4, 8, 16, 32, 64)[data[pos] % 7]
    return data[:pos] + bytes([data[pos] ^ delta]) + data[pos + 1:]


def structural_positions(data):
    return [i for i, b in enumerate(data) if b in b'{}[]:,"']


def inject_corrupt(scen, target, mode, rng):
    """Apply the seeded corruption; return the fault descriptor. For the cost ledger's
    byte flip the flip targets structural bytes and walks them in a seeded order until
    the document actually breaks or a parsed value actually changes: a flip that is a
    parsed no-op (JSON whitespace) tests nothing, and this ledger has no hash chain to
    catch the rest -- the boundary the module docstring states."""
    paths = {"journal-manifest": scen["manifest"], "attest-chain": scen["attest_path"],
             "cost-ledger": scen["ledger_path"], "resume-session": scen["session"]}
    path = paths[target]
    raw = open(path, "rb").read()
    if mode == "bad-json":
        open(path, "wb").write(b"{bad json\n")
        return {"store": target, "mode": mode, "note": "whole-file overwrite"}
    if mode == "half-line":
        if target == "attest-chain":
            open(path, "ab").write(b'{"seq": 9, "payl')
            return {"store": target, "mode": mode, "note": "partial jsonl tail, no newline"}
        cut = max(1, int(len(raw) * 0.55))
        open(path, "wb").write(raw[:cut])
        # dimensionless note: the absolute byte derives from file size, and file size
        # carries wall-clock digits (ledger wall_s) -- recording it would leak wall
        # noise into a report that must be byte-identical across same-seed runs
        return {"store": target, "mode": mode, "note": "truncated at 55% of bytes"}
    # bit-flip
    if target == "cost-ledger":
        pos_list = structural_positions(raw)
        rng.shuffle(pos_list)
        for pos in pos_list:
            trial = flip_byte(raw, pos)
            try:
                doc = json.loads(trial.decode("utf-8-sig"))
                entries = doc.get("entries") if isinstance(doc, dict) else None
                old = json.loads(raw.decode("utf-8-sig")).get("entries")
                if entries == old:
                    continue  # parsed no-op: try the next structural byte
                open(path, "wb").write(trial)
                return {"store": target, "mode": "bit-flip", "note": "value flip, still valid json"}
            except (ValueError, UnicodeDecodeError):
                open(path, "wb").write(trial)
                return {"store": target, "mode": "bit-flip", "note": "structural flip, breaks json"}
        return {"store": target, "mode": "bit-flip", "note": "no effective structural byte"}
    pos = rng.randrange(len(raw))
    open(path, "wb").write(flip_byte(raw, pos))
    return {"store": target, "mode": mode, "note": "byte flip applied (position in fault log only)"}


def scen_corrupt(seed, base, violations):
    scen = build_fixture(base)
    rng = rng_for(seed, "corrupt")
    r = chain(scen, "rc-corrupt.json", scen["plan_fast"])
    if r[0] != 0:
        violations.append(f"setup-chain-failed:exit-{r[0]}")
        return {"store": "-", "mode": "-", "chain": "failed"}
    receipt = check_receipt(scen, "rc-corrupt.json", violations)
    # changes re-verification runs BEFORE the fault injection: the receipt's claims
    # were made against the pre-fault tree, and a resume-session corruption would
    # legitimately change the tree under -Root after this point
    check_changes(scen, "rc-corrupt.json", violations)
    pre_attest = attest_entries_of(scen["attest_path"]) or []
    pre_ledger = (load_json(scen["ledger_path"]) or {}).get("entries", [])
    fault = inject_corrupt(scen, rng.choice(CORRUPT_STORES), rng.choice(CORRUPT_MODES), rng)
    a = check_attest(scen, violations)
    j = check_journal(scen, violations)
    d = check_ledger(scen, violations)
    res = check_resume(scen, violations)
    lock = check_lock_recovery(scen, violations, scen["plan_fast"])
    if lock.get("lock_left"):
        violations.append("lock-left-after-clean-chain")

    # per-store expectations (fail-closed or verified-prefix, never silent)
    store, mode = fault["store"], fault["mode"]
    if store == "attest-chain":
        post = attest_entries_of(scen["attest_path"]) or []
        prefix = post == pre_attest[:len(post)]
        # prefix discipline: whatever the loader LEAVES BEHIND must be a prefix of the
        # pre-fault book. A bit flip that stays parseable is expected to CHANGE content
        # (that is what the fault is) -- the invariant there is an explicit refusal, so
        # prefix only binds when verify passed or the recovery scan repaired the tail.
        if not prefix and a.get("exit") == 0:
            violations.append("attest-prefix-broken")
        if a.get("recovered") and not prefix:
            violations.append("recovery-fabricated-entries")
        if mode == "bad-json" and a.get("exit") == 0:
            violations.append("attest-silent-bad-book")
        if mode == "bit-flip" and a.get("exit") == 0 and not a.get("recovered"):
            # the only legitimate pass-through is a flip that changed no parsed field
            if post != pre_attest:
                violations.append("attest-silent-bad-book")
        if mode == "half-line" and not a.get("recovered"):
            violations.append("attest-tail-not-recovered")
        if a.get("recovered") and not quarantine_entries(scen):
            violations.append("attest-recovery-without-quarantine")
        outcome_attest = {"exit": a.get("exit"), "recovered": a.get("recovered", False),
                          "prefix_ok": prefix}
    else:
        outcome_attest = {"exit": a.get("exit"), "recovered": a.get("recovered", False)}

    if store == "cost-ledger":
        post_doc = load_json(scen["ledger_path"])
        post = (post_doc or {}).get("entries", [])
        prefix = post == pre_ledger[:len(post)]
        gap = None
        if post and not prefix and mode == "bit-flip":
            gap = "value-flip-undetected"  # documented boundary: no hash chain on this book
        if not prefix and gap is None:
            violations.append("ledger-prefix-broken")
        if mode == "bad-json" and d.get("exit") == 0:
            violations.append("ledger-silent-bad-book")
        if mode == "half-line" and d.get("exit") == 0 and not d.get("recovered"):
            violations.append("ledger-tail-not-recovered")
        if d.get("recovered") and not quarantine_entries(scen):
            violations.append("ledger-recovery-without-quarantine")
        outcome_ledger = {"exit": d.get("exit"), "recovered": d.get("recovered", False),
                          "prefix_ok": prefix, "gap": gap}
    else:
        outcome_ledger = {"exit": d.get("exit"), "recovered": d.get("recovered", False)}

    if store == "journal-manifest":
        if j["exit"] == 0:
            rr = tool([JOURNAL, "-Mode", "restore", "-Root", scen["root"],
                       "-Store", scen["store"]])
            if has_traceback(rr):
                violations.append("restore-traceback")
            if rr[0] != 0:
                violations.append(f"manifest-claims-ok-but-restore-exit:{rr[0]}")
            outcome_journal = {"exit": 0, "restore": rr[0]}
        else:
            outcome_journal = {"exit": j["exit"], "loss_reported": True}
    else:
        outcome_journal = {"exit": j["exit"]}

    if store == "resume-session":
        if res["exit"] == 2:
            rb = tool([RESUME, "begin", TASK, "-Root", scen["root"]])
            if rb[0] != 2 or has_traceback(rb):
                violations.append(f"resume-begin-overwrote-corrupt:{rb[0]}")
        outcome_resume = {"exit": res["exit"]}
    else:
        outcome_resume = {"exit": res["exit"]}

    return {"store": store, "mode": mode, "note": fault["note"], "receipt": receipt,
            "attest": outcome_attest, "ledger": outcome_ledger,
            "journal": outcome_journal, "resume": outcome_resume}


# ---------------------------------------------------------------- family: concurrent

def scen_concurrent(seed, base, violations):
    scen = build_fixture(base)
    procs = {}
    def launch(idx):
        procs[idx] = chain(scen, f"rc-conc-{idx}.json", scen["plan_fast"], wait=False)
    t1 = threading.Thread(target=launch, args=(1,))
    t2 = threading.Thread(target=launch, args=(2,))
    t1.start(); t2.start(); t1.join(); t2.join()
    outs = {}
    for idx, p in procs.items():
        o, e = p.communicate(timeout=180)
        outs[idx] = (p.returncode, o or "", e or "")
    codes = sorted(idx for idx, r in outs.items() if r[0] == 0)
    if len(codes) != 1:
        violations.append(f"concurrent-winners:{[r[0] for r in outs.values()]}")
    loser = next((idx for idx, r in outs.items() if r[0] != 0), None)
    winner = next((idx for idx, r in outs.items() if r[0] == 0), None)
    loser_refused = False
    if loser is not None:
        # the refusal names the dirty-state evidence on stdout; the verdict string
        # lives in the loser's receipt (agent_verify emits one even on refusal)
        ldoc = load_json(os.path.join(scen["base"], f"rc-conc-{loser}.json"))
        loser_refused = ("chain.lock" in combined(outs[loser])
                         and ldoc is not None
                         and ldoc.get("verdict") == "refused by chain lock")
        if not loser_refused:
            violations.append(f"loser-not-lock-refused:{outs[loser][0]}")
    wdoc = load_json(os.path.join(scen["base"], f"rc-conc-{winner}.json")) if winner else None
    if wdoc is None:
        violations.append("winner-receipt-missing-or-partial")
    if has_traceback(outs[1]) or has_traceback(outs[2]):
        violations.append("concurrent-traceback")
    a = check_attest(scen, violations)
    d = check_ledger(scen, violations)
    j = check_journal(scen, violations)
    check_resume(scen, violations)
    if os.path.exists(scen["lock"]):
        violations.append("lock-left-after-concurrent")
    if j["exit"] != 0:
        violations.append(f"concurrent-snapshot-lost:{j['exit']}")
    return {"one_winner": len(codes) == 1, "loser_refused": loser_refused,
            "winner_receipt": wdoc is not None, "attest": a["exit"],
            "ledger": d["exit"], "journal": j["exit"]}


# ---------------------------------------------------------------- family: truncate

def scen_truncate(seed, base, violations):
    scen = build_fixture(base)
    rng = rng_for(seed, "truncate")
    r = chain(scen, "rc-trunc.json", scen["plan_fast"])
    if r[0] != 0:
        violations.append(f"setup-chain-failed:exit-{r[0]}")
        return {"file": "-", "loads_equal": False}
    check_receipt(scen, "rc-trunc.json", violations)
    target = rng.choice(("attest-chain", "cost-ledger"))
    if target == "attest-chain":
        path = scen["attest_path"]
        for i in range(3):
            ra = tool([ATTEST, "append", "--store", scen["store"], "--kind", "spend",
                       "--task-id", TASK, "--runid", f"trunc-seed-{seed}-{i}",
                       "--claimed-tokens", str(10 + i)])
            if ra[0] != 0:
                violations.append(f"setup-attest-append-failed:{ra[0]}")
    else:
        path = scen["ledger_path"]
        for i in range(3):
            rl = tool([LEDGER, "-Mode", "add", "-Ledger", path, "-Task", "sim-extra",
                       "-Key", f"trunc-seed-{seed}-{i}", "-Wall", "1.5"])
            if rl[0] != 0:
                violations.append(f"setup-ledger-add-failed:{rl[0]}")
    raw = open(path, "rb").read()
    pre_attest = attest_entries_of(path) or []
    pre_ledger = (load_json(path) or {}).get("entries", [])
    frac = round(rng.uniform(0.05, 0.95), 3)
    offset = int(len(raw) * frac)
    open(path, "wb").write(raw[:offset])

    if target == "attest-chain":
        r1 = tool([ATTEST, "verify", "--store", scen["store"]])
        out1 = combined(r1)
        if has_traceback(r1):
            violations.append("attest-verify-traceback")
        if r1[0] not in (0, 1):
            violations.append(f"attest-verify-exit:{r1[0]}")
        post = attest_entries_of(path) or []
        prefix = post == pre_attest[:len(post)]
        if not prefix:
            violations.append("attest-prefix-broken")
        r2 = tool([ATTEST, "verify", "--store", scen["store"]])
        loads_equal = (load_result_lines(combined(r2)) == load_result_lines(out1)
                       and r2[0] == r1[0]
                       and "recovered:" not in combined(r2))
        size_after_load = os.path.getsize(path)  # append-after rewrites the file below
        outcome = {"file": target, "frac": frac, "load1_exit": r1[0],
                   "recovered": "recovered:" in out1, "prefix_ok": prefix,
                   "loads_equal": loads_equal, "append_after": None}
        m = RECOVERED_RX.search(out1)
        if r1[0] == 0:
            # a book that refused its tail must still accept new, verified entries
            ra = tool([ATTEST, "append", "--store", scen["store"], "--kind", "spend",
                       "--task-id", TASK, "--runid", f"after-recovery-{seed}",
                       "--claimed-tokens", "77"])
            if ra[0] != 0:
                violations.append(f"append-after-recovery-refused:{ra[0]}")
            rv = tool([ATTEST, "verify", "--store", scen["store"]])
            if rv[0] != 0:
                violations.append(f"verify-after-append:{rv[0]}")
            outcome["append_after"] = ra[0] == 0
    else:
        r1 = tool([LEDGER, "-Mode", "report", "-Ledger", path])
        out1 = combined(r1)
        if has_traceback(r1):
            violations.append("ledger-report-traceback")
        if r1[0] not in (0, 2):
            violations.append(f"ledger-report-exit:{r1[0]}")
        post_doc = load_json(path)
        post = (post_doc or {}).get("entries", [])
        prefix = post == pre_ledger[:len(post)]
        if not prefix:
            violations.append("ledger-prefix-broken")
        r2 = tool([LEDGER, "-Mode", "report", "-Ledger", path])
        loads_equal = (load_result_lines(combined(r2)) == load_result_lines(out1)
                       and r2[0] == r1[0]
                       and "recovered:" not in combined(r2))
        size_after_load = os.path.getsize(path)
        outcome = {"file": target, "frac": frac, "load1_exit": r1[0],
                   "recovered": "recovered:" in out1, "prefix_ok": prefix,
                   "loads_equal": loads_equal, "append_after": None}
        m = RECOVERED_RX.search(out1)
        if r1[0] == 0:
            rl = tool([LEDGER, "-Mode", "add", "-Ledger", path, "-Task", "after-recovery",
                       "-Key", f"after-{seed}", "-Wall", "1.0"])
            if rl[0] != 0:
                violations.append(f"add-after-recovery-refused:{rl[0]}")
            outcome["append_after"] = rl[0] == 0

    if m:
        n = int(m.group(1))
        q = quarantine_entries(scen)
        if not q:
            violations.append("recovery-without-quarantine")
        else:
            if q[-1]["bytes"] != n:
                violations.append(f"quarantine-bytes-mismatch:{q[-1]['bytes']}-vs-{n}")
            if not q[-1]["blob_exists"]:
                violations.append("quarantine-blob-missing")
        kept = size_after_load - (5 if (target == "cost-ledger") else 0)
        # byte honesty: what the loader SAW (the truncated file, offset bytes) is fully
        # accounted for -- kept on disk plus quarantined whole. The bytes the simulated
        # power cut destroyed before any loader ran are unrecoverable by definition;
        # the cost-ledger file carries the 5 rebuilt closing bytes (structure, not book).
        if kept + n != offset:
            violations.append(f"byte-accounting:{kept}+{n}!={offset}")
    elif not outcome["recovered"]:
        if os.path.getsize(path) != offset and outcome["load1_exit"] != 0:
            violations.append(f"file-modified-without-recovery:{os.path.getsize(path)}")
    if outcome["recovered"] and not outcome["prefix_ok"]:
        violations.append("recovery-fabricated-entries")
    return outcome


# ---------------------------------------------------------------- reporting

RUNNERS = {"kill": scen_kill, "corrupt": scen_corrupt,
           "concurrent": scen_concurrent, "truncate": scen_truncate}


def ascii_table(by_family):
    lines = [ASCII_TABLE_HEAD]
    total = {"scenarios": 0, "ok": 0, "violations": 0, "recovered": 0, "refused": 0}
    for fam in FAMILIES:
        f = by_family.get(fam, {})
        lines.append(f"{fam:<12}{f.get('scenarios', 0):>10}{f.get('ok', 0):>15}"
                     f"{f.get('violations', 0):>12}{f.get('recovered', 0):>11}"
                     f"{f.get('refused', 0):>9}")
        for k, key in (("scenarios", "scenarios"), ("ok", "ok"),
                       ("violations", "violations"), ("recovered", "recovered"),
                       ("refused", "refused")):
            total[k] += f.get(key, 0)
    lines.append(f"{'TOTAL':<12}{total['scenarios']:>10}{total['ok']:>15}"
                 f"{total['violations']:>12}{total['recovered']:>11}{total['refused']:>9}")
    return "\n".join(lines), total


def load_result_lines(text):
    """The load's verdict content with one-time evidence lines removed: the recovery
    scan quarantines and prints on the FIRST load only, so the honest comparison is
    the book the loader returns, not the repair narration."""
    out = []
    for ln in text.splitlines():
        if ln.startswith("trash:") or ln.startswith("attest: recovered:") \
                or ln.startswith("ledger: recovered:"):
            continue
        out.append(ln)
    return "\n".join(out)


def scenario_refused(outcome):
    flags = []
    for v in (outcome.get("attest"), outcome.get("ledger"), outcome.get("journal")):
        if isinstance(v, dict):
            e = v.get("exit")
            if e == 1 or e == 2:
                flags.append(e)
    if isinstance(outcome.get("resume"), dict) and outcome["resume"].get("exit") == 2:
        flags.append(2)
    return bool(flags)


def main(argv):
    ap = argparse.ArgumentParser(prog="simchain", add_help=True)
    sub = ap.add_subparsers(dest="cmd")
    sp = sub.add_parser("simulate")
    sp.add_argument("--seed", type=int, default=0,
                    help="base seed; scenario i uses seed base+i per family")
    sp.add_argument("--faults", default=",".join(FAMILIES),
                    help=f"comma list from {list(FAMILIES)}")
    sp.add_argument("--runs", type=int, default=DEFAULT_SEEDS,
                    help=f"seeds swept (default {DEFAULT_SEEDS}) x selected families")
    sp.add_argument("--report", default=os.path.join(os.getcwd(), ".agent",
                                                     "last-simchain.json"))
    sp.add_argument("--keep", action="store_true",
                    help="keep scenario fixture directories for post-mortem")
    sp.add_argument("--quiet", action="store_true", help="per-scenario lines off")
    a = ap.parse_args(argv)
    if a.cmd != "simulate":
        print("simchain: mode must be: simulate")
        return 2
    fams = [f.strip() for f in a.faults.split(",") if f.strip()]
    if not fams or any(f not in FAMILIES for f in fams):
        print(f"simchain: --faults must be a comma list from {list(FAMILIES)}")
        return 2
    if a.runs < 1:
        print("simchain: --runs must be >= 1")
        return 2

    t0 = time.time()
    scenarios = []
    for i in range(a.runs):
        seed = a.seed + i
        for fam in fams:
            base = tempfile.mkdtemp(prefix=f"simchain-{seed}-{fam}-")
            violations = []
            try:
                outcome = RUNNERS[fam](seed, base, violations)
            except Exception as ex:  # a scenario crash is data, not a sim crash
                violations.append(f"simulator-exception:{ex.__class__.__name__}")
                outcome = {"family_error": ascii_of(str(ex))[:120]}
            if not a.keep:
                shutil.rmtree(base, ignore_errors=True)
            row = {"seed": seed, "family": fam,
                   "chain_outcome": outcome,
                   "invariants_ok": not violations,
                   "violations": violations}
            scenarios.append(row)
            if not a.quiet:
                tag = f"seed {seed} {fam}"
                state = "OK" if not violations else "VIOLATIONS"
                print(f"  {tag:<34} invariants={state}"
                      + (f" violations={violations}" if violations else ""))
    duration = round(time.time() - t0, 1)

    by_family = {f: {"scenarios": 0, "ok": 0, "violations": 0, "recovered": 0,
                     "refused": 0} for f in fams}
    for row in scenarios:
        f = by_family[row["family"]]
        f["scenarios"] += 1
        f["ok"] += 1 if row["invariants_ok"] else 0
        f["violations"] += 0 if row["invariants_ok"] else 1
        oc = row["chain_outcome"]
        if oc.get("recovered") or any(isinstance(oc.get(k), dict) and oc[k].get("recovered")
                                      for k in ("attest", "ledger")):
            f["recovered"] += 1
        if scenario_refused(oc):
            f["refused"] += 1
    table, total = ascii_table(by_family)
    print(table)
    all_viol = [v for row in scenarios for v in row["violations"]]
    print(f"simchain: {total['scenarios']} scenario(s), "
          f"invariants_ok={total['ok']}, violations={total['violations']}, "
          f"duration_s={duration}")

    report = {"tool": "simchain", "at": datetime.now().astimezone().isoformat(timespec="seconds"),
              "config": {"seed_base": a.seed, "runs": a.runs, "faults": fams},
              "seeds": a.runs, "faults": len(fams),
              "scenarios": total["scenarios"], "invariants_ok": total["ok"],
              "violations": total["violations"], "duration_s": duration,
              "summary": {"by_family": by_family, "total": total},
              "violation_kinds": sorted({v.split(":")[0] for v in all_viol}),
              # scenario_details is the per-scenario list; the "scenarios" key stays the
              # COUNT -- a duplicate key here would silently shadow the count (the JSON
              # keeps the last), exactly the quiet-shape bug a machine interface must not ship
              "scenario_details": scenarios, "table": table}
    try:
        os.makedirs(os.path.dirname(os.path.abspath(a.report)), exist_ok=True)
        with open(a.report, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(report, fh, ensure_ascii=True, indent=2)
            fh.write("\n")
        print(f"simchain: report written {a.report} "
              f"(seeds={a.runs} faults={len(fams)} violations={total['violations']} "
              f"duration_s={duration}) -- simchain face evidence for doctor")
    except OSError as ex:
        print(f"simchain: report write failed ({ex})")
        return 1
    return 0 if total["violations"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
