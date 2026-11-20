#!/usr/bin/env python3
"""watch: the event ledger and the observation face -- what happened, by whom, at which
turn position, and whether the whole board is still healthy.

    watch.py record <event> [-AgentId <s>] [-Turn N] [-Ok | -Fail] [-Note <s>] -Root <dir>
    watch.py report [--Json] [-AgentId <s>] -Root <dir>
    watch.py health [--Json] -Root <dir>
    watch.py pollution [--Json] -Root <dir>

Events append to <Root>/.agent/watch.jsonl, one JSON object per line, always the same six
keys: {ts, event, agent_id, turn, verdict, note}. Event labels are free -- record accepts
any word, because the harness invents vocabulary faster than a whitelist could age.
`report` counts only RECOGNIZED outcome labels (cmd/test/build/check/commit/verify) that
carry a verdict, so free labels cannot pollute the denominators.

report -- the fatigue face. Failure rate bucketed by turn position (early = turn 1..10,
late = turn 25+), commands per turn, consecutive fail streaks. When the late bucket has
enough observations and its fail rate rises over the early bucket by FATIGUE_RISE_PP
points, it prints one WARN line addressed to the harness:
"fatigue suspected at turn N (fail-rate x% vs y% early) -- checkpoint & resume in a fresh
turn".

Honest boundary: a tool cannot see the model, only its traces. The fatigue line is
behavioral evidence inference, not mind-reading -- outcome rates by turn position, nothing
more. It is a checkpoint reminder for the harness, not a diagnosis of the model. The same
events are the corpus for anyone who actually wants to study the model.

health -- the whole-board face. Four alarm surfaces, each with the consequence of keeping
going: a trailing fail streak; git status dirty drift; a giant diff; days since the last
commit while changes sit in the tree. Exit 1 when any alarm fires.

pollution -- picks pollution patterns out of the event stream and lists them: commits that
mention --no-verify, giant diffs recorded as changed-lines=N, and refused-then-bypass
chains (a refusal marker followed within REFUSAL_WINDOW events by a bypass marker).
Conventions it greps for are literal flags people type, not a schema. Division of labor:
watch observes and reminds; policy.py/constraints.py are the ones that block. Nothing here
refuses a command -- exit 1 only means "there is something to look at".

Privacy: the event file lives inside the repo and may be committed, so record refuses
machine-absolute paths (drive letters, /Users/, /home/, /mnt/) in event, agent id and note.

Exit codes: 0 clean / no warn; 1 findings (report WARN, health alarms, pollution hits);
2 bad invocation.
"""

import glob
import json
import os
import re
import subprocess
import sys
from datetime import datetime

MODES = {"record", "report", "health", "pollution"}

RECOGNIZED = ("cmd", "test", "build", "check", "commit", "verify")

EARLY_MAX_TURN = 10
LATE_MIN_TURN = 25
MIN_BUCKET_OBS = 4
FATIGUE_RISE_PP = 20.0
FAIL_STREAK_ALARM = 3
DIRTY_PATHS_ALARM = 25
GIANT_DIFF_LINES = 600
UNCOMMITTED_DAYS_ALARM = 2.0
REFUSAL_WINDOW = 10

REFUSAL_MARKS = ("refused", "rejected", "blocked", "denied")
BYPASS_MARKS = ("--no-verify", "--force", "bypass", "workaround", "skip-checks", "skip-hooks")
GIANT_TOKEN_RX = re.compile(r"changed-lines=(\d+)")
# The event file is committable, so the same shapes privacy_scan hunts for are refused at
# the door -- a note that carries a home directory must never reach the ledger.
MACHINE_PATH_RX = re.compile(
    r"[A-Za-z]:[\\/]|/Users/[A-Za-z0-9._-]+/|/home/[A-Za-z0-9._-]+/|/mnt/[a-z]/")

BOUNDARY = ("fatigue is inferred from behavioral evidence (outcome rates by turn position) "
            "-- behavioral evidence inference, not mind-reading; a checkpoint reminder for "
            "the harness, not a diagnosis of the model")
DIVISION = ("watch observes and reminds; policy/constraints block -- nothing here refuses "
            "a command, exit 1 only means there is something to look at")


