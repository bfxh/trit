#!/usr/bin/env python3
"""ledger: keep token cost and wall-clock cost in separate columns, on purpose.

    ledger.py -Mode add -Ledger .ledger.json -Task "split" -Key "obligation.rs|sha" -Tokens 18400 -Wall 12.6
    ledger.py -Mode report -Ledger .ledger.json

Why separate: a task can be cheap in wall-clock and ruinous in tokens (a long reasoning pass
over one big file), or the reverse (a suite that runs three minutes and costs almost nothing
to plan). Adding them into one "cost" hides which lever to pull.

Same accounting rule as costprobe: entries carry a key, and a key recorded twice is recorded
once. Totals are derived from entries, never accumulated -- an accumulator turns a double
count into a permanent lie, while a derived total just recomputes.

Subagents record participation: add --agent-id <id> and the entry carries it. The field is
appended only -- old ledgers without it read unchanged, old field semantics untouched.

Ported from ledger.ps1; messages and exit codes are kept identical.
"""

import json
import os
import sys
from datetime import datetime

MODES = {"add", "report"}


def parse_args(argv):
    out = {"mode": "", "ledger": "", "task": "", "key": "", "tokens": -1.0, "wall": -1.0,
           "agent_id": ""}
    positional = [x for x in argv if not x.startswith("-")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if name in ("tokens", "wall"):
            try:
                out[name] = float(val)
            except ValueError:
                out[name] = -1.0
        else:
            out[{"ledger": "ledger", "task": "task", "key": "key",
                 "agent-id": "agent_id", "agentid": "agent_id"}.get(name, name)] = val
        i += 2
    if not out["mode"] and positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def fmt_num(v):
    """PowerShell prints a double that holds an integer as `18400`, not `18400.0`."""
    if v is None:
        return "-"
    if float(v).is_integer():
        return str(int(v))
    return repr(float(v))


def median(values):
    if not values:
        return 0.0
    s = sorted(values)
    return s[int(len(s) / 2)]


def load(path):
    if not os.path.exists(path):
        return []
    with open(path, "r", encoding="utf-8-sig") as fh:
        doc = json.load(fh)
    return doc.get("entries", []) or []


def save(path, entries):
    doc = {"accounting": "totals derived from entries; a repeated key is recorded once",
           "entries": entries}
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False, indent=4)
        fh.write("\n")


def cmd_add(a, entries):
    if not a["task"]:
        print("ledger: -Task is required for add")
        return 2
    if a["tokens"] < 0 and a["wall"] < 0:
        print("ledger: nothing to record (-Tokens and/or -Wall)")
        return 2
    if not a["key"]:
        print("ledger: -Key is required (it is what prevents double counting)")
        return 2

    seen = {e.get("key") for e in entries}
    if a["key"] in seen:
        print(f"ledger: key '{a['key']}' already recorded -- kept the earlier entry, "
              f"discarded this one")
    else:
        entry = {
            "task": a["task"], "key": a["key"],
            "tokens": a["tokens"] if a["tokens"] >= 0 else None,
            "wall_s": a["wall"] if a["wall"] >= 0 else None,
            "at": datetime.now().astimezone().isoformat(timespec="microseconds"),
        }
        agent = str(a.get("agent_id", "")).strip()
        # appended field only: entries without it keep the exact old shape, and readers of
        # old ledgers never see a key they did not know
        if agent:
            entry["agent_id"] = agent
        entries.append(entry)
        tok = fmt_num(a["tokens"]) if a["tokens"] >= 0 else "-"
        wal = (fmt_num(a["wall"]) + "s") if a["wall"] >= 0 else "-"
        who = f", agent={agent}" if agent else ""
        print(f"ledger: recorded '{a['key']}' (tokens={tok}, wall={wal}{who})")
    save(a["ledger"], entries)
    return 0


def cmd_report(a, entries):
    if not entries:
        print("ledger: empty")
        return 0
    by_task = {}
    for e in entries:
        by_task.setdefault(e.get("task", ""), []).append(e)

    print(f"{'task':<28} {'tokens':>10} {'wall(s)':>10} {'entries':>8}")
    for t in sorted(by_task):
        es = by_task[t]
        tok = [float(e["tokens"]) for e in es if e.get("tokens") is not None]
        wal = [float(e["wall_s"]) for e in es if e.get("wall_s") is not None]
        print(f"{t:<28} {fmt_num(sum(tok)):>10} {fmt_num(sum(wal)):>10} {len(es):>8}")
    all_tok = [float(e["tokens"]) for e in entries if e.get("tokens") is not None]
    all_wal = [float(e["wall_s"]) for e in entries if e.get("wall_s") is not None]
    print()
    print(f"total tokens: {fmt_num(sum(all_tok))}   median per entry: {fmt_num(median(all_tok))}")
    print(f"total wall:   {fmt_num(round(sum(all_wal), 2))}s  median per entry: {fmt_num(median(all_wal))}s")
    print("note: the two totals are deliberately not combined -- a cheap hour and a cheap "
          "token bill are different wins")
    return 0


def main(argv):
    a = parse_args(argv)
    if a["mode"] not in MODES:
        print(f"ledger: -Mode 必须是 {sorted(MODES)} 之一")
        return 2
    if not a["ledger"]:
        print("ledger: -Ledger is required")
        return 2
    entries = load(a["ledger"])
    if a["mode"] == "add":
        return cmd_add(a, entries)
    return cmd_report(a, entries)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
