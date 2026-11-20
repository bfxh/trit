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

import json
import os
import re
import subprocess
import sys
import time
import uuid
from datetime import datetime

LANGS = {"rust", "python", "node", "go"}
PROFILES = {"read-only", "workspace-write", "full"}

# The receipt may be committed with the repo, so the same shapes privacy_scan hunts for
# are refused at the door of the one field a caller types by hand.
MACHINE_PATH_RX = re.compile(
    r"[A-Za-z]:[\\/]|/Users/[A-Za-z0-9._-]+/|/home/[A-Za-z0-9._-]+/|/mnt/[a-z]/")

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
RESUME = os.path.join(TOOLS, "resume.py")
RUN_CAPS = os.path.join(TOOLS, "run_caps.py")

BUDGET_NOTE = ("declaration-based gate: the tool layer cannot measure model-side token "
               "usage; --budget-token is the caller's declared remaining budget "
               "(continuation of the ENTRY-README boundary #3: honest scope statements "
               "travel with the tool)")
CAPS_NOTE = ("kernel-level resource caps (memory and process count enforced by the "
             "Windows job object, RLIMIT_AS on POSIX); NOT a security boundary: no "
             "filesystem/network isolation, no defense against deliberate escape; "
             "complements policy -- policy pre-classifies commands to stop misuse, "
             "run-caps bounds the damage if a permitted command runs away")

HELP = f"""agent-verify: chain preflight -> policy -> run -> journal, one audited receipt.

  agent-verify.sh -Root <dir> -Changed <files> -Lang rust|python|node|go
                    [-Profile read-only|workspace-write|full] [-DryRun]
                    [-Receipt out.json] [-Store <dir>] [-Plan plan.json]
  budget gate:  --budget-token N --task-id <id>
  caps:         --caps [--caps-mem-mb N] [--caps-procs N] [--caps-seconds S]
  subagent:     --agent-id <id>

Subagent usage: a subagent running a task passes its own id (--agent-id sub-7). The id is
recorded in the receipt (top level and every run[] line, appended fields only -- old
receipt fields untouched) and, via `ledger.py add --agent-id`, on each cost line. With no
flag the receipt is exactly what it always was.

Budget gate (honest scope): the tool layer cannot measure model-side token usage.
--budget-token N is a DECLARATION by the caller. A declared value <= 0 refuses before
run, suspends the session (reason=budget, censored -- not a failure) and prints the
continuation block. No flag, no gate.

Caps (honest scope): {CAPS_NOTE}
Defaults when --caps is given without parameters: mem 2048 MB, 64 procs, 1800 s.

Exit codes: 0 all good; 1 a guard refused or the command failed; 2 bad invocation.
"""


def parse_args(argv):
    out = {"root": "", "changed": [], "lang": "", "profile": "workspace-write",
           "allow_network": False, "dry_run": False, "receipt": "", "store": "", "plan": "",
           "allow": [], "agent_id": ""}
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
        if name in ("dryrun", "allownetwork", "caps"):
            out["dry_run" if name == "dryrun"
                else "allow_network" if name == "allownetwork" else "caps"] = True
            i += 1
            continue
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if name == "changed":
            out["changed"].extend([x for x in val.split(",") if x.strip()])
        elif name == "allow":
            out["allow"].extend([x for x in val.replace(",", " ").split() if x])
        else:
            out[{"root": "root", "lang": "lang", "profile": "profile", "receipt": "receipt",
                 "store": "store", "plan": "plan", "agent-id": "agent_id",
                 "agentid": "agent_id"}.get(name, name)] = val
        i += 2
    return out


def step(args):
    """Run one tool module and report (output lines, exit code, seconds)."""
    t0 = time.time()
    p = subprocess.run([PY] + args, capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    lines = ((p.stdout or "") + (p.stderr or "")).splitlines()
    return {"out": lines, "code": p.returncode, "seconds": round(time.time() - t0, 2)}


def skipped(msg):
    return {"out": [msg], "code": 0, "seconds": 0}


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


def write_receipt(path, doc):
    # ensure_ascii=True is the point, not a detail: a reader that decodes UTF-8 as ANSI can
    # mangle Chinese and even break the closing quote, turning a valid receipt into garbage.
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True, indent=2)
        fh.write("\n")


