#!/usr/bin/env python3
"""agent-verify: the single entry point that chains preflight -> policy -> run -> journal.

    agent_verify.py -Root D:\\proj -Changed src/a.rs -Lang rust -Profile workspace-write
    agent_verify.py -Root D:\\proj -Changed pkg/core.py -Lang python -DryRun

Why one entry point: four scripts that each do one thing correctly still fail as a system if
the caller has to remember the order. The order IS the safety property --

    memory-brief  (what this project already knows)
    timeline-due  (which temporary problems have come due)
    preflight     (nothing is broken before we start)
    budget gate   (declared-empty budget suspends BEFORE run -- see below)
    affected      (the subset, only if it carries a proof)
    policy        (we are allowed to run this at all)
    constraints   (this change is allowed at all)
    journal-begin (we can prove and undo what it writes)
    run           (only now; with --caps each command is wrapped by run_caps.py)
    journal-end   (what it actually wrote)
    memory-note   (a durable, evidenced trace)
    timeline-record (a failure is a problem with a timestamp)

Budget gate (declaration-based, honestly): the tool layer CANNOT measure model-side
token usage, so --budget-token N is the caller's declared remaining budget. A declared
value <= 0 suspends the whole run before anything executes (verdict "suspended (budget)
before run"), records the session as censored in resume.py -- censoring is not failure
-- and prints the continuation block. The gate fires only when the flag is given.

Cost accounting (the chain is a ledger CLIENT, never a meter): after journal-end every
run command's wall-clock is appended to <Store>/cost-ledger.json under a command-level
key (+ --agent-id when given), plus one overhead entry so ledger totals equal chain
wall. Tokens are recorded only when the caller declares them (--spent-token N = model
side usage the harness measured; the tool layer cannot measure it). The budget gate
then reads the same ledger: remaining = declared - recorded tokens of this session
scope (task-id or agent-id); with no token records the gate stays pure declaration --
recorded wall never substitutes for tokens (ledger.py keeps the two columns apart on
purpose). costprobe keeps its own measurement surface (measure-in-place, fail-closed,
one-mtime, derived totals); the chain does not call it and does not alter it -- the
chain is only another ledger WRITER, same accounting rule: totals derived, never
accumulated.

Chain lock: the chain takes <Root>/.journal/chain.lock (O_CREAT|O_EXCL) for its whole
life. A second chain on the same root refuses (fail-closed, verdict "refused by chain
lock") instead of racing journal-begin. The lock is released via atexit on every
normal exit; a killed process leaves it behind ON PURPOSE -- the leftover lock plus
the surviving .journal snapshot is the dirty-state marker the next run diagnoses.

Compact receipt (--compact, opt-in -- full stays the default and the audit artifact):
a single-line machine JSON with the routing fields (verdict/exit/proven), one entry
per step (step, verdict, exit), one per run command (command, exit), plus budget
numbers / cost / agent_id / changes count. Free-text evidence is dropped, which is
where the order of magnitude goes; subagents each pass --agent-id and get their own
compact receipt and their own ledger lines.

Prefix-stable serialization (industry prompt caching: MLSys'24 Prompt Cache
arXiv:2311.04934, EuroSys'25 CacheBlend -- providers discount cached input 50-90%
and agentic deployments report 41-80% API-cost savings, but a hit depends on the
ARTIFACT's prefix stability). So both receipt modes are written deterministic and
prefix-stable: fixed key order, stable fields first, and every per-run volatile
value (timestamp, wall clocks, per-step/per-run seconds, budget/attest/cost state)
re-homed into one tail `volatile` segment -- nothing is dropped, fields only move
(步骤/命令判定信息一个不删). Two consecutive runs of the same task then share the
longest possible byte prefix, and bench_orchestration.py measures that LCP share.
Honest boundary: the tool layer builds the cacheable prefix; whether the provider
honors it with a discount is the provider's decision and cannot be measured locally.
Task-identity fields (root/changed/agent_id) are stable across a task's runs and
stay in the stable head; the volatile segment is the per-run machine state.

Parallel gates (--parallel-gates, opt-in -- serial stays the default): affected, policy
and constraints are read-only classifiers with no shared state, so they run
concurrently (policy also fans out per command); receipt steps are recorded in chain
order and refusals are evaluated in chain order, so verdict, exit and receipt shape
match the serial run exactly. The write side (journal-begin, run, journal-end) is
NEVER parallel. Deliberately not parallelized: preflight/budget/checks (order
semantics), and any gate that ever grows shared state must go back to serial.

Caps (--caps): each run command executes under run_caps.py kernel-level resource caps
(memory / process count on Windows, RLIMIT_AS on POSIX). That is a runaway limiter, NOT
a security boundary: no filesystem/network isolation, no defense against deliberate
escape. policy pre-classifies commands to stop misuse; run-caps bounds the damage if a
permitted command runs away. One gates, the other contains.

A run command that fails with --task-id records itself unproven and suspends the
session (reason=manual); the verdict stays "command failed" and the receipt only gains
appended fields.

If a caller skips one, no individual script can tell. So the chain is the deliverable, and
every step's decision is written into one receipt that can be audited afterwards.

The receipt is written last and in pure ASCII: a machine artifact must survive any reader,
including one that decodes bytes as ANSI.

Exit codes: 0 all good; 1 a guard refused or the command failed; 2 bad invocation.

Ported from agent-verify.ps1. Steps now call the python implementations directly (one less
process spawn, and no dependency on whichever PowerShell is on PATH).
"""

import atexit
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime

import attest  # same tools directory; the budget gate consumes the attest chain balance
import ledger  # same tools directory; the chain appends cost entries, one writer per chain
import agent_settings  # same tools directory; user/project settings resolution

# The policy allow path emits the same two boilerplate lines for every command.
# They are useful when printed once, but repeating them per command makes the full
# receipt (and every model that reads it) pay for the same text N times.  Keep the
# per-command decision and every WARN/DENY/special-note line; drop only these two
# generic lines from allow results.
POLICY_GENERIC_ALLOW_RX = re.compile(
    r"^policy: OK\s+allowed: .* reads or executes$"
)
POLICY_ALLOW_TAIL_RX = re.compile(
    r"^policy: allow \([A-Za-z0-9_-]+(?:-[A-Za-z0-9_-]+)*, network=(?:on|off)\)$"
)

LANGS = {"rust", "python", "node", "go"}
PROFILES = {"read-only", "workspace-write", "full"}

# The receipt may be committed with the repo, so the same shapes privacy_scan hunts for
# are refused at the door of the one field a caller types by hand.
MACHINE_PATH_RX = re.compile(
    r"[A-Za-z]:[\\/]|/Users/[A-Za-z0-9._-]+/|/home/[A-Za-z0-9._-]+/|/mnt/[a-z]/")

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
RESUME = os.path.join(TOOLS, "resume.py")
HANDOFF = os.path.join(TOOLS, "handoff.py")
CLAIMS = os.path.join(os.path.dirname(TOOLS), "scripts", "agent_claims.py")
RUN_CAPS = os.path.join(TOOLS, "run_caps.py")
SECURITY = os.path.join(TOOLS, "security_gate.py")

BUDGET_NOTE = ("declaration-based gate: the tool layer cannot measure model-side token "
               "usage; --budget-token is the caller's declared remaining budget "
               "(continuation of the ENTRY-README boundary #3: honest scope statements "
               "travel with the tool)")
CAPS_NOTE = ("kernel-level resource caps (memory and process count enforced by the "
             "Windows job object, RLIMIT_AS on POSIX); NOT a security boundary: no "
             "filesystem/network isolation, no defense against deliberate escape; "
             "complements policy -- policy pre-classifies commands to stop misuse, "
             "run-caps bounds the damage if a permitted command runs away")
COST_NOTE = ("wall is tool-measured per command; tokens are caller-declared (--spent-token) "
             "or harness-written ledger entries -- the tool layer cannot measure model-side "
             "token usage")
CLAIMS_NOTE = ("model statements are audited against recorded evidence (receipts, disk, "
               "cited artifacts), not believed; VERIFIED/UNVERIFIED/FABRICATED per claim, "
               "FABRICATED refuses the whole chain -- what the model says carries zero "
               "evidential weight on its own; --second-opinion re-derives the same claims "
               "through two independent evidence paths (freshest receipt vs disk + cost "
               "ledger run records), dual-source upgrade or FABRICATED downgrade; "
               "--derivations (default on) re-executes number-claim derivation graphs in "
               "exact rational arithmetic (tools/mre.py): a REJECTED re-execution refuses "
               "the chain as 'rejected: derivation mismatch' with the first mismatching "
               "step, --no-derivations restores the legacy single-source shape")
ATTEST_NOTE = ("--spent-token also lands on the attest hash chain (<Store>/attest-ledger.jsonl): "
               "tamper-evident bookkeeping and dedup (a repeated spend is structurally "
               "impossible), NOT model-side ground truth -- claimed_tokens stay a declaration; "
               "the budget gate consumes the chain balance when it exists, stays "
               "declaration-based (labeled declared-only) when it does not")