def parse_args(argv):
    out = {"mode": "", "event": "", "agent_id": "", "turn": "", "note": "",
           "verdict": "", "json": False, "extra": []}
    positional = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            positional.append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name in ("ok", "fail"):
            # last one wins would hide a contradiction: --ok --fail is an unclear record
            if out["verdict"] and out["verdict"] != ("ok" if name == "ok" else "fail"):
                out["verdict_conflict"] = True
            out["verdict"] = "ok" if name == "ok" else "fail"
            i += 1
            continue
        if name == "json":
            out["json"] = True
            i += 1
            continue
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[{"agent-id": "agent_id", "agentid": "agent_id"}.get(name, name)] = val
        i += 2
    if positional and positional[0] in MODES:
        out["mode"] = positional[0]
        if len(positional) > 1:
            out["event"] = positional[1]
        out["extra"] = positional[2:]
    return out


def refuse(msg):
    print(f"watch: refuse -- {msg}")
    return 1


def site(root):
    return os.path.join(root, ".agent", "watch.jsonl")


def append_event(root, ev):
    path = site(root)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "a", encoding="utf-8", newline="\n") as fh:
        fh.write(json.dumps(ev, ensure_ascii=True) + "\n")


def load_events(root):
    """(events, malformed_lines). A truncated tail line (crash mid-append) is counted and
    skipped, never silently dropped and never fatal for the analysis faces."""
    path = site(root)
    if not os.path.exists(path):
        return [], 0
    events, bad = [], 0
    with open(path, "r", encoding="utf-8-sig") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                doc = json.loads(line)
            except ValueError:
                bad += 1
                continue
            if isinstance(doc, dict):
                events.append(doc)
            else:
                bad += 1
    return events, bad


def counted(events):
    """The events report's math consumes: a recognized outcome label with an explicit
    verdict. Free labels are kept in the file for study, but never enter a denominator."""
    return [e for e in events
            if str(e.get("event", "")) in RECOGNIZED and e.get("verdict") in ("ok", "fail")]


def rate_pct(fails, obs):
    return round(100.0 * fails / obs, 1) if obs else 0.0


def analyze(events):
    """One pass over counted events: buckets, per-turn load, streaks. Turn-less events
    join the streaks but not the position buckets -- position is the fatigue axis."""
    obs = {"early_n": 0, "early_f": 0, "late_n": 0, "late_f": 0}
    per_turn, max_turn = {}, 0
    streak = longest = trailing = 0
    agents = {}
    for e in counted(events):
        agents[str(e.get("agent_id", "")) or "-"] = agents.get(str(e.get("agent_id", "")) or "-", 0) + 1
        t = e.get("turn")
        if isinstance(t, int) and not isinstance(t, bool):
            if t < 0:
                t = 0
            per_turn[t] = per_turn.get(t, 0) + 1
            max_turn = max(max_turn, t)
            if t <= EARLY_MAX_TURN:
                obs["early_n"] += 1
                obs["early_f"] += e["verdict"] == "fail"
            elif t >= LATE_MIN_TURN:
                obs["late_n"] += 1
                obs["late_f"] += e["verdict"] == "fail"
        if e["verdict"] == "fail":
            streak += 1
            longest = max(longest, streak)
            trailing += 1
        else:
            streak = 0
            trailing = 0
    obs["early_rate"] = rate_pct(obs["early_f"], obs["early_n"])
    obs["late_rate"] = rate_pct(obs["late_f"], obs["late_n"])
    return obs, per_turn, max_turn, longest, trailing, agents


def fatigue_warning(obs, max_turn):
    """Fires only with enough observations in both buckets: one late fail against one early
    ok is noise, and calling that fatigue would teach the harness to ignore the warning."""
    if (obs["early_n"] < MIN_BUCKET_OBS or obs["late_n"] < MIN_BUCKET_OBS
            or obs["late_rate"] - obs["early_rate"] < FATIGUE_RISE_PP):
        return None
    text = (f"fatigue suspected at turn {max_turn} (fail-rate {obs['late_rate']}% vs "
            f"{obs['early_rate']}% early) -- checkpoint & resume in a fresh turn")
    return {"fired": True, "turn": max_turn, "late_rate": obs["late_rate"],
            "early_rate": obs["early_rate"], "text": text}