def main(argv):
    if "--help" in argv or "-h" in argv:
        print(HELP)
        return 0
    a = parse_args(argv)
    root_in = a["root"].strip()
    if not root_in or not os.path.isdir(root_in):
        print("agent-verify: -Root 必须是已存在的目录")
        return 2
    if not a["changed"]:
        print("agent-verify: -Changed 至少要给一个文件")
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

    root = os.path.abspath(root_in)
    lang = a["lang"].lower()
    profile = a["profile"].lower()
    task_id = str(a.get("task-id", "")).strip()
    store = a["store"].strip() or os.path.join(
        os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "agent-verify",
        root.replace("\\", "_").replace(":", "_"))

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
        if a["receipt"]:
            write_receipt(a["receipt"], doc)
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

    # --- 2.5 budget gate: suspend before run when the declared budget is empty ----------
    # Honest by construction: no tool can measure the model's remaining tokens, so this
    # gate acts only on the caller's declaration (BUDGET_NOTE travels in the receipt).
    # A declared value <= 0 means "do not even start": the session is suspended with
    # reason=budget -- censoring, which is not failure -- and the continuation block is
    # printed for whoever picks the task up in a fresh session.
    if "budget-token" in a:
        try:
            budget = int(a["budget-token"])
        except ValueError:
            print("agent-verify: --budget-token must be an integer")
            return 2
        doc["budget"] = {"declared": budget, "gate": BUDGET_NOTE}
        if budget <= 0:
            print(f"agent-verify: budget gate -- declared {budget} <= 0, suspend before run")
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

    # --- 4. affected plan: subset only when it carries a proof --------------------------
    if a["plan"]:
        # replaying a plan is allowed on purpose: it is how the chain is tested without
        # invoking a real build, and it keeps the run/journal steps observable.
        plan_path = a["plan"]
        ap = skipped(f"affected: 使用给定 plan ({plan_path})")
    else:
        plan_path = os.path.join(os.environ.get("TEMP", "/tmp"),
                                 f"av-plan-{uuid.uuid4().hex[:8]}.json")
        ap = step([os.path.join(TOOLS, "affected.py"), "-Path", root,
                   "-Changed", ",".join(a["changed"]), "-Lang", lang, "-Out", plan_path])
    add("affected", ap, "ok")
    plan = None
    if os.path.exists(plan_path):
        with open(plan_path, "r", encoding="utf-8-sig") as fh:
            plan = json.load(fh)
    commands = plan.get("commands", []) if plan else []
    proven = bool(plan.get("proven")) if plan else False
    doc["plan"] = {"proven": proven, "commands": commands,
                   "unproven": plan.get("unproven", ["plan unavailable"]) if plan else ["plan unavailable"]}

    # --- 4. policy: are we even allowed to run it --------------------------------------
    policy_log, policy_ok = [], True
    for cmd in commands:
        pa = [os.path.join(TOOLS, "policy.py"), "-Profile", profile, "-Command", cmd, "-Root", root]
        if a["allow_network"]:
            pa.append("-AllowNetwork")
        pr = step(pa)
        policy_log.append({"command": cmd, "exit": pr["code"], "output": pr["out"][-3:]})
        if pr["code"] != 0:
            policy_ok = False
    doc["policy"] = policy_log
    add("policy", {"out": [], "code": 0 if policy_ok else 1, "seconds": 0},
        "ok" if policy_ok else "refused")
    if not policy_ok:
        print(f"agent-verify: 拒绝 -- 权限档 {profile} 不允许这些命令")
        return finish("refused by policy", 1)

    # --- 5. constraints: is this change allowed at all ---------------------------------
    c_init = os.path.exists(os.path.join(root, ".agent", "constraints.json"))
    ck = (step([os.path.join(TOOLS, "constraints.py"), "-Mode", "check", "-Root", root]
               + (["-Allow", " , ".join(a["allow"])] if a["allow"] else []))
          if c_init else skipped("constraints: 未初始化（没有 .agent/constraints.json），跳过"))
    ck_verdict = ("ok" if c_init else "skipped") if ck["code"] == 0 else "refused"
    add("constraints", ck, ck_verdict)
    if ck["code"] != 0:
        for ln in ck["out"]:
            print(ln)
        print("agent-verify: 拒绝 -- 改动触碰了本仓的禁止操作（见上）")
        return finish("refused by constraints", 1)

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
    add("journal-end", je, "clean" if je["code"] == 0 else "changed")
    doc["changes"] = changes
    doc["wall_s"] = total_run

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
    for l in run_log:
        print(f"  {l['command']}  -> exit {l['exit']} in {l['seconds']}s")
    for c in changes:
        print(f"  {c}")
    # Appended tail, appended receipt field only -- the existing output above is unchanged.
    if "resume_checkpoint" in doc:
        print(f"  resume checkpoint: {doc['resume_checkpoint']}")
    return finish(verdict, 0 if ran_ok else 1)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