HELP = f"""agent-verify: chain preflight -> policy -> run -> journal, one audited receipt.

  agent-verify.sh -Root <dir> -Changed <files> -Lang rust|python|node|go
                    [-Profile read-only|workspace-write|full] [-DryRun]
                    [-Receipt out.json] [-Store <dir>] [-Plan plan.json]
  budget gate:  --budget-token N --task-id <id>
  caps:         --caps [--caps-mem-mb N] [--caps-procs N] [--caps-seconds S]
  subagent:     --agent-id <id>
  cost:         [--spent-token N]   (declared model-side tokens of this run)
  claims:       [--claims-file f.jsonl] [--claims-receipts glob]... [--second-opinion]
                [--derivations (default on) | --no-derivations]  (audit model statements;
                number claims carrying derivations are re-executed by mre, REJECTED refuses)
  coordination: [--claim-gate --claim-who <host:session> [--claims-ledger <CLAIMS.md>]]
                [--handoff <packet.json>]
                (multi-agent gate: the changed paths must be fully claimed by --claim-who
                (false or unknown refuses); a handoff packet's base_commit/state_hash must
                match this tree (unknown or false refuses). Both are opt-in; no flag means
                the chain is exactly what it was.)
  receipt:      [--compact] [--model-receipt <path>] [--settings <file>]
                (settings resolve builtin < user < project .agent/settings.json
                < explicit file; explicit CLI flags win; full receipt stays the
                audit artifact; --model-receipt writes a compact routing view)
  brief:        [--brief]           (stdout lists only failed commands; full receipt
                still records every command and its output tail)
  security:     [--security [--security-mode quick|static|dynamic|runtime|all]]
                (static/dynamic/runtime gate; high/critical or a required scanner
                gap refuses before any write; report path+hash go in the receipt;
                off unless the flag or settings.security.enabled says otherwise)
  delta:        [--delta --against <prev-receipt>] [--delta-full <path>]
                (subagent chains: only changed fields + hash refs for unchanged ones;
                --rebuild <delta> --against <prev> [--out <full.json>] reconstructs
                the receipt byte-identically -- lossless, hash-verified, fail-closed)
  gates:        [--parallel-gates]  (affected/policy/constraints concurrent; opt-in)
  watchdog:     [--gate-deadline-affected S] [--gate-deadline-policy S]
                [--gate-deadline-constraints S]   (default 600s each)
                affected timeout -> sound fallback: full run, unproven (not a skip);
                policy/constraints timeout -> fail-closed refusal naming the gate

Subagent usage: a subagent running a task passes its own id (--agent-id sub-7). The id is
recorded in the receipt (top level and every run[] line, appended fields only -- old
receipt fields untouched) and, via `ledger.py add --agent-id`, on each cost line. With no
flag the receipt is exactly what it always was.

Budget gate (honest scope): the tool layer cannot measure model-side token usage.
--budget-token N is a DECLARATION by the caller. A declared value <= 0 refuses before
run, suspends the session (reason=budget, censored -- not a failure) and prints the
continuation block. No flag, no gate. When the session's cost ledger already carries
token records (--spent-token from earlier runs of the same task-id/agent-id scope), the
gate subtracts them: remaining = declared - recorded. With no token records the gate
stays pure declaration -- recorded WALL never substitutes for tokens.

Cost accounting (honest scope): {COST_NOTE}. Entries land in <Store>/cost-ledger.json
(command-level key + agent-id) and the verdict tail gains one appended `cost:` line.
Additionally --spent-token appends one spend entry to the attest hash chain
(<Store>/attest-ledger.jsonl, {ATTEST_NOTE}). The budget gate consumes the ATTEST
CHAIN BALANCE when the chain exists (remaining = declared - chain spent_net for the
scope); with no chain it keeps the old cost-ledger/declaration behavior, labeled
declared-only in the receipt budget.source. A chain that fails verify refuses the
gate outright (fail-closed). `attest.py import --from-ledger` brings old cost-ledger
spent entries onto the chain so already-recorded spend is neither lost nor counted
twice (identity derivation is shared: task-id, or agent-id, or anon -- same rule in
both writers).

Claim audit (--claims-file, honest scope): {CLAIMS_NOTE}
The audit runs after constraints and before journal-begin: a run is not allowed to
even start on top of claims that contradict reality.

Coordination gates (multi-agent, opt-in; honest scope): --claim-gate answers "am I
allowed to write these paths?" with the same tri-state table as trit-orch's ClaimStore
(true = every changed path is covered by my live claim; false = another owner overlaps;
unknown = not claimed / only partly covered / ledger unreadable). False and unknown
both refuse -- an unclaimed path is not a permission. --handoff answers "is this the
same tree the previous agent handed over?" with trit-orch::handoff_gate's table
(true = valid, unexpired, base_commit and state_hash match; unknown = moved/expired/
state unprovable; false = malformed). Both gates are host-side checks before run;
they are not a security boundary (the claim ledger is a coordination file, not an ACL).

Caps (honest scope): {CAPS_NOTE}
Defaults when --caps is given without parameters: mem 2048 MB, 64 procs, 1800 s.

Delta receipt (--delta --against <prev-receipt>, opt-in; subagent chains): the receipt
is diffed field-by-field against the previous receipt of the same task -- changed
fields travel in full, unchanged fields as bare uppercase sha256 refs (the same
digest family/format as the journal manifest, so any hash can be pulled with
`journal.py get -Hash <hex>`). Lossless by construction: `--rebuild <delta>
--against <prev> -Out <full>` reconstructs the receipt byte-identically (the delta
records the new key order; every unchanged ref is hash-verified at rebuild and a
mismatch refuses). --delta-full <path> additionally writes the full-shape receipt
of this run (audit companion). Diff granularity: two levels -- an unchanged
top-level key is one hash ref; a changed dict key (e.g. `volatile`) is diffed one
level deeper so its stable sub-fields also become refs.

Receipt shape note: steps entries carry step/verdict/exit/output and run entries
command/exit/tail(/caps/agent_id); the per-step and per-run walls live in
volatile.steps_seconds / volatile.run_seconds, index-aligned with those arrays.

出处 (usage provenance, for the prefix-stable work): industry prompt caching --
Prompt Cache (MLSys'24, arXiv:2311.04934), CacheBlend (EuroSys'25); provider
cached-input discounts 50-90%; measured agentic savings 41-80%. The cacheable
prefix is manufactured here deterministically; provider hit rate is not locally
measurable and is claimed nowhere.

Exit codes: 0 all good; 1 a guard refused or the command failed; 2 bad invocation.
"""


def parse_args(argv):
    out = {"root": "", "changed": [], "lang": "", "profile": "workspace-write",
           "settings": "", "allow_network": False, "dry_run": False, "receipt": "",
           "store": "", "plan": "", "allow": [], "agent_id": "", "spent_token": "",
           "compact": False, "brief": False, "model_receipt": "",
           "security": False, "security_mode": "",
           "parallel_gates": False, "claims_file": "", "claims_receipts": [],
           "claim_gate": False, "claim_who": "", "claims_ledger": "", "handoff": "",
           "second_opinion": False, "no_derivations": False, "delta": False,
           "against": "", "rebuild": "", "delta_full": "", "out": "",
           "gate_deadline_affected": "600", "gate_deadline_policy": "600",
           "gate_deadline_constraints": "600", "_given": set()}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        # Boolean flags must be listed here: a bool flag that falls through to the
        # value-branch swallows the NEXT argument as its value (--caps -Root ... would
        # parse -Root as the caps value).
        if name in ("brief", "security"):
            out["brief" if name == "brief" else "security"] = True
            out["_given"].add(name)
            i += 1
            continue
        if name in ("dryrun", "allownetwork", "caps", "compact", "secondopinion",
                    "second-opinion", "derivations", "noderivations", "no-derivations",
                    "delta", "claimgate", "claim-gate"):
            # --derivations is the default-on no-op; --no-derivations opts out of the
            # mre re-execution pass (legacy single-source shape).
            out["no_derivations"] = name in ("noderivations", "no-derivations")
            out["dry_run" if name == "dryrun"
                else "allow_network" if name == "allownetwork"
                else "caps" if name == "caps"
                else "compact" if name == "compact"
                else "delta" if name == "delta"
                else "claim_gate" if name in ("claimgate", "claim-gate")
                else "second_opinion"] = True
            out["_given"].add("compact" if name == "compact" else name)
            i += 1
            continue
        if name == "parallelgates" or name == "parallel-gates":
            out["parallel_gates"] = True
            out["_given"].add("parallel_gates")
            i += 1
            continue
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if name == "changed":
            out["changed"].extend([x for x in val.split(",") if x.strip()])
        elif name == "allow":
            out["allow"].extend([x for x in val.replace(",", " ").split() if x])
        elif name in ("claims-receipts", "claimsreceipts"):
            out["claims_receipts"].extend([x for x in val.split(",") if x.strip()])
        else:
            key = {"root": "root", "lang": "lang", "profile": "profile", "receipt": "receipt",
                   "store": "store", "plan": "plan", "agent-id": "agent_id",
                   "agentid": "agent_id", "spent-token": "spent_token",
                   "spenttoken": "spent_token", "claims-file": "claims_file",
                   "claimsfile": "claims_file", "delta-full": "delta_full",
                   "deltafull": "delta_full", "claim-who": "claim_who",
                   "claimwho": "claim_who", "claims-ledger": "claims_ledger",
                   "claimsledger": "claims_ledger", "handoff": "handoff",
                   "model-receipt": "model_receipt", "modelreceipt": "model_receipt",
                   "settings": "settings", "settings-file": "settings", "settingsfile": "settings",
                   "security-mode": "security_mode", "securitymode": "security_mode",
                   "gate-deadline-affected": "gate_deadline_affected",
                   "gate-deadline-policy": "gate_deadline_policy",
                   "gate-deadline-constraints": "gate_deadline_constraints"}.get(name, name)
            out[key] = val
            out["_given"].add(key)
        i += 2
    return out


def step(args, timeout=None):
    """Run one tool module and report (output lines, exit code, seconds).

    timeout (watchdog, L123): when the gate subprocess exceeds its deadline it is
    killed and reported as exit 124 -- the caller decides the fallback direction
    (affected -> sound unproven fallback; policy/constraints -> fail-closed refusal).
    """
    t0 = time.time()
    try:
        p = subprocess.run([PY] + args, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"out": [f"watchdog: gate deadline {timeout}s exceeded, process killed"],
                "code": 124, "seconds": round(time.time() - t0, 2)}
    lines = ((p.stdout or "") + (p.stderr or "")).splitlines()
    return {"out": lines, "code": p.returncode, "seconds": round(time.time() - t0, 2)}


def skipped(msg):
    return {"out": [msg], "code": 0, "seconds": 0}


def json_tail(result):
    """Last parseable JSON object in a step's merged stdout/stderr.

    Gates print human lines before the machine line (or after), so scanning from the
    tail is deliberate; an unparseable gate result is None and the caller refuses.
    """
    for line in reversed(result["out"]):
        text = line.strip()
        if text.startswith("{") and text.endswith("}"):
            try:
                return json.loads(text)
            except ValueError:
                continue
    return None


def repo_relative(root, paths):
    """Changed paths as claim keys. Absolute paths outside -Root are unclaimable."""
    out, bad = [], []
    for raw in paths:
        full = raw if os.path.isabs(raw) else os.path.join(root, raw)
        rel = os.path.relpath(os.path.abspath(full), root)
        if rel == os.pardir or rel.startswith(os.pardir + os.sep):
            bad.append(raw)
            continue
        out.append(rel.replace("\\", "/"))
    return out, bad


def int_or(val, default):
    """int(val) when a value was actually given; '' must NOT fall into the default."""
    s = str(val).strip() if val is not None else ""
    if not s:
        return default
    return int(s)