def cmd_record(a, root):
    if a.get("verdict_conflict"):
        return refuse("--ok and --fail together is an unclear record")
    if not a["event"].strip():
        return refuse("record needs an event label (a free word, no whitelist)")
    if a["extra"]:
        return refuse("record takes exactly one event label: " + ", ".join(a["extra"]))
    agent = a["agent_id"].strip()
    note = a["note"].strip()
    for label, val in (("event", a["event"]), ("agent id", agent), ("note", note)):
        if MACHINE_PATH_RX.search(val):
            return refuse(f"{label} carries a machine-absolute path -- the event file is "
                          "committed with the repo, keep it repo-relative")
    turn = None
    if str(a["turn"]).strip():
        try:
            turn = int(str(a["turn"]).strip())
        except ValueError:
            return refuse("--turn must be an integer")
        if turn < 0:
            return refuse("--turn must be >= 0")
    ev = {"ts": datetime.now().astimezone().isoformat(timespec="seconds"),
          "event": a["event"].strip(), "agent_id": agent, "turn": turn,
          "verdict": a["verdict"], "note": note}
    append_event(root, ev)
    tail = (f" turn={turn}" if turn is not None else "") + \
           (f" agent={agent}" if agent else "") + \
           (f" verdict={a['verdict']}" if a["verdict"] else "")
    print(f"watch: recorded {ev['event']}{tail}")
    return 0


def cmd_report(a, root):
    events, bad = load_events(root)
    agent = a["agent_id"].strip()
    if agent:
        events = [e for e in events if str(e.get("agent_id", "")) == agent]
    obs, per_turn, max_turn, longest, trailing, agents = analyze(events)
    warn = fatigue_warning(obs, max_turn)
    n = len(counted(events))
    counts = sorted(per_turn.values())
    median = counts[int(len(counts) / 2)] if counts else 0
    peak_turn = max(per_turn, key=per_turn.get) if per_turn else 0
    doc = {"mode": "report", "agent_filter": agent, "events_total": len(events),
           "counted": n, "skipped_malformed": bad, "recognized": list(RECOGNIZED),
           "early": {"turns": f"1-{EARLY_MAX_TURN}", "obs": obs["early_n"],
                     "fails": obs["early_f"], "fail_rate": obs["early_rate"]},
           "late": {"turns": f"{LATE_MIN_TURN}+", "obs": obs["late_n"],
                    "fails": obs["late_f"], "fail_rate": obs["late_rate"]},
           "commands_per_turn": {"median": median, "max": max(per_turn.values()) if per_turn else 0,
                                 "at_turn": peak_turn},
           "fail_streak": {"longest": longest, "trailing": trailing},
           "agents": agents, "warn": warn or {"fired": False}, "boundary": BOUNDARY}
    if a["json"]:
        print(json.dumps(doc, ensure_ascii=True))
        return 1 if warn else 0
    print(f"watch: report counted={n}/{len(events)} skipped_malformed={bad}"
          + (f" agent={agent}" if agent else ""))
    print(f"watch: early turns 1-{EARLY_MAX_TURN}: obs={obs['early_n']} fails={obs['early_f']} "
          f"fail-rate={obs['early_rate']}%")
    print(f"watch: late turns {LATE_MIN_TURN}+: obs={obs['late_n']} fails={obs['late_f']} "
          f"fail-rate={obs['late_rate']}%")
    print(f"watch: commands per turn: median={median} max={max(per_turn.values()) if per_turn else 0}"
          + (f" (turn {peak_turn})" if per_turn else ""))
    print(f"watch: fail streak: longest={longest} trailing={trailing}")
    if agents:
        print("watch: agents: " + ", ".join(f"{k}={v}" for k, v in sorted(agents.items())))
    if warn:
        print(f"watch: WARN {warn['text']}")
    print(f"watch: boundary: {BOUNDARY}")
    return 1 if warn else 0


def git_out(root, args):
    try:
        p = subprocess.run(["git", "-C", root] + args, capture_output=True, text=True,
                           encoding="utf-8", errors="replace")
    except OSError:
        return None, ""
    return p.returncode, p.stdout


def load_sessions(root):
    """Resume sessions are the receipt-adjacent record of failed/suspended runs."""
    out = []
    for path in glob.glob(os.path.join(root, ".agent", "resume", "*.json")):
        try:
            with open(path, "r", encoding="utf-8-sig") as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            continue
        if isinstance(doc, dict):
            out.append(doc)
    return out


