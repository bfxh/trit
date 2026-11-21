#!/usr/bin/env python3
"""resume: per-task session ledger for suspend-and-continue ("out of budget" survival).

    resume.py begin    <task-id> [--budget-token N] [-Root <dir>]
    resume.py progress <task-id> <step-id> [--note s] [--proven|--unproven] [-Root <dir>]
    resume.py suspend  <task-id> --reason budget|deadline|manual [-Root <dir>]
    resume.py resume   <task-id> [-Root <dir>]   # state + a verbatim continuation block
    resume.py finish   <task-id> [-Root <dir>]
    resume.py status   <task-id> [-Root <dir>]
    resume.py list     [-Root <dir>]

-Root defaults to the current directory; the session lives at
<Root>/.agent/resume/<task-id>.json -- inside the repo it describes, never containing
a machine path (only ids, timestamps and states).

Why censoring is not failure: a task stopped mid-way because the budget ran out is an
observation that ended BEFORE the outcome, not an outcome. Survival analysis calls that
censoring. The ledger therefore has no "failed" status at all -- a task is

    open       in progress
    suspended  a censoring event was recorded, with a reason (budget/deadline/manual)
    finished   completed
    censored   closed without finishing -- explicitly not a failure

`resume` prints exactly what a fresh session needs to continue: the done steps, the
last recorded step, and a copy-verbatim ASCII command block. The person or model taking
over reads only that output.

State machine: begin -> open; suspend -> suspended (reason recorded); progress on a
suspended session implicitly resumes it first (printed, recorded in history); finish on
suspended -> censored, finish on open -> finished. Closed sessions refuse everything
except being viewed.

Fail-closed rules: begin on an existing session refuses (a ledger silently overwritten
is a ledger that lost its history); progress/suspend on a closed session refuses;
finish on a closed session refuses; unknown flags refuse (a typo must not drop a field
silently); task ids are [A-Za-z0-9][A-Za-z0-9._-]{0,63} so a session id can never be a
path.

Honest scope: the ledger records what the caller declares and what the tool observed.
`budget` is a declaration -- the tool layer cannot measure model-side token usage (this
is the same declaration-based gate agent-verify --budget-token applies). `journal_id`
is only the content hash of the latest journal snapshot (a stable handle a second
session can recompute), never the snapshot's contents.

Output is pure ASCII: the continuation block is meant to be pasted verbatim into a
fresh session; an encoding mishap would corrupt exactly the thing that must survive.

Exit codes: 0 ok; 2 refused (bad invocation, missing session, illegal state).
"""

import hashlib
import json
import os
import re
import sys
from datetime import datetime

MODES = ("begin", "progress", "suspend", "resume", "finish", "status", "list")
REASONS = ("budget", "deadline", "manual")
TASK_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
CORRUPT = object()  # sentinel: session file exists but is not valid JSON

# Flags that take a value vs flags that do not (proven/unproven are the only booleans).
VALUE_FLAGS = {"budget-token": "budget", "note": "note", "reason": "reason", "root": "root"}
BOOL_FLAGS = ("proven", "unproven")


def parse_args(argv):
    """Returns (args, error). Strict: an unknown flag is an error, not an ignored token."""
    out = {"mode": "", "task": "", "step": "", "root": "", "budget": "",
           "note": "", "reason": "", "proven": None}
    pos = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            pos.append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name in BOOL_FLAGS:
            out["proven"] = (name == "proven")
            i += 1
            continue
        if name not in VALUE_FLAGS:
            return None, f"unknown flag {a} (typo protection: nothing is dropped silently)"
        if i + 1 >= len(argv):
            return None, f"{a} needs a value"
        out[VALUE_FLAGS[name]] = argv[i + 1]
        i += 2
    if pos:
        out["mode"] = pos[0]
    if len(pos) > 1:
        out["task"] = pos[1]
    if len(pos) > 2:
        out["step"] = pos[2]
    if len(pos) > 3:
        return None, f"too many positional arguments ({len(pos)})"
    return out, None