def resume_step(argv_tail, root):
    """One resume.py call; if the session does not exist yet, begin it first.

    resume.py itself is fail-closed and refuses to touch a missing session. The
    orchestrator owns the task-id lifecycle, so beginning an empty session here is not
    fabrication: it records exactly the state this run observed (no steps yet), and the
    next line suspends it. argv_tail[1] is always the task-id (progress/suspend shape).
    """
    r = step([RESUME] + argv_tail + ["-Root", root])
    if r["code"] == 2:
        b = step([RESUME, "begin", argv_tail[1], "-Root", root])
        r = step([RESUME] + argv_tail + ["-Root", root]) if b["code"] == 0 else b
    return r


def session_file(root, task_id):
    return os.path.join(root, ".agent", "resume", f"{task_id}.json")


STABLE_HEAD = ("root", "lang", "profile", "changed", "settings", "security", "agent_id", "handoff", "claim",
               "caps", "plan",
               "policy", "steps", "run", "changes", "claims", "watchdog",
               "verdict", "exit")
VOLATILE_ORDER = ("budget", "cost", "wall_s", "steps_seconds", "run_seconds",
                  "resume", "resume_checkpoint", "attest", "dirty", "at")
BUDGET_ORDER = ("declared", "gate", "source", "recorded_tokens", "remaining")
COST_ORDER = ("note", "scope", "tokens", "recorded", "wall_s", "overhead_s")
STEP_STABLE = ("step", "verdict", "exit", "output")
RUN_STABLE = ("command", "exit", "tail", "caps", "agent_id")


def ordered_dict(d, order):
    out = {k: d[k] for k in order if k in d}
    for k in d:
        if k not in out:
            out[k] = d[k]
    return out


def receipt_view(doc):
    """Prefix-stable full receipt: stable head first, one tail volatile segment.

    Same fields as always, re-homed only: the per-step and per-run walls move into
    volatile.steps_seconds / volatile.run_seconds (index-aligned), and every per-run
    value (timestamp, wall_s, budget/attest/cost/resume/dirty state) moves into the
    tail segment. Inside the volatile segment the order is chosen for the LCP metric
    too: static text (cost/budget notes, declared budget, scope) first, the wall
    clocks and the timestamp last -- everything that changes every run sits at the
    very end of the receipt. Consecutive runs of one task then share the longest
    byte prefix, which is what provider prompt caches key on.
    """
    view = {}
    for k in STABLE_HEAD:
        if k in doc:
            view[k] = doc[k]
    volatile = {}
    for k in VOLATILE_ORDER:
        if k not in doc:
            continue
        if k == "budget":
            volatile[k] = ordered_dict(doc[k], BUDGET_ORDER)
        elif k == "cost":
            volatile[k] = ordered_dict(doc[k], COST_ORDER)
        else:
            volatile[k] = doc[k]
    if "steps" in doc:
        view["steps"] = [{k: e[k] for k in STEP_STABLE if k in e} for e in doc["steps"]]
        volatile["steps_seconds"] = [e.get("seconds") for e in doc["steps"]]
    if "run" in doc:
        view["run"] = [{k: e[k] for k in RUN_STABLE if k in e} for e in doc["run"]]
        volatile["run_seconds"] = [e.get("seconds") for e in doc["run"]]
    if volatile:
        view["volatile"] = volatile
    return view


def policy_evidence(lines, code):
    """Per-command policy evidence without the repeated allow boilerplate.

    The batch policy gate returns the exact lines a single invocation would print.
    For an allow, two of those lines are generic (``policy: OK ... reads or
    executes`` and ``policy: allow (<profile>, network=...)``).  They carry no
    per-command decision not already expressed by ``command`` + ``exit``.  WARN
    lines (advisory, exit 0), DENY lines, and special allow notes ("allowed by
    -Allow ...", "write allowed inside workspace", ...) are kept.

    This is an evidence-shape change, not a decision change: the full receipt
    keeps every command and exit; raw policy output remains available from
    policy.py when a caller needs it.
    """
    kept = []
    for line in list(lines or [])[-3:]:
        text = str(line).strip()
        if code == 0 and (POLICY_GENERIC_ALLOW_RX.match(text)
                          or POLICY_ALLOW_TAIL_RX.match(text)):
            continue
        kept.append(line)
    return kept


def compact_view(doc, steps):
    """Prefix-stable --compact shape: one line, routing fields first, volatile tail.

    Keeps every field a machine needs to route on the result (verdict/exit, per-step
    verdicts, per-command exit, proven, cost, budget numbers, agent_id); drops the
    free-text evidence (output tails, policy output, resume block text, notes) that
    makes the full receipt an audit artifact. Walls land in the volatile segment,
    index-aligned, exactly like the full shape.
    """
    c = {"root": doc["root"], "lang": doc["lang"], "profile": doc["profile"],
         "verdict": doc["verdict"], "exit": doc["exit"],
         "steps": [{"step": s["step"], "verdict": s["verdict"], "exit": s["exit"]}
                   for s in steps],
         "run": [{"command": r["command"], "exit": r["exit"]}
                 for r in doc.get("run", [])],
         "changes": len(doc.get("changes", []))}
    if "plan" in doc:
        c["proven"] = bool(doc["plan"].get("proven"))
    if "security" in doc:
        c["security"] = {k: doc["security"].get(k) for k in
                          ("verdict", "exit", "mode", "findings", "blocking", "gaps")}
    if "settings" in doc:
        # routing only: a compact reader needs to know which profile decided,
        # not the entire scanner configuration.
        c["settings"] = {k: doc["settings"].get(k)
                         for k in ("name", "source", "policy_profile")}
    if "agent_id" in doc:
        c["agent_id"] = doc["agent_id"]
    if "handoff" in doc:
        # routing only: id/verdict/hashes are small; reason is free text and stays in full
        c["handoff"] = {k: v for k, v in doc["handoff"].items()
                        if k in ("id", "verdict", "base_commit", "state_hash")}
    if "claim" in doc:
        # paths can be long; count + owner + verdict are the routing information
        c["claim"] = {"who": doc["claim"].get("who"),
                      "verdict": doc["claim"].get("verdict"),
                      "paths": len(doc["claim"].get("paths", [])),
                      "ledger": doc["claim"].get("ledger")}
    if "claims" in doc:
        c["claims"] = doc["claims"]
    volatile = {}
    if "budget" in doc:
        volatile["budget"] = ordered_dict({k: v for k, v in doc["budget"].items()
                                           if k != "gate"}, BUDGET_ORDER)
    if "cost" in doc:
        volatile["cost"] = ordered_dict({k: v for k, v in doc["cost"].items()
                                         if k != "note"}, COST_ORDER)
    if "wall_s" in doc:
        volatile["wall_s"] = doc["wall_s"]
    volatile["steps_seconds"] = [s["seconds"] for s in steps]
    volatile["run_seconds"] = [r["seconds"] for r in doc.get("run", [])]
    if "resume" in doc:
        volatile["resume"] = {k: v for k, v in doc["resume"].items() if k != "output"}
    for k in ("attest", "dirty"):
        if k in doc:
            volatile[k] = doc[k]
    volatile["at"] = doc["at"]
    c["volatile"] = volatile
    return c


def write_view(path, view, compact):
    # ensure_ascii=True is the point, not a detail: a reader that decodes UTF-8 as ANSI can
    # mangle Chinese and even break the closing quote, turning a valid receipt into garbage.
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        if compact:
            json.dump(view, fh, ensure_ascii=True, separators=(",", ":"))
        else:
            json.dump(view, fh, ensure_ascii=True, indent=2)
        fh.write("\n")


def emit_receipt(path, doc, steps, compact):
    """One write surface for both receipt modes: full (indent=2) or compact (single
    line), both prefix-stable (stable head, tail volatile segment)."""
    if not path:
        return
    view = compact_view(doc, steps) if compact else receipt_view(doc)
    write_view(path, view, compact)


def canon(obj):
    return json.dumps(obj, ensure_ascii=True, sort_keys=True, separators=(",", ":"))


def value_hash(v):
    """Bare uppercase sha256 of the canonical JSON form -- the same digest family and
    format as the journal manifest's per-file sha256, so one retrieval surface
    (journal.py get -Hash <hex>) answers every hash a consumer may hold."""
    return hashlib.sha256(canon(v).encode("utf-8")).hexdigest().upper()


def file_hash(path):
    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest().upper()


def diff_views(prev, new):
    """Two-level field diff: an unchanged top-level key is one hash ref; a changed
    dict key is diffed one level deeper so its stable sub-fields also become refs.
    Returns the delta envelope body (added/changed/unchanged/removed/removed_sub/
    orders) -- lossless with the base receipt: rebuild replays key_order through
    these three maps, hash-verifying every ref."""
    added, changed, unchanged, removed, removed_sub, orders = {}, {}, {}, [], {}, {}
    for k, v in new.items():
        if k not in prev:
            added[k] = v
        elif canon(prev[k]) == canon(v):
            unchanged[k] = value_hash(prev[k])
        elif isinstance(v, dict) and isinstance(prev[k], dict):
            sc, sa, su = {}, {}, {}
            for sk, sv in v.items():
                if sk not in prev[k]:
                    sa[sk] = sv
                elif canon(prev[k][sk]) == canon(sv):
                    su[sk] = value_hash(prev[k][sk])
                else:
                    sc[sk] = sv
            sr = [sk for sk in prev[k] if sk not in v]
            changed[k] = sc
            orders[k] = list(v.keys())
            if su:
                unchanged[k] = su
            if sa:
                added[k] = sa
            if sr:
                removed_sub[k] = sr
        else:
            changed[k] = v
    removed = [k for k in prev if k not in new]
    return {"added": added, "changed": changed, "unchanged": unchanged,
            "removed": removed, "removed_sub": removed_sub, "orders": orders}


def delta_envelope(prev_path, new_view, mode):
    body = diff_views(json.loads(open(prev_path, "r", encoding="utf-8-sig").read()),
                      new_view)
    counts = {name: len(body[name]) for name in ("added", "changed", "unchanged",
                                                 "removed", "removed_sub", "orders")}
    env = {"delta": True, "schema": "av-delta/1", "mode": mode,
           "against": file_hash(prev_path),
           "verdict": new_view.get("verdict"), "exit": new_view.get("exit"),
           "counts": counts}
    env.update(body)
    env["key_order"] = list(new_view.keys())
    env["rebuild"] = ("python tools/agent_verify.py --rebuild <delta.json> "
                      "--against <prev-receipt> --out <full.json>")
    return env