def cmd_health(a, root):
    events, bad = load_events(root)
    alarms, context = [], []
    obs, _per_turn, _max_turn, longest, trailing, _agents = analyze(events)
    if trailing >= FAIL_STREAK_ALARM:
        alarms.append({
            "kind": "fail-streak",
            "detail": f"{trailing} consecutive failures (longest {longest})",
            "consequence": ("every next command builds on a red base -- checkpoint and fix "
                            "before continuing, or the failures compound")})
    context.append(f"watch events={len(events)} counted={len(counted(events))} skipped={bad}")

    rc, por = git_out(root, ["status", "--porcelain"])
    if rc != 0:
        context.append("not a git work tree (or git unavailable) -- git alarms skipped")
    else:
        dirty = [l for l in por.splitlines() if l.strip()]
        context.append(f"dirty paths={len(dirty)}")
        if len(dirty) >= DIRTY_PATHS_ALARM:
            alarms.append({
                "kind": "dirty-drift",
                "detail": f"{len(dirty)} dirty paths in git status",
                "consequence": ("dirty drift piles up -- when the budget runs out mid-task, "
                                "the scene cannot be restored cleanly; commit or checkpoint "
                                "in small slices")})
        rc2, numstat = git_out(root, ["diff", "HEAD", "--numstat"])
        if rc2 == 0:
            added = deleted = files = 0
            for line in numstat.splitlines():
                parts = line.split("\t")
                if len(parts) < 3:
                    continue
                files += 1
                added += int(parts[0]) if parts[0].isdigit() else 0
                deleted += int(parts[1]) if parts[1].isdigit() else 0
            context.append(f"diff vs HEAD: +{added}/-{deleted} across {files} files")
            if added + deleted >= GIANT_DIFF_LINES:
                alarms.append({
                    "kind": "giant-diff",
                    "detail": f"+{added}/-{deleted} lines across {files} files",
                    "consequence": ("a diff this size cannot be reviewed or reverted "
                                    "piecewise -- one rollback loses the good parts with "
                                    "the bad; split it before piling more on")})
        rc3, last = git_out(root, ["log", "-1", "--format=%ct"])
        if rc3 == 0 and last.strip().isdigit():
            days = round((datetime.now().timestamp() - int(last.strip())) / 86400.0, 1)
            context.append(f"last commit {days} days ago")
            if days >= UNCOMMITTED_DAYS_ALARM and dirty:
                alarms.append({
                    "kind": "uncommitted",
                    "detail": f"last commit {days} days ago with {len(dirty)} dirty paths",
                    "consequence": ("work that exists only in the working tree can be erased "
                                    "by one bad command -- commit or checkpoint before "
                                    "piling more on")})

    sessions = load_sessions(root)
    manual = sum(1 for s in sessions
                 if s.get("status") == "suspended" and (s.get("censor") or {}).get("reason") == "manual")
    budget = sum(1 for s in sessions
                 if s.get("status") == "suspended" and (s.get("censor") or {}).get("reason") == "budget")
    context.append(f"sessions: {manual} suspended(manual), {budget} suspended(budget)")
    man_path = os.path.join(root, ".journal", "latest", "manifest.json")
    if os.path.exists(man_path):
        try:
            with open(man_path, "r", encoding="utf-8-sig") as fh:
                man = json.load(fh)
            # manifest["root"] is a machine-absolute path -- count and timestamp only
            context.append(f"journal snapshot: {man.get('count', '?')} files at {man.get('at', '?')}")
        except (OSError, ValueError):
            context.append("journal snapshot: unreadable")

    doc = {"mode": "health", "alarms": alarms, "context": context, "division": DIVISION}
    if a["json"]:
        print(json.dumps(doc, ensure_ascii=True))
        return 1 if alarms else 0
    print(f"watch: health alarms={len(alarms)}")
    for al in alarms:
        print(f"watch: ALARM {al['kind']}: {al['detail']} -- {al['consequence']}")
    for c in context:
        print(f"watch: context: {c}")
    print(f"watch: division of labor: {DIVISION}")
    return 1 if alarms else 0


def event_text(e):
    return f"{e.get('event', '')} {e.get('note', '')}".lower()