def now_iso():
    return datetime.now().astimezone().isoformat(timespec="microseconds")


def session_path(root, task):
    return os.path.join(root, ".agent", "resume", f"{task}.json")


def load_session(root, task):
    p = session_path(root, task)
    if not os.path.isfile(p):
        return None
    try:
        with open(p, "r", encoding="utf-8-sig") as fh:
            return json.load(fh)
    except ValueError as e:
        print(f"resume: session file for '{task}' is corrupt (not valid JSON: {e})")
        return CORRUPT


def save_session(root, doc):
    # encoding="ascii" + ensure_ascii=True is the point, not a detail: the session file
    # must survive a reader that decodes bytes as ANSI (same rule as agent receipts).
    p = session_path(root, doc["task"])
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="ascii", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True, indent=4)
        fh.write("\n")


def journal_snapshot_id(root):
    """Id (never contents) of the latest journal snapshot: sha256 of its manifest.

    journal.py writes .journal/latest/manifest.json but assigns no id, so a content
    hash is the only stable handle a second session can recompute to confirm it is
    looking at the same snapshot. Only the 12-hex id is stored.
    """
    m = os.path.join(root, ".journal", "latest", "manifest.json")
    if not os.path.isfile(m):
        return None
    h = hashlib.sha256()
    with open(m, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()[:12]


def require_session(root, task):
    if not task:
        print("resume: a task-id is required")
        return None
    if not TASK_RE.match(task):
        print(f"resume: illegal task-id '{task}' -- use [A-Za-z0-9._-] only, "
              f"<= 64 chars (a session id must never be a path)")
        return None
    doc = load_session(root, task)
    if doc is CORRUPT:
        return None  # load_session already printed the refusal; exit 2 comes from the caller
    if doc is None:
        print(f"resume: no session '{task}' (expected .agent/resume/{task}.json under "
              f"-Root) -- run: resume.py begin {task}")
    return doc


def proven_word(v):
    return {True: "proven", False: "unproven", None: "unspecified"}[v]


def parse_budget(raw):
    """Returns (budget, error). budget is None when the flag was not given."""
    if raw == "":
        return None, None
    try:
        v = int(raw)
    except ValueError:
        return None, "--budget-token must be an integer"
    if v < 0:
        return None, "--budget-token must be >= 0 (it is a declaration, not a measurement)"
    return v, None


def fmt_budget(b):
    return "-" if b is None else str(b)


def summary_lines(doc):
    """The state half of the view; `status` prints this, `resume` prints it plus the block."""
    censor = doc.get("censor") or {}
    lines = [f"resume: task {doc['task']}  status={doc['status']}  "
             f"steps={len(doc.get('steps', []))}  budget={fmt_budget(doc.get('budget'))}"]
    if doc["status"] == "suspended":
        lines.append(f"resume: censored at {censor.get('at')}, reason={censor.get('reason')} "
                     f"-- censored is not a failure")
    if doc["status"] == "censored":
        lines.append("resume: this task was closed without finishing -- censored is not a failure")
    if doc.get("journal_id"):
        lines.append(f"resume: journal snapshot id {doc['journal_id']}")
    for s in doc.get("steps", []):
        note = f"  {s['note']}" if s.get("note") else ""
        lines.append(f"  - {s['step']}  {proven_word(s.get('proven'))}{note}")
    return lines


def continuation_block(doc):
    """The ASCII block a fresh session copies verbatim; nothing here is decoration.

    A placeholder (<next-step>) appears only where the ledger genuinely cannot know the
    answer (it records steps as they complete; it does not hold the plan). Everything
    else -- task id, flags, ordering -- is copy-ready as printed.
    """
    task = doc["task"]
    steps = doc.get("steps", [])
    done = ",".join(s["step"] for s in steps) or "none"
    lines = ["--- resume checkpoint (copy verbatim) ---"]
    if doc["status"] in ("finished", "censored"):
        lines.append(f"# task {task} is closed ({doc['status']}) -- nothing to continue")
        lines.append("# start a new task with a new task-id if there is more work")
    else:
        last = steps[-1] if steps else None
        if last is not None and last.get("proven") is False:
            lines.append(f"# task {task}: done={done}, last={last['step']} unproven")
            lines.append(f"python tools/resume.py progress {task} {last['step']} --proven "
                         f"# re-run it, then re-record it proven")
        elif last is not None:
            lines.append(f"# task {task}: done={done}, last={last['step']} proven")
            lines.append(f"python tools/resume.py progress {task} <next-step> --proven "
                         f"# <next-step>: the plan's next step (ledger does not hold the plan)")
        else:
            lines.append(f"# task {task}: no steps recorded yet")
            lines.append(f"python tools/resume.py progress {task} <step> --proven "
                         f"# <step>: the first step of the plan")
        lines.append(f"python tools/resume.py finish {task} "
                     f"# close the task when everything is done")
        lines.append(f"python tools/resume.py suspend {task} --reason budget "
                     f"# stop cleanly if the budget runs out again: censoring, not failing")
    lines.append("------------------------------------------")
    return lines


def cmd_begin(a, root):
    task = a["task"]
    if not task or not TASK_RE.match(task):
        print(f"resume: illegal task-id '{task}' -- use [A-Za-z0-9._-] only, <= 64 chars")
        return 2
    existing = load_session(root, task)
    if existing is CORRUPT:
        # load_session printed the diagnosis; a corrupt session is refuse-coverage, not
        # free space -- begin must not overwrite evidence of a half-finished run
        return 2
    if existing is not None:
        print(f"resume: session '{task}' already exists (status={existing['status']}) -- "
              f"refusing to overwrite; use resume/suspend/status, or a new task-id")
        return 2
    budget, err = parse_budget(a["budget"])
    if err:
        print(f"resume: {err}")
        return 2
    doc = {"task": task, "created": now_iso(), "status": "open", "budget": budget,
           "steps": [], "censor": None, "journal_id": None,
           "history": [{"event": "begin", "at": now_iso()}]}
    save_session(root, doc)
    print(f"resume: task {task} open (budget={fmt_budget(budget)})")
    return 0


def cmd_progress(a, root):
    doc = require_session(root, a["task"])
    if doc is None:
        return 2
    if not a["step"]:
        print("resume: a step-id is required")
        return 2
    if doc["status"] in ("finished", "censored"):
        print(f"resume: task {doc['task']} is closed ({doc['status']}) -- progress refuses")
        return 2
    if doc["status"] == "suspended":
        # Taking over a suspended session is an explicit event, not a silent one.
        reason = (doc.get("censor") or {}).get("reason", "unknown")
        doc["history"].append({"event": "resume", "at": now_iso(), "from_reason": reason})
        doc["censor"] = None
        doc["status"] = "open"
        print(f"resume: task {doc['task']} resumed (was suspended, reason={reason})")
    doc["steps"].append({"step": a["step"], "at": now_iso(),
                         "note": a["note"], "proven": a["proven"]})
    doc["history"].append({"event": "progress", "step": a["step"], "at": now_iso()})
    save_session(root, doc)
    print(f"resume: task {doc['task']} +{a['step']} ({proven_word(a['proven'])}) "
          f"steps={len(doc['steps'])}")
    return 0


def cmd_suspend(a, root):
    doc = require_session(root, a["task"])
    if doc is None:
        return 2
    if doc["status"] in ("finished", "censored"):
        print(f"resume: task {doc['task']} is closed ({doc['status']}) -- suspend refuses")
        return 2
    if a["reason"] not in REASONS:
        print(f"resume: --reason must be one of {list(REASONS)}")
        return 2
    at = now_iso()
    doc["status"] = "suspended"
    doc["censor"] = {"reason": a["reason"], "at": at}
    jid = journal_snapshot_id(root)
    if jid:
        doc["journal_id"] = jid
    doc["history"].append({"event": "suspend", "reason": a["reason"], "at": at})
    save_session(root, doc)
    print(f"resume: task {doc['task']} suspended (reason={a['reason']}) -- "
          f"censored, not a failure")
    if jid:
        print(f"resume: journal snapshot id {jid}")
    print(f"resume: continue later with: resume.py resume {doc['task']}")
    return 0


def cmd_resume(a, root):
    doc = require_session(root, a["task"])
    if doc is None:
        return 2
    # Mostly a read-only view; the one permitted write is the journal snapshot id, which
    # the spec assigns to this command ("resume/suspend 时探测 journal 快照 id").
    jid = journal_snapshot_id(root)
    if jid and jid != doc.get("journal_id"):
        doc["journal_id"] = jid
        save_session(root, doc)
    for ln in summary_lines(doc):
        print(ln)
    print("")
    for ln in continuation_block(doc):
        print(ln)
    return 0


def cmd_finish(a, root):
    doc = require_session(root, a["task"])
    if doc is None:
        return 2
    if doc["status"] in ("finished", "censored"):
        print(f"resume: task {doc['task']} is closed ({doc['status']}) -- finish refuses")
        return 2
    at = now_iso()
    if doc["status"] == "suspended":
        reason = (doc.get("censor") or {}).get("reason", "unknown")
        doc["status"] = "censored"
        doc["history"].append({"event": "finish", "outcome": "censored",
                               "reason": reason, "at": at})
        save_session(root, doc)
        print(f"resume: task {doc['task']} closed as censored (suspended, reason={reason}) "
              f"-- censored is not a failure")
    else:
        doc["status"] = "finished"
        doc["history"].append({"event": "finish", "outcome": "finished", "at": at})
        save_session(root, doc)
        print(f"resume: task {doc['task']} finished (steps={len(doc['steps'])})")
    return 0


def cmd_status(a, root):
    doc = require_session(root, a["task"])
    if doc is None:
        return 2
    for ln in summary_lines(doc):
        print(ln)
    return 0


def cmd_list(a, root):
    d = os.path.join(root, ".agent", "resume")
    if not os.path.isdir(d):
        print("resume: no sessions (no .agent/resume yet)")
        return 0
    names = sorted(f for f in os.listdir(d) if f.endswith(".json"))
    if not names:
        print("resume: no sessions")
        return 0
    print(f"resume: {len(names)} session(s) under .agent/resume")
    for f in names:
        try:
            with open(os.path.join(d, f), "r", encoding="utf-8-sig") as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            print(f"  {f}  unreadable -- skipped (a bad file must not kill the listing)")
            continue
        censor = (doc.get("censor") or {}).get("reason", "-")
        created = str(doc.get("created", ""))[:19]
        print(f"  {doc.get('task', f):<24} {doc.get('status', '?'):<10} "
              f"steps={len(doc.get('steps', [])):<3} budget={fmt_budget(doc.get('budget')):<8} "
              f"censor={censor:<8} created={created}")
    return 0


def main(argv):
    a, err = parse_args(argv)
    if err:
        print(f"resume: {err}")
        return 2
    if a["mode"] not in MODES:
        print(f"resume: mode must be one of {list(MODES)}")
        return 2
    root = os.path.abspath(a["root"]) if a["root"].strip() else os.getcwd()
    if a["mode"] == "begin":
        return cmd_begin(a, root)
    if a["mode"] == "progress":
        return cmd_progress(a, root)
    if a["mode"] == "suspend":
        return cmd_suspend(a, root)
    if a["mode"] == "resume":
        return cmd_resume(a, root)
    if a["mode"] == "finish":
        return cmd_finish(a, root)
    if a["mode"] == "status":
        return cmd_status(a, root)
    return cmd_list(a, root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