def rebuild_from_delta(delta, base, base_digest):
    """Reconstruct the receipt the delta was computed from. Fail-closed: the base
    file must hash to delta['against'], every unchanged ref must hash-verify against
    the base, every key_order key must be covered, and removed keys must match."""
    if delta.get("schema") != "av-delta/1" or not delta.get("delta"):
        raise ValueError("not an av-delta/1 envelope")
    if delta.get("against") != base_digest:
        raise ValueError("base hash mismatch: --against is not the receipt this "
                         "delta was computed against")
    if not isinstance(delta.get("key_order"), list):
        raise ValueError("delta carries no key_order")
    added, changed, unchanged = delta.get("added", {}), delta.get("changed", {}), \
        delta.get("unchanged", {})
    orders = delta.get("orders", {})
    out = {}
    for k in delta["key_order"]:
        if k in changed and isinstance(changed[k], dict) and k in orders:
            base_seg = base.get(k)
            if not isinstance(base_seg, dict):
                raise ValueError(f"changed key '{k}' has no dict base to diff against")
            seg = {}
            for sk in orders[k]:
                if sk in changed[k]:
                    seg[sk] = changed[k][sk]
                elif sk in added.get(k, {}):
                    seg[sk] = added[k][sk]
                elif sk in unchanged.get(k, {}):
                    h = unchanged[k][sk]
                    if sk not in base_seg or value_hash(base_seg[sk]) != h:
                        raise ValueError(f"unchanged ref does not verify: {k}.{sk}")
                    seg[sk] = base_seg[sk]
                else:
                    raise ValueError(f"key_order sub-key not covered: {k}.{sk}")
            out[k] = seg
        elif k in unchanged and isinstance(unchanged[k], str):
            if k not in base or value_hash(base[k]) != unchanged[k]:
                raise ValueError(f"unchanged ref does not verify: {k}")
            out[k] = base[k]
        elif k in added:
            out[k] = added[k]
        elif k in changed:
            out[k] = changed[k]
        else:
            raise ValueError(f"key_order key not covered: {k}")
    extra = sorted(set(base) - set(delta["key_order"]))
    if extra != sorted(delta.get("removed", [])):
        raise ValueError("removed keys do not match the base")
    for k, subs in delta.get("removed_sub", {}).items():
        if k in out and isinstance(out[k], dict):
            still = sorted(set(base.get(k, {})) - set(out[k]))
            if still != sorted(subs):
                raise ValueError(f"removed sub-keys do not match the base: {k}")
    return out


def rebuild_receipts(delta_path, base_path, out_path):
    """--rebuild entry: delta + base receipt -> the original receipt, byte-identical."""
    try:
        base_bytes = open(base_path, "rb").read()
    except OSError as e:
        print(f"agent-verify: --against 不可读（{e}）")
        return 2
    try:
        base = json.loads(base_bytes.decode("utf-8-sig"))
    except (UnicodeDecodeError, ValueError) as e:
        print(f"agent-verify: --against 不是合法 JSON（{e}）")
        return 2
    try:
        delta = json.loads(open(delta_path, "r", encoding="utf-8-sig").read())
    except (OSError, UnicodeDecodeError, ValueError) as e:
        print(f"agent-verify: delta 回执不可读（{e}）")
        return 2
    try:
        view = rebuild_from_delta(delta, base,
                                  hashlib.sha256(base_bytes).hexdigest().upper())
    except ValueError as e:
        print(f"agent-verify: 拒绝 -- 重建失败（{e}）；fail-closed 不输出半份回执")
        return 1
    compact = delta.get("mode") == "compact"
    if out_path:
        write_view(out_path, view, compact)
    else:
        with open(1, "w", encoding="ascii", newline="\n") as fh:
            if compact:
                json.dump(view, fh, ensure_ascii=True, separators=(",", ":"))
            else:
                json.dump(view, fh, ensure_ascii=True, indent=2)
            fh.write("\n")
    return 0


def emit_delta(path, doc, steps, ctx):
    """--delta surface: the receipt diffed against the previous receipt. Writes the
    envelope to path; when ctx['full_path'] is set, also writes this run's plain
    view there (the audit companion the rebuild is checked against)."""
    mode = "compact" if ctx["compact"] else "full"
    view = compact_view(doc, steps) if ctx["compact"] else receipt_view(doc)
    if ctx["full_path"]:
        write_view(ctx["full_path"], view, ctx["compact"])
    if not path:
        return
    env = delta_envelope(ctx["prev_path"], view, mode)
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        json.dump(env, fh, ensure_ascii=True, separators=(",", ":"))
        fh.write("\n")


def sha8(text):
    return hashlib.sha256(text.encode("utf-8", "replace")).hexdigest()[:8]


def cost_scope(task_id, agent_id):
    """Ledger task tag for this chain run: the session the spend belongs to.

    task-id wins (the resume session), agent-id is the subagent fallback; with neither
    there is no session scope -- entries still land (task=av|anon) but the budget gate
    will not subtract them (an anonymous scope shared by unrelated runs must never
    silently shrink anyone's declared budget).
    """
    if task_id:
        return f"av|{task_id}"
    if agent_id:
        return f"av|{agent_id}"
    return "av|anon"


def ledger_tokens_for(entries, scope):
    """Sum of token values recorded under this scope, or None when there are none.

    Wall-only entries do not count -- subtracting seconds from a token budget is the
    category error ledger.py exists to prevent.
    """
    vals = [float(e["tokens"]) for e in entries
            if e.get("task") == scope and e.get("tokens") is not None]
    return round(sum(vals), 2) if vals else None


def append_cost_entries(ledger_path, scope, agent_id, runid, run_log, overhead_s,
                        spent_token):
    """Append this run's actual consumption to the cost ledger. Returns (ok, error).

    Entries: one per run command (command-level key: runid + index + content hash, so a
    re-run of the same command is a new entry, never a dedup hit), one overhead entry
    (ledger wall totals equal chain wall), one tokens entry when the caller declares
    --spent-token. Uses ledger.py's own load/save so the file shape and the
    derived-totals accounting rule stay single-homed.
    """
    try:
        entries = ledger.load(ledger_path)
        if entries is None:
            return False, "ledger corrupt -- refusing to append (fail-closed)"
        ts = datetime.now().astimezone().isoformat(timespec="microseconds")
        for idx, l in enumerate(run_log, start=1):
            e = {"task": scope, "key": f"av|{runid}|cmd{idx}|{sha8(l['command'])}",
                 "tokens": None, "wall_s": l["seconds"], "at": ts}
            if agent_id:
                e["agent_id"] = agent_id
            entries.append(e)
        ov = {"task": scope, "key": f"av|{runid}|overhead", "tokens": None,
              "wall_s": overhead_s, "at": ts}
        if agent_id:
            ov["agent_id"] = agent_id
        entries.append(ov)
        if spent_token is not None:
            sp = {"task": scope, "key": f"av|{runid}|spent", "tokens": spent_token,
                  "wall_s": None, "at": ts}
            if agent_id:
                sp["agent_id"] = agent_id
            entries.append(sp)
        ledger.save(ledger_path, entries)
        return True, ""
    except OSError as e:
        return False, f"ledger write failed: {e}"


def acquire_chain_lock(root):
    """(lock_path, None) on success; (None, held_info) when another chain holds it.

    The lock lives in <Root>/.journal (journal.py's SKIP set keeps it out of every
    snapshot and every diff). Released via atexit; a killed process leaves it behind
    on purpose -- that leftover is the dirty-state marker, not garbage.
    """
    lock_dir = os.path.join(root, ".journal")
    lock_path = os.path.join(lock_dir, "chain.lock")
    try:
        os.makedirs(lock_dir, exist_ok=True)
        fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        try:
            os.write(fd, json.dumps({
                "pid": os.getpid(), "root": root,
                "at": datetime.now().astimezone().isoformat(timespec="microseconds"),
            }).encode("ascii"))
        finally:
            os.close(fd)
    except FileExistsError:
        held = {"lock": ".journal/chain.lock"}
        try:
            with open(lock_path, "r", encoding="utf-8-sig") as fh:
                held.update(json.load(fh))
        except (OSError, ValueError):
            held["content"] = "unreadable"
        return None, held
    except OSError as e:
        # Only a filesystem that cannot host the lock gets here; say so loudly and run
        # unguarded rather than refuse every dry-run on a read-only root.
        return f"warn:{e}", None
    return lock_path, None


def release_chain_lock(lock_path):
    if not lock_path or lock_path == "warn":
        return
    try:
        os.remove(lock_path)
    except OSError:
        pass