def cmd_pollution(a, root):
    events, bad = load_events(root)
    findings = []
    for e in events:
        txt = event_text(e)
        note = str(e.get("note", ""))
        if "--no-verify" in txt:
            findings.append({"kind": "no-verify", "at": e.get("ts", ""), "event": str(e.get("event", "")),
                             "detail": note[:120],
                             "consequence": ("hooks were skipped: what they would have caught "
                                             "is now in the tree and resurfaces later at a "
                                             "higher price")})
        m = GIANT_TOKEN_RX.search(note)
        if (m and int(m.group(1)) >= GIANT_DIFF_LINES) or str(e.get("event", "")) == "giant-diff":
            findings.append({"kind": "giant-diff", "at": e.get("ts", ""), "event": str(e.get("event", "")),
                             "detail": (m.group(0) if m else "giant-diff label"),
                             "consequence": ("too big to review or revert piecewise -- one "
                                             "rollback loses the good parts with the bad")})
    for i, e in enumerate(events):
        txt = event_text(e)
        if not any(mark in txt for mark in BYPASS_MARKS):
            continue
        for j in range(max(0, i - REFUSAL_WINDOW), i):
            if any(mark in event_text(events[j]) for mark in REFUSAL_MARKS):
                findings.append({"kind": "refused-then-bypass", "at": e.get("ts", ""),
                                 "event": str(e.get("event", "")),
                                 "detail": f"after '{events[j].get('event', '')}' at {events[j].get('ts', '')}",
                                 "consequence": ("a refusal is a boundary, not an obstacle -- "
                                                 "rerouting around it turns a guarded error "
                                                 "into an unguarded one")})
                break
    doc = {"mode": "pollution", "events": len(events), "skipped_malformed": bad,
           "findings": findings, "division": DIVISION}
    if a["json"]:
        print(json.dumps(doc, ensure_ascii=True))
        return 1 if findings else 0
    print(f"watch: pollution events={len(events)} findings={len(findings)}")
    for f in findings:
        print(f"watch: POLLUTION {f['kind']}: '{f['event']}' {f['detail']} -- {f['consequence']}")
    print(f"watch: division of labor: {DIVISION}")
    return 1 if findings else 0


def usage(code):
    print("""watch: event ledger + observation face -- record what happened, analyse fatigue,
watch the whole board, list pollution patterns.

usage:
  tools/watch.py record <event> [-AgentId <s>] [-Turn N] [-Ok | -Fail] [-Note <s>] -Root <dir>
  tools/watch.py report  [--Json] [-AgentId <s>] -Root <dir>
  tools/watch.py health  [--Json] -Root <dir>
  tools/watch.py pollution [--Json] -Root <dir>

site: <Root>/.agent/watch.jsonl, one JSON object per line {ts, event, agent_id, turn,
verdict, note}. Event labels are free (no whitelist); report counts only recognized
outcome labels with a verdict: cmd/test/build/check/commit/verify.

report: fatigue analysis. Buckets by turn position (early 1..10, late 25+); when both
buckets have >= 4 observations and the late fail rate rises >= 20 points over early:
WARN "fatigue suspected at turn N (...) -- checkpoint & resume in a fresh turn".

Honest boundary: """ + BOUNDARY + """

pollution conventions (grepped literally from event+note): --no-verify commits;
changed-lines=N with N >= 600 (or the event label giant-diff); a refusal marker
(refused/rejected/blocked/denied) followed within 10 events by a bypass marker
(--no-verify/--force/bypass/workaround/skip-checks/skip-hooks).

Division of labor: """ + DIVISION + """

record refuses machine-absolute paths (drive letters, /Users/, /home/, /mnt/) in event,
agent id and note: the event file is committed with the repo.

Exit codes: 0 clean; 1 findings (report WARN / health alarms / pollution hits); 2 bad
invocation.""")
    return code


def main(argv):
    a = parse_args(argv)
    if not a["mode"]:
        return usage(2)
    root_in = a.get("root", "").strip()
    if not root_in or not os.path.isdir(root_in):
        print("watch: -Root must be an existing directory (no cwd fallback)")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    if a["mode"] == "record":
        return cmd_record(a, root)
    if a["mode"] == "report":
        return cmd_report(a, root)
    if a["mode"] == "health":
        return cmd_health(a, root)
    return cmd_pollution(a, root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
