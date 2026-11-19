#!/usr/bin/env python3
"""agent-verify: the single entry point that chains preflight -> policy -> run -> journal.

    agent_verify.py -Root D:\\proj -Changed src/a.rs -Lang rust -Profile workspace-write
    agent_verify.py -Root D:\\proj -Changed pkg/core.py -Lang python -DryRun

Why one entry point: four scripts that each do one thing correctly still fail as a system if
the caller has to remember the order. The order IS the safety property --

    memory-brief  (what this project already knows)
    timeline-due  (which temporary problems have come due)
    preflight     (nothing is broken before we start)
    affected      (the subset, only if it carries a proof)
    policy        (we are allowed to run this at all)
    constraints   (this change is allowed at all)
    journal-begin (we can prove and undo what it writes)
    run           (only now)
    journal-end   (what it actually wrote)
    memory-note   (a durable, evidenced trace)
    timeline-record (a failure is a problem with a timestamp)

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

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable


def parse_args(argv):
    out = {"root": "", "changed": [], "lang": "", "profile": "workspace-write",
           "allow_network": False, "dry_run": False, "receipt": "", "store": "", "plan": "",
           "allow": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name in ("dryrun", "allownetwork"):
            out["dry_run" if name == "dryrun" else "allow_network"] = True
            i += 1
            continue
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if name == "changed":
            out["changed"].extend([x for x in val.split(",") if x.strip()])
        elif name == "allow":
            out["allow"].extend([x for x in val.replace(",", " ").split() if x])
        else:
            out[{"root": "root", "lang": "lang", "profile": "profile", "receipt": "receipt",
                 "store": "store", "plan": "plan"}.get(name, name)] = val
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


def write_receipt(path, doc):
    # ensure_ascii=True is the point, not a detail: a reader that decodes UTF-8 as ANSI can
    # mangle Chinese and even break the closing quote, turning a valid receipt into garbage.
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True, indent=2)
        fh.write("\n")


def main(argv):
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

    root = os.path.abspath(root_in)
    lang = a["lang"].lower()
    profile = a["profile"].lower()
    store = a["store"].strip() or os.path.join(
        os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "agent-verify",
        root.replace("\\", "_").replace(":", "_"))

    doc = {"root": root, "lang": lang, "profile": profile, "changed": a["changed"],
           "at": datetime.now().astimezone().isoformat(timespec="microseconds")}
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
    run_log, ran_ok = [], True
    for cmd in commands:
        r = step([os.path.join(TOOLS, "_run.py"), "-Root", root, "-Command", cmd])
        run_log.append({"command": cmd, "exit": r["code"], "seconds": r["seconds"],
                        "tail": r["out"][-5:]})
        if r["code"] != 0:
            ran_ok = False
    doc["run"] = run_log
    total_run = round(sum(x["seconds"] for x in run_log), 2)
    add("run", {"out": [], "code": 0 if ran_ok else 1, "seconds": total_run},
        "ok" if ran_ok else "failed")

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
    return finish(verdict, 0 if ran_ok else 1)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