def main(argv):
    t_chain_start = time.time()
    if "--help" in argv or "-h" in argv:
        print(HELP)
        return 0
    a = parse_args(argv)
    if a.get("rebuild"):
        if not a.get("against"):
            print("agent-verify: --rebuild needs --against <prev-receipt> (fail-closed: "
                  "a delta without its base cannot be verified)")
            return 2
        return rebuild_receipts(a["rebuild"], a["against"], a.get("out", ""))
    if a.get("against") and not a.get("delta"):
        print("agent-verify: --against only carries meaning with --delta or --rebuild")
        return 2
    if a.get("delta") and not a.get("against"):
        print("agent-verify: --delta needs --against <prev-receipt> (the diff base is "
              "not optional -- without it nothing is verifiable)")
        return 2
    root_in = a["root"].strip()
    if not root_in or not os.path.isdir(root_in):
        print("agent-verify: -Root 必须是已存在的目录")
        return 2
    # Settings are resolved before validation so a user/project profile can
    # supply defaults, but an explicit CLI flag always wins.  A malformed
    # settings file refuses the run: silently ignoring a typo in a security
    # switch is exactly the failure mode this layer exists to prevent.
    try:
        effective, settings_meta = agent_settings.load_settings(
            root_in, explicit=str(a.get("settings", "")).strip())
    except (OSError, ValueError) as e:
        print(f"agent-verify: 设置不可用（{e}）")
        return 2
    a["_effective_settings"] = effective
    a["_settings_meta"] = settings_meta
    if "profile" not in a["_given"]:
        a["profile"] = str(effective.get("policy_profile", a["profile"]))
    if "compact" not in a["_given"]:
        a["compact"] = bool(effective.get("receipt", {}).get("compact", False))
    if "brief" not in a["_given"]:
        a["brief"] = bool(effective.get("receipt", {}).get("brief", False))
    if "security" not in a["_given"]:
        a["security"] = bool(effective.get("security", {}).get("enabled", False))
    if "security_mode" not in a["_given"]:
        a["security_mode"] = str(effective.get("security", {}).get("mode", "quick"))
    if not a["changed"]:
        print("agent-verify: -Changed 至少要给一个文件")
        return 2
    model_receipt = str(a.get("model_receipt", "")).strip()
    if model_receipt and a.get("receipt") and os.path.abspath(model_receipt) == os.path.abspath(a["receipt"]):
        print("agent-verify: --model-receipt 不能和 -Receipt 同一个路径（审计原文与模型视图必须分开）")
        return 2
    if a["lang"].lower() not in LANGS:
        print(f"agent-verify: -Lang 必须是 {sorted(LANGS)} 之一")
        return 2
    if a["profile"].lower() not in PROFILES:
        print(f"agent-verify: -Profile 必须是 {sorted(PROFILES)} 之一")
        return 2
    agent_id = str(a.get("agent_id", "")).strip()
    if agent_id and MACHINE_PATH_RX.search(agent_id):
        print("agent-verify: --agent-id 带本机绝对路径 -- 回执可能随仓提交，请用仓库相对的 id")
        return 2
    spent_token = None
    if str(a.get("spent_token", "")).strip():
        try:
            spent_token = int(a["spent_token"])
        except ValueError:
            print("agent-verify: --spent-token must be an integer")
            return 2
        if spent_token < 0:
            print("agent-verify: --spent-token must be >= 0 (it is a declaration, not a measurement)")
            return 2
    if a.get("second_opinion") and not a.get("claims_file"):
        print("agent-verify: --second-opinion needs --claims-file (the two sources re-derive a claims set)")
        return 2
    if a.get("claim_gate"):
        claim_who = str(a.get("claim_who", "")).strip()
        if not claim_who:
            print("agent-verify: --claim-gate needs --claim-who <host:session> "
                  "(the claim ledger compares owners literally; --agent-id is a run label, not a claim owner)")
            return 2
        if MACHINE_PATH_RX.search(claim_who):
            print("agent-verify: --claim-who 带本机绝对路径 -- 用规范形 <host:session>（例 qoder:abc12345）")
            return 2
    elif a.get("claim_who") or a.get("claims_ledger"):
        print("agent-verify: --claim-who/--claims-ledger only carry meaning with --claim-gate")
        return 2
    if a.get("handoff"):
        packet = str(a["handoff"]).strip()
        if not os.path.isfile(packet):
            print(f"agent-verify: --handoff 包不存在或读不到：{packet}")
            return 2

    root = os.path.abspath(root_in)
    lang = a["lang"].lower()
    profile = a["profile"].lower()
    task_id = str(a.get("task-id", "")).strip()
    store = a["store"].strip() or os.path.join(
        os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "agent-verify",
        root.replace("\\", "_").replace(":", "_"))

    # --- delta base: validated before the chain lock, fail-closed -----------------------
    delta_ctx = None
    if a.get("delta"):
        prev_path = a["against"]
        try:
            with open(prev_path, "rb") as fh:
                prev_doc = json.loads(fh.read().decode("utf-8-sig"))
        except (OSError, UnicodeDecodeError, ValueError) as e:
            print(f"agent-verify: --against 不可用（{e}）-- delta 没有可验证的基线就拒绝开跑")
            return 2
        if not isinstance(prev_doc, dict):
            print("agent-verify: --against 不是回执 JSON（delta 基线必须是上一次回执）")
            return 2
        # shape match: a compact view diffed against a full base (or the reverse) would
        # produce refs that can never verify at rebuild -- refuse at the door
        base_shape = ("full" if ("plan" in prev_doc and "proven" not in prev_doc)
                      else "compact" if ("proven" in prev_doc and "plan" not in prev_doc)
                      else None)
        want_shape = "compact" if a.get("compact") else "full"
        if base_shape is not None and base_shape != want_shape:
            print(f"agent-verify: 拒绝 -- --against 是 {base_shape} 形回执，本次 delta 是 "
                  f"{want_shape} 形（基线与本次回执必须同形，否则重建必然失配）")
            return 2
        delta_ctx = {"prev_path": prev_path, "compact": bool(a.get("compact")),
                     "full_path": a.get("delta_full", "")}

    # --- gate watchdog deadlines: validated up front (0/negative is a bad invocation) ---
    deadlines = {}
    try:
        for g in ("affected", "policy", "constraints"):
            deadlines[g] = float(a.get(f"gate_deadline_{g}", "600"))
    except ValueError:
        print("agent-verify: --gate-deadline-* values must be numbers (seconds)")
        return 2
    bad = [g for g, v in deadlines.items() if v <= 0]
    if bad:
        print(f"agent-verify: --gate-deadline-{bad[0]} must be > 0 seconds "
              f"(a deadline of 0 would hang every gate; lower it to a small "
              f"positive value instead)")
        return 2

    # --- chain lock: one chain per root, fail-closed ------------------------------------
    # Held for the chain's whole life; the leftover lock of a killed process is the
    # dirty-state marker the next run diagnoses (never auto-broken -- breaking a lock
    # you did not verify dead is how two chains race journal-begin).
    lock_path, held = acquire_chain_lock(root)
    if held is not None:
        print("agent-verify: 拒绝 -- 另一条 agent_verify 链正在占用此 root（fail-closed，不与它竞争 journal-begin）")
        print(f"  dirty-state evidence: {held.get('lock')} held by pid={held.get('pid')} "
              f"since {held.get('at')}")
        print("  a killed run leaves the lock AND its journal snapshot behind -- inspect with:")
        print("    python tools/journal.py status -Root <root>")
        print("    python tools/journal.py restore -Root <root>   # 把树放回快照")
        print("  then remove <root>/.journal/chain.lock and rerun")
        if a["receipt"]:
            emit_receipt(a["receipt"], {
                "root": root, "lang": lang, "profile": profile, "changed": a["changed"],
                "at": datetime.now().astimezone().isoformat(timespec="microseconds"),
                "agent_id": agent_id, "verdict": "refused by chain lock", "exit": 1,
                "dirty": held}, [], a.get("compact"))
        return 1
    if lock_path == "warn" or (isinstance(lock_path, str) and lock_path.startswith("warn:")):
        print(f"agent-verify: 警告 -- chain lock 不可用（{lock_path.split(':', 1)[1]}），"
              f"本次运行没有并发护栏（只影响并发竞争，不影响闸门语义）")
    else:
        atexit.register(release_chain_lock, lock_path)

    # --- caps: kernel-level resource caps around the run segment ------------------------
    # Validated up front (bad invocation exits 2 before anything runs) and recorded in
    # the receipt below, so the honesty statement travels with every audited run.
    caps = None
    if a.get("caps"):
        try:
            caps = {"mem_mb": int_or(a.get("caps-mem-mb"), 2048),
                    "procs": int_or(a.get("caps-procs"), 64),
                    "seconds": int_or(a.get("caps-seconds"), 1800)}
        except ValueError:
            print("agent-verify: --caps-mem-mb/--caps-procs/--caps-seconds must be integers")
            return 2
        if min(caps["mem_mb"], caps["procs"], caps["seconds"]) <= 0:
            print("agent-verify: caps values must be positive integers")
            return 2

    doc = {"root": root, "lang": lang, "profile": profile, "changed": a["changed"],
           "at": datetime.now().astimezone().isoformat(timespec="microseconds")}
    doc["settings"] = agent_settings.receipt_echo(a["_effective_settings"],
                                                   a["_settings_meta"])
    if agent_id:
        # appended field only: subagent participation recorded on the receipt; no flag, no key
        doc["agent_id"] = agent_id
    if caps:
        doc["caps"] = dict(caps, note=CAPS_NOTE)
    steps = []

    def add(name, r, verdict):
        steps.append({"step": name, "verdict": verdict, "exit": r["code"],
                      "seconds": r["seconds"], "output": r["out"][-6:]})

    def finish(verdict, code):
        # steps assigned LAST on purpose: assigning before the remaining steps run leaves an
        # empty step list in the receipt, which is exactly the audit trail we wanted.
        doc["steps"] = steps
        doc["verdict"] = verdict
        doc["exit"] = code
        if delta_ctx is not None:
            emit_delta(a["receipt"], doc, steps, delta_ctx)
        else:
            emit_receipt(a["receipt"], doc, steps, a.get("compact"))
        if a.get("model_receipt"):
            # Separate surfaces: the caller keeps the full (or delta) audit receipt
            # and gives the model only the compact routing view.  No field used by a
            # gate is removed from the audit artifact.
            write_view(a["model_receipt"], compact_view(doc, steps), True)
        return code

    # --- 0. memory brief: what this project already knows ------------------------------
    mem_init = os.path.exists(os.path.join(root, ".agent", "memory", "meta.json"))
    mb = (step([os.path.join(TOOLS, "memory.py"), "-Mode", "brief", "-Root", root])
          if mem_init else skipped("memory: 未初始化（项目没有 .agent/memory），跳过"))
    add("memory-brief", mb, "ok" if mem_init else "skipped")
    for ln in mb["out"]:
        print(ln)

    # --- 1. timeline due: temporary problems come due -----------------------------------
    tl_init = os.path.exists(os.path.join(root, ".agent", "timeline.json"))
    td = (step([os.path.join(TOOLS, "timeline.py"), "-Mode", "due", "-Root", root])
          if tl_init else skipped("timeline: 未初始化，跳过"))
    add("timeline-due", td, "due" if td["code"] == 1 else ("ok" if tl_init else "skipped"))
    if td["code"] == 1:
        for ln in td["out"]:
            print("  " + ln)

    # --- 2. preflight: nothing is broken before we start -------------------------------
    pf = step([os.path.join(TOOLS, "refactor_guard.py"), "-Mode", "preflight", "-Repo", root])
    add("preflight", pf, "ok" if pf["code"] == 0 else "refused")
    if pf["code"] != 0:
        for ln in pf["out"]:
            print(ln)
        print("agent-verify: 拒绝 -- preflight 判红，坏代码不要落到磁盘上")
        return finish("refused by preflight", 1)

    # --- 2.1 security gate: static/dynamic/runtime evidence before any write ----------
    # Opt-in through --security or settings.security.enabled; the default is
    # OFF so existing chains/tests keep their timing.  A high/critical finding
    # or a required scanner gap refuses; the report path/hash travels in the
    # receipt so agent_audit can verify the same evidence later.
    if a.get("security"):
        mode = str(a.get("security_mode", "") or
                   effective.get("security", {}).get("mode", "quick")).lower()
        timeout_s = float(effective.get("security", {}).get("timeout_s", 900)) + 30
        report_rel = os.path.join("target", "agent-security", "security-gate.json")
        report_abs = os.path.join(root, report_rel)
        sec_args = [SECURITY, "--root", root, "--mode", mode, "--out", report_abs, "--json"]
        if str(a.get("settings", "")).strip():
            sec_args += ["--settings", os.path.abspath(str(a["settings"]).strip())]
        sr = step(sec_args, timeout=timeout_s)
        sdoc = json_tail(sr) or {}
        sverdict = str(sdoc.get("verdict", "")).lower()
        doc["security"] = {"verdict": sverdict or "unreadable",
                           "exit": sr["code"], "mode": mode,
                           "findings": (sdoc.get("summary") or {}).get("findings", 0),
                           "blocking": (sdoc.get("summary") or {}).get("blocking", 0),
                           "gaps": len(sdoc.get("gaps") or []),
                           "report": report_rel.replace("\\", "/")}
        if os.path.isfile(report_abs):
            doc["security"]["sha256"] = file_hash(report_abs)
        add("security-gate", sr, "ok" if sr["code"] == 0 and sverdict == "pass" else "refused")
        if sr["code"] != 0 or sverdict != "pass":
            for ln in sr["out"][-8:]:
                print(ln)
            print("agent-verify: 拒绝 -- security gate 未通过（高危/关键发现或必需扫描器缺口；"
                  "报告在 target/agent-security/security-gate.json）")
            return finish("refused by security gate", 1)

    # --- 2.2 handoff gate: is this the same tree the previous agent handed over? ------
    # trit-orch::handoff_gate's tri-state table, host-side. Unknown is NOT permission:
    # an expired/moved/unprovable state must be reconciled first, not waved through.
    if a.get("handoff"):
        hr = step([HANDOFF, "gate", "-Root", root, "-Packet", a["handoff"], "--json"])
        hdoc = json_tail(hr) or {}
        hverdict = str(hdoc.get("verdict", "")).lower()
        add("handoff", hr, "ok" if hverdict == "true" else "refused")
        doc["handoff"] = {"packet": os.path.basename(str(a["handoff"])),
                          "id": hdoc.get("id"), "verdict": hdoc.get("verdict"),
                          "reason": hdoc.get("reason"),
                          "base_commit": hdoc.get("base_commit"),
                          "state_hash": hdoc.get("state_hash")}
        if hverdict != "true":
            for ln in hr["out"]:
                print(ln)
            print("agent-verify: 拒绝 -- 交接包与本树对不上（unknown 不是通行证，"
                  "先对账或用 handoff.py 重写包）")
            return finish("refused by handoff", 1)

    # --- 2.25 claim gate: am I allowed to write these paths? ---------------------------
    # The ledger is a coordination file, not an ACL; but "not claimed" is Unknown and
    # Unknown refuses -- an unclaimed path is not a default permission.
    if a.get("claim_gate"):
        who = str(a["claim_who"]).strip()
        paths, outside = repo_relative(root, a["changed"])
        claims_ledger = str(a.get("claims_ledger", "")).strip()
        if claims_ledger:
            rel = os.path.relpath(os.path.abspath(claims_ledger), root)
            label = rel.replace("\\", "/") if not rel.startswith(os.pardir) else os.path.basename(claims_ledger)
        else:
            label = "default"
        doc["claim"] = {"who": who, "paths": paths, "ledger": label}
        if outside:
            cr = {"out": [f"claim-gate: changed path outside -Root is unclaimable: {outside}"],
                  "code": 1, "seconds": 0}
            cverdict, creason = "unknown", "changed path outside -Root"
        else:
            args = [CLAIMS, "gate", "--who", who, "--paths", *paths, "--json"]
            if claims_ledger:
                args += ["--claims", claims_ledger]
            cr = step(args)
            cdoc = json_tail(cr) or {}
            cverdict = str(cdoc.get("verdict", "")).lower()
            creason = ("" if cverdict == "true"
                       else "path held by another active claim" if cverdict == "false"
                       else "paths are not fully claimed by this owner")
            if cverdict == "true" and cdoc.get("claims") is not None:
                doc["claim"]["ledger_claims"] = cdoc.get("claims")
        add("claim-gate", cr, "ok" if cverdict == "true" else "refused")
        doc["claim"]["verdict"] = cverdict or "unreadable"
        if cverdict != "true":
            for ln in cr["out"]:
                print(ln)
            print(f"agent-verify: 拒绝 -- 认领门不通过（{creason or 'gate unreadable'}）："
                  f"先 `python scripts/agent_claims.py claim --who {who} --repo <o/r> "
                  f"--paths {' '.join(paths) or '<paths>'} --plan <方案>`；"
                  f"重叠时 compare 后由败方 release/收窄")
            return finish("refused by claim gate", 1)

    # --- 2.5 budget gate: suspend before run when the declared budget is empty ----------
    # Honest by construction: no tool can measure the model's remaining tokens, so this
    # gate acts only on the caller's declaration (BUDGET_NOTE travels in the receipt).
    # A declared value <= 0 means "do not even start": the session is suspended with
    # reason=budget -- censoring, which is not failure -- and the continuation block is
    # printed for whoever picks the task up in a fresh session.
    # Book-of-record precedence: when an attest chain exists for this store it IS the
    # spend book (tamper-evident, dedup-enforced); the gate consumes its verified
    # balance for the scope. No chain -> the old cost-ledger/declaration behavior,
    # labeled declared-only (the recorded numbers there are caller declarations).
    # Either book corrupt or broken refuses the gate outright -- a gate that cannot
    # trust its own books must not silently degrade to a weaker mode. Wall-only
    # records do NOT count (seconds are not tokens).
    if "budget-token" in a:
        try:
            budget = int(a["budget-token"])
        except ValueError:
            print("agent-verify: --budget-token must be an integer")
            return 2
        scope = cost_scope(task_id, agent_id)
        recorded = None
        budget_source = "declared-only"
        attest_path = os.path.join(store, "attest-ledger.jsonl")
        if os.path.exists(attest_path):
            entries, err = attest.load_chain(attest_path)
            ok, reason = (attest.verify_chain(entries) if entries is not None
                          else (False, err))
            if not ok:
                print(f"agent-verify: 拒绝 -- attest 链损坏且无法信任（{reason}），"
                      f"认证余额无法核算，fail-closed")
                return finish("refused by attest ledger", 1)
            recorded = attest.spent_net_for(entries, task_id, agent_id)
            budget_source = "attest-chain"
        elif scope != "av|anon":
            cost_ledger_path = os.path.join(store, "cost-ledger.json")
            prev_entries = ledger.load(cost_ledger_path)
            if prev_entries is None:
                print(f"agent-verify: 拒绝 -- 预算台账损坏且无法读取（{os.path.basename(store)}/cost-ledger.json），"
                      f"剩余预算无法核算，fail-closed")
                return finish("refused by budget ledger", 1)
            recorded = ledger_tokens_for(prev_entries, scope)
        remaining = budget if recorded is None else round(budget - recorded, 2)
        doc["budget"] = {"declared": budget, "gate": BUDGET_NOTE,
                         "recorded_tokens": recorded, "remaining": remaining,
                         "source": budget_source}
        if remaining <= 0:
            why = (f"declared {budget} <= 0" if recorded is None
                   else f"declared {budget} - recorded {recorded} = {remaining} <= 0")
            print(f"agent-verify: budget gate -- {why}, suspend before run")
            if task_id:
                if not os.path.exists(session_file(root, task_id)):
                    # The session records what this run observed: nothing yet, budget 0.
                    step([RESUME, "begin", task_id, "--budget-token", str(max(budget, 0)),
                          "-Root", root])
                sr = step([RESUME, "suspend", task_id, "--reason", "budget", "-Root", root])
                rr = step([RESUME, "resume", task_id, "-Root", root])
                doc["resume"] = {"task": task_id, "reason": "budget",
                                 "suspend": sr["code"], "output": (sr["out"] + rr["out"])[-12:]}
                for ln in sr["out"] + rr["out"]:
                    print(ln)
            print("agent-verify: suspended (budget) before run -- censored is not a failure")
            return finish("suspended (budget) before run", 1)

    # --- 3. checks: the project's recorded problems still cannot recur ------------------
    # Runs fast tier only: forbid/require are text scans (milliseconds), so _not_ closing
    # this gate costs nothing. A red here means a rule the project already paid for is
    # broken now -- it is refused before anything runs, same as preflight.
    ch_init = os.path.exists(os.path.join(root, ".agent", "checks", "registry.json"))
    chk = (step([os.path.join(TOOLS, "checks.py"), "run", "-Root", root, "-Tier", "fast"])
           if ch_init else skipped("checks: 未初始化（没有 .agent/checks/registry.json），跳过"))
    add("checks", chk, ("ok" if ch_init else "skipped") if chk["code"] == 0 else "refused")
    if chk["code"] != 0:
        for ln in chk["out"]:
            print(ln)
        print("agent-verify: 拒绝 -- 已固化的检查判红，这条缺陷又回来了")
        return finish("refused by checks", 1)

    # --- 4. affected + policy + constraints: the three read-only classifier gates -------
    # Serial is the historical shape and stays the default. --parallel-gates runs the
    # read-only classifiers concurrently where they have no shared state. The policy
    # gate itself is ONE batch call since L123: a single policy.py process classifies
    # the whole plan (-CommandsFile), per-command results identical to the old
    # one-subprocess-per-command walk (pinned by selftest) -- that spawn-per-command
    # walk was the dominant term of the guard overhead. Receipt steps are recorded in
    # CHAIN order, refusals evaluated in chain order -- same verdict, same exit, same
    # receipt shape. The write side (journal-begin, run, journal-end) is NEVER parallel.
    def affected_gate():
        if a["plan"]:
            # replaying a plan is allowed on purpose: it is how the chain is tested without
            # invoking a real build, and it keeps the run/journal steps observable.
            return skipped(f"affected: 使用给定 plan ({a['plan']})"), a["plan"]
        p_path = os.path.join(os.environ.get("TEMP", "/tmp"),
                              f"av-plan-{uuid.uuid4().hex[:8]}.json")
        ap = step([os.path.join(TOOLS, "affected.py"), "-Path", root,
                   "-Changed", ",".join(a["changed"]), "-Lang", lang, "-Out", p_path],
                  timeout=deadlines["affected"])
        return ap, p_path

    def policy_gate():
        b_path = os.path.join(os.environ.get("TEMP", "/tmp"),
                              f"av-policy-{uuid.uuid4().hex[:8]}.json")
        with open(b_path, "w", encoding="ascii", newline="\n") as fh:
            json.dump({"commands": commands}, fh, ensure_ascii=True)
        pa = [os.path.join(TOOLS, "policy.py"), "-Profile", profile,
              "-Root", root, "-CommandsFile", b_path]
        if a["allow_network"]:
            pa.append("-AllowNetwork")
        return step(pa, timeout=deadlines["policy"])

    def constraints_gate():
        c_init = os.path.exists(os.path.join(root, ".agent", "constraints.json"))
        return (step([os.path.join(TOOLS, "constraints.py"), "-Mode", "check", "-Root", root]
                     + (["-Allow", " , ".join(a["allow"])] if a["allow"] else []),
                     timeout=deadlines["constraints"])
                if c_init else skipped("constraints: 未初始化（没有 .agent/constraints.json），跳过"))

    pool = None
    ck_future = None
    if a.get("parallel_gates"):
        pool = ThreadPoolExecutor(max_workers=2)
        ck_future = pool.submit(constraints_gate)
    try:
        ap, plan_path = affected_gate()
        if ap["code"] == 124:
            # watchdog: the affected gate is a classifier, not evidence -- its timeout
            # falls back to the sound shape (full run, unproven), never to a skip.
            add("affected", ap, "timeout")
            doc.setdefault("watchdog", {})["affected"] = (
                f"deadline {deadlines['affected']}s exceeded -> sound fallback: "
                f"full run, unproven (fallback, not a skip)")
            print(f"agent-verify: affected 门看门狗超时（deadline {deadlines['affected']}s）"
                  f"-- sound 回退全量 unproven（不是跳过）")
            doc["plan"] = {"proven": False, "commands": [],
                           "unproven": [f"affected gate deadline exceeded "
                                        f"({deadlines['affected']}s) -- sound fallback to "
                                        f"full run, unproven"]}
            commands, proven = [], False
        else:
            add("affected", ap, "ok")
            plan = None
            if os.path.exists(plan_path):
                try:
                    with open(plan_path, "r", encoding="utf-8-sig") as fh:
                        plan = json.load(fh)
                except ValueError:
                    # a plan that cannot parse must not silently become an empty (hence
                    # all-pass) policy check -- that would be refuse-nothing and run-nothing
                    print("agent-verify: 拒绝 -- plan 文件无法解析（坏 JSON 不许静默变成空计划）")
                    return finish("refused by plan", 1)
            commands = plan.get("commands", []) if plan else []
            proven = bool(plan.get("proven")) if plan else False
            doc["plan"] = {"proven": proven, "commands": commands,
                           "unproven": plan.get("unproven", ["plan unavailable"]) if plan else ["plan unavailable"]}

        # --- 4. policy: are we even allowed to run it (one batch call) ------------------
        if not commands:
            # nothing to classify -- the gate is vacuously ok, same as the old walk over
            # an empty command list (the affected-watchdog fallback lands here)
            doc["policy"] = []
            add("policy", {"out": [], "code": 0, "seconds": 0}, "ok")
        else:
            pb = policy_gate()
            if pb["code"] == 124:
                add("policy", pb, "timeout")
                doc.setdefault("watchdog", {})["policy"] = (
                    f"deadline {deadlines['policy']}s exceeded -> fail-closed refusal "
                    f"(batch classification killed)")
                print(f"agent-verify: 拒绝 -- policy 门看门狗超时（deadline "
                      f"{deadlines['policy']}s），fail-closed 拒绝整链")
                return finish("refused by policy watchdog", 1)
            batch = None
            for ln in pb["out"]:
                s = ln.strip()
                if s.startswith("{") and s.endswith("}"):
                    try:
                        batch = json.loads(s)
                        break
                    except ValueError:
                        batch = None
            prs = batch.get("results") if isinstance(batch, dict) else None
            if not isinstance(prs, list):
                add("policy", pb, "refused")
                print("agent-verify: 拒绝 -- policy 批量分类结果不可解析（fail-closed，不许静默放行）")
                return finish("refused by policy", 1)
            policy_log = []
            for r in prs:
                evidence = policy_evidence(r.get("lines"), r.get("exit"))
                entry = {"command": r.get("command"), "exit": r.get("exit")}
                if evidence:
                    # empty output is the common allow case: omit the key instead of
                    # writing output=[] for every command in the audit receipt.
                    entry["output"] = evidence
                policy_log.append(entry)
            policy_ok = all(r.get("exit") == 0 for r in prs)
            doc["policy"] = policy_log
            add("policy", {"out": [], "code": 0 if policy_ok else 1, "seconds": pb["seconds"]},
                "ok" if policy_ok else "refused")
            if not policy_ok:
                print(f"agent-verify: 拒绝 -- 权限档 {profile} 不允许这些命令")
                return finish("refused by policy", 1)

        # --- 5. constraints: is this change allowed at all ------------------------------
        ck = ck_future.result() if ck_future is not None else constraints_gate()
        c_init = os.path.exists(os.path.join(root, ".agent", "constraints.json"))
        if ck["code"] == 124:
            add("constraints", ck, "timeout")
            doc.setdefault("watchdog", {})["constraints"] = (
                f"deadline {deadlines['constraints']}s exceeded -> fail-closed refusal "
                f"(check killed)")
            print(f"agent-verify: 拒绝 -- constraints 门看门狗超时（deadline "
                  f"{deadlines['constraints']}s），fail-closed 拒绝整链")
            return finish("refused by constraints watchdog", 1)
        ck_verdict = ("ok" if c_init else "skipped") if ck["code"] == 0 else "refused"
        add("constraints", ck, ck_verdict)
        if ck["code"] != 0:
            for ln in ck["out"]:
                print(ln)
            print("agent-verify: 拒绝 -- 改动触碰了本仓的禁止操作（见上）")
            return finish("refused by constraints", 1)
    finally:
        if pool is not None:
            pool.shutdown(wait=True)

    # --- 5.5 claim audit: model statements are checked against reality -----------------
    # After constraints, before journal-begin: a run does not get to start on top of
    # claims that contradict reality. UNVERIFIED claims pass but are recorded (they
    # must not be taken as true); FABRICATED claims refuse the whole chain -- the
    # loudest alarm this chain has. A claims file that cannot be audited (bad line,
    # machine path) refuses too: nothing is believed from a half-readable claim set.
    if a.get("claims_file"):
        ca_args = [os.path.join(TOOLS, "claim_audit.py"), "-Claims", a["claims_file"],
                   "-Root", root]
        if a.get("no_derivations"):
            ca_args.append("-NoDerivations")
        for g in a.get("claims_receipts", []):
            ca_args += ["-Receipts", g]
        ca = step(ca_args)
        ca_text = "\n".join(ca["out"])
        add("claim-audit", ca, {0: "ok", 1: "unverified", 2: "fabricated",
                                3: "refused"}.get(ca["code"], "refused"))
        m = re.search(r"summary: total=(\d+) verified=(\d+) unverified=(\d+) "
                      r"fabricated=(\d+)", ca_text)
        if m:
            doc["claims"] = {"file": os.path.basename(a["claims_file"]),
                             "total": int(m.group(1)), "verified": int(m.group(2)),
                             "unverified": int(m.group(3)), "fabricated": int(m.group(4))}
        # decomposition pass-through, appended fields only: an aggregate-heavy claims
        # set shows in the receipt how many sentences were decomposed into how many
        # atoms (the aggregate verdict itself is computed inside claim_audit)
        ma = re.search(r"summary: aggregates=(\d+) atoms=(\d+)", ca_text)
        if ma and "claims" in doc:
            doc["claims"]["aggregates"] = int(ma.group(1))
            doc["claims"]["atoms"] = int(ma.group(2))
        # derivation pass-through, appended fields only: derivation-bearing number
        # claims show how many re-executions matched and how many contradicted
        md = re.search(r"summary: derivations verified=(\d+) rejected=(\d+)", ca_text)
        if md and "claims" in doc:
            doc["claims"]["derivations"] = {"verified": int(md.group(1)),
                                            "rejected": int(md.group(2))}
        if ca["code"] in (2, 3):
            for ln in ca["out"]:
                print(ln)
            if ca["code"] == 2:
                if re.search(r"derivation REJECTED", ca_text):
                    print("agent-verify: 拒绝 -- 推导重执行与现实矛盾（derivation REJECTED）："
                          "附推导的数字声明算不出它声称的结果")
                    return finish("rejected: derivation mismatch", 1)
                print("agent-verify: 拒绝 -- 声明与现实矛盾（FABRICATED）：模型说什么不算什么")
                return finish("rejected: fabricated claims", 1)
            print("agent-verify: 拒绝 -- claims 文件不可审计（fail-closed）")
            return finish("refused by claims", 1)

    # --- 5.6 second opinion (opt-in --second-opinion): the claims step runs both sources
    # The same claims set is re-derived independently: src1 = the freshest receipt alone,
    # src2 = disk reality + the store's cost ledger run records. VERIFIED only survives
    # as dual-source when BOTH paths walk through; a contradiction downgrades to
    # FABRICATED and refuses the chain exactly like the primary audit does. UNVERIFIED
    # claims pass but are recorded (they must not be taken as true).
    if a.get("second_opinion"):
        so_args = [os.path.join(TOOLS, "claim_audit.py"), "second-opinion",
                   "-Claims", a["claims_file"], "-Root", root,
                   "-Ledger", os.path.join(store, "cost-ledger.json")]
        for g in a.get("claims_receipts", []):
            so_args += ["-Receipts", g]
        so = step(so_args)
        add("claim-second-opinion", so, {0: "ok", 1: "unverified", 2: "fabricated",
                                        3: "refused"}.get(so["code"], "refused"))
        so_text = "\n".join(so["out"])
        m2 = re.search(r"summary: total=(\d+) verified=(\d+) unverified=(\d+) "
                       r"fabricated=(\d+)", so_text)
        if m2 and "claims" in doc:
            doc["claims"]["second"] = {"total": int(m2.group(1)),
                                       "verified": int(m2.group(2)),
                                       "unverified": int(m2.group(3)),
                                       "fabricated": int(m2.group(4))}
            m3 = re.search(r"dual-source=(\d+)", so_text)
            if m3:
                doc["claims"]["second"]["dual_source"] = int(m3.group(1))
        md2 = re.search(r"summary: derivations verified=(\d+) rejected=(\d+)", so_text)
        if md2 and "claims" in doc and "second" in doc["claims"]:
            doc["claims"]["second"]["derivations"] = {"verified": int(md2.group(1)),
                                                      "rejected": int(md2.group(2))}
        if so["code"] in (2, 3):
            for ln in so["out"]:
                print(ln)
            if so["code"] == 2:
                if re.search(r"derivation REJECTED", so_text):
                    print("agent-verify: 拒绝 -- 独立复核抓到推导重执行矛盾（derivation REJECTED）")
                    return finish("rejected: derivation mismatch (second opinion)", 1)
                print("agent-verify: 拒绝 -- 独立复核与现实矛盾（second opinion FABRICATED）")
                return finish("rejected: fabricated claims (second opinion)", 1)
            print("agent-verify: 拒绝 -- 独立复核不可审计（fail-closed）")
            return finish("refused by claims (second opinion)", 1)

    # --- dry-run: report and stop, before anything is written --------------------------
    if a["dry_run"]:
        print(f"agent-verify: dry-run -- proven={proven}, {len(commands)} command(s), policy ok")
        for cmd in commands:
            print(f"  would run: {cmd}")
        if not proven:
            print("  note: plan is unproven, so this is the full run")
        return finish("dry-run", 0)

    # --- 6. journal: so we can prove and undo what happens next -----------------------
    jm = step([os.path.join(TOOLS, "journal.py"), "-Mode", "begin", "-Root", root, "-Store", store])
    add("journal-begin", jm, "ok" if jm["code"] == 0 else "failed")
    if jm["code"] != 0:
        # journal-begin is a gate, not decoration: a run without a snapshot has no proof
        # of what it wrote and no undo. Proceeding used to produce exactly that -- a run
        # with no evidence chain, and journal-end then reading no manifest at all.
        for ln in jm["out"]:
            print(ln)
        print("agent-verify: 拒绝 -- journal 快照建不起来，没有证据链的 run 不许发生")
        return finish("refused by journal", 1)

    # --- 7. run ------------------------------------------------------------------------
    if caps:
        print(f"agent-verify: caps on -- mem={caps['mem_mb']}MB procs={caps['procs']} "
              f"seconds={caps['seconds']} (kernel caps, not a security boundary)")
    run_log, ran_ok = [], True
    for cmd in commands:
        if caps:
            # The caps wrap the runner itself: job membership is inherited, so the shell
            # and everything it spawns are inside the same job. Shell-picking (the L44
            # WSL-bash lesson) stays in exactly one place, _run.py.
            r = step([RUN_CAPS, "--mem-mb", str(caps["mem_mb"]),
                      "--procs", str(caps["procs"]), "--seconds", str(caps["seconds"]),
                      "--", PY, os.path.join(TOOLS, "_run.py"), "-Root", root,
                      "-Command", cmd])
        else:
            r = step([os.path.join(TOOLS, "_run.py"), "-Root", root, "-Command", cmd])
        entry = {"command": cmd, "exit": r["code"], "seconds": r["seconds"],
                 "tail": r["out"][-5:]}
        # Appended field only: the caps parameters are evidence, the old shape is untouched.
        if caps:
            entry["caps"] = dict(caps)
        if agent_id:
            entry["agent_id"] = agent_id
        run_log.append(entry)
        if r["code"] != 0:
            ran_ok = False
    doc["run"] = run_log
    total_run = round(sum(x["seconds"] for x in run_log), 2)
    add("run", {"out": [], "code": 0 if ran_ok else 1, "seconds": total_run},
        "ok" if ran_ok else "failed")

    # --- 7.5 failure checkpoint: a failed run suspends the session (reason=manual) ------
    # The verdict stays "command failed" -- the suspend is a checkpoint for whoever
    # continues, not a reclassification of the outcome.
    if not ran_ok and task_id:
        failed = [x for x in run_log if x["exit"] != 0]
        pr_rcs = []
        for idx, l in enumerate(failed, start=1):
            pr = resume_step(["progress", task_id, f"run:{idx}", "--unproven", "--note",
                              f"exit {l['exit']}: {l['command']}"[:120]], root)
            pr_rcs.append(pr["code"])
        sr = resume_step(["suspend", task_id, "--reason", "manual"], root)
        pr_ok = all(c == 0 for c in pr_rcs)
        add("resume-progress", {"out": [], "code": 0 if pr_ok else 1, "seconds": 0},
            "ok" if pr_ok else "failed")
        add("resume-suspend", sr, "ok" if sr["code"] == 0 else "failed")
        doc["resume"] = {"task": task_id, "reason": "manual",
                         "progress": pr_rcs, "suspend": sr["code"]}
        doc["resume_checkpoint"] = (f"task {task_id} suspended (manual) -- next session: "
                                    f"python tools/resume.py resume {task_id}")

    # --- 8. journal end: what did it actually write -----------------------------------
    je = step([os.path.join(TOOLS, "journal.py"), "-Mode", "end", "-Root", root, "-Store", store])
    changes = [ln for ln in je["out"] if re.search(r"journal:\s+(added|deleted|modified)", ln)]
    # Exit codes 0/1/else map to clean/changed/failed explicitly: journal.py exits 2 on
    # "no snapshot" and 2 on a corrupt manifest -- folding every non-zero into "changed"
    # classified a broken audit chain as a routine write (the receipt lied, L114).
    add("journal-end", je,
        "clean" if je["code"] == 0 else ("changed" if je["code"] == 1 else "failed"))
    doc["changes"] = changes
    doc["wall_s"] = total_run

    # --- 8.5 cost recording: this run's actual consumption goes on the ledger ----------
    # journal-end 后、memory-note 前。失败也只记账面（cost 记账不是闸）：loud, not fatal.
    runid = uuid.uuid4().hex[:8]
    scope = cost_scope(task_id, agent_id)
    chain_wall = round(time.time() - t_chain_start, 2)
    overhead_s = max(0.0, round(chain_wall - total_run, 2))
    cost_ok, cost_err = append_cost_entries(
        os.path.join(store, "cost-ledger.json"), scope, agent_id, runid, run_log,
        overhead_s, spent_token)
    doc["cost"] = {"wall_s": total_run, "overhead_s": overhead_s,
                   "tokens": spent_token, "recorded": cost_ok, "scope": scope,
                   "note": COST_NOTE}
    if not cost_ok:
        doc["cost"]["error"] = cost_err
        print(f"agent-verify: cost 记账失败（{cost_err}）-- 消耗没上账，回执 cost 字段已标注")

    # --- 8.6 attest spend: the declared tokens land on the authentication chain -------
    # Identity derivation shared with attest import (L116): task-id, else agent-id,
    # else anon -- import reconstructs the same key from cost-ledger rows, so a
    # re-import skips what the live run already recorded instead of double counting.
    # Loud, not fatal (the run is done by now), but a failed append means the next
    # budget gate will UNDERCOUNT this run -- the receipt names it.
    if spent_token is not None:
        attest_task = task_id or agent_id or "anon"
        entry, attest_err = attest.append_entry(
            store, "spend", agent_id, attest_task, runid, spent_token,
            note=f"agent-verify run (task={task_id or '-'}, agent={agent_id or '-'})")
        doc["attest"] = {"appended": entry is not None,
                         "seq": entry["seq"] if entry else None,
                         "task_id": attest_task, "runid": runid}
        if entry is None:
            doc["attest"]["error"] = attest_err
            print(f"agent-verify: attest 记账失败（{attest_err}）-- 本次 spend 没上认证链，"
                  f"后续预算闸会少算这次消耗")

    verdict = ("command failed" if not ran_ok else
               ("passed on full run (plan was unproven)" if not proven else "passed on proven subset"))

    # --- 9. memory note: leave a durable, evidenced trace of this run ------------------
    if mem_init and run_log:
        commit = subprocess.run(["git", "-C", root, "rev-parse", "--short", "HEAD"],
                                capture_output=True, text=True).stdout.strip()
        commit_txt = f"commit {commit}" if commit else "no git"
        for l in run_log:
            mn = step([os.path.join(TOOLS, "memory.py"), "-Mode", "note", "-Root", root,
                       "-Kind", "command",
                       "-Title", f"{l['command']} -> exit {l['exit']} in {l['seconds']}s",
                       "-Detail", f"由 agent-verify 记录；plan.proven={str(proven).lower()}；wall={l['seconds']}s",
                       "-Evidence", f"{l['command']} | exit {l['exit']} | {commit_txt}",
                       "-Tags", "agent-verify"])
            add("memory-note", mn, "ok" if mn["code"] == 0 else "refused")

    # --- 10. timeline record: a failed verification is a problem with a timestamp ------
    # Only failures create series entries. A green run does not close anything by itself:
    # closing needs someone to state why, with evidence, which is what `resolve` demands.
    # Auto-closing on green would be the tool lying on your behalf.
    if tl_init and not ran_ok and run_log:
        for l in run_log:
            if l["exit"] == 0:
                continue
            ev = f"{l['command']} | exit {l['exit']} | {l['seconds']}s"
            op = step([os.path.join(TOOLS, "timeline.py"), "-Mode", "open", "-Root", root,
                       "-Kind", "blocker", "-Title", f"验证失败：{l['command']}",
                       "-Detail", f"由 agent-verify 记录；plan.proven={str(proven).lower()}",
                       "-Evidence", ev])
            if op["code"] == 0:
                add("timeline-record", op, "opened")
                for ln in op["out"]:
                    print(ln)
                continue
            joined = " ".join(op["out"])
            m = re.search(r"已存在同标题条目 (\w+)", joined)
            if m:
                tc = step([os.path.join(TOOLS, "timeline.py"), "-Mode", "touch", "-Root", root,
                           "-Id", m.group(1), "-Evidence", ev])
                add("timeline-record", tc, "recurred")
                for ln in tc["out"]:
                    print(ln)
            else:
                add("timeline-record", op, "unknown")

    print(f"agent-verify: {verdict}  (proven={str(proven).lower()}, wall={total_run}s, "
          f"changed={len(changes)})")
    if a.get("brief"):
        failed = [l for l in run_log if l.get("exit") != 0]
        print(f"  commands: {len(run_log)} run, {len(failed)} failed")
        for l in failed:
            print(f"  FAIL {l['command']}  -> exit {l['exit']} in {l['seconds']}s")
    else:
        for l in run_log:
            print(f"  {l['command']}  -> exit {l['exit']} in {l['seconds']}s")
    if a.get("brief"):
        if changes:
            print(f"  changes: {len(changes)} (details in receipt)")
    else:
        for c in changes:
            print(f"  {c}")
    # Appended tail, appended receipt field only -- the existing output above is unchanged.
    if "resume_checkpoint" in doc:
        print(f"  resume checkpoint: {doc['resume_checkpoint']}")
    cost = doc.get("cost") or {}
    if cost:
        print(f"  cost: wall={cost.get('wall_s')}s +{cost.get('overhead_s')}s overhead "
              f"tokens={cost.get('tokens') if cost.get('tokens') is not None else '-'} "
              f"ledger={'recorded' if cost.get('recorded') else 'FAILED'} scope={cost.get('scope')}")
    return finish(verdict, 0 if ran_ok else 1)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
