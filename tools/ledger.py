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

Durability (crash consistency, honest Windows boundary): save() flushes and os.fsync's
the file handle before close. os.fsync on Windows is FlushFileBuffers on that handle --
the closest stdlib equivalent of fsync(2); a directory-level fsync does not exist in the
Windows stdlib surface, so name-entry durability after a power cut is a stated boundary,
not a promise. A process kill does not lose OS-buffered writes, so in-simulator kills
already preserve every confirmed save.

Recovery scan (load time, physical truncation only): this is a whole-JSON-document
ledger, so its "truncated tail" is bytes lost off the closing brackets. A load that
fails to parse scans for the LAST '}' that still closes into a valid ledger document
with a complete entries list, quarantines everything after that point into the recycle
bin (<ledger dir>/trash, via trash.put_paths -- never raw-deleted), truncates the file
to that prefix closed with the 5-byte scaffolding "\n]\n}\n" (the rebuilt closing
brackets are structure, not bookkeeping), and prints
"recovered: truncated N bytes (quarantined at <path>)". Nothing is silently dropped,
the recovered entries are exactly the disk prefix (the sim asserts prefix-preservation),
and a bad tail never wedges the ledger forever. A file with no complete entry at all
(or a non-ledger shape, e.g. valid JSON that is not a document with an entries list)
is logical corruption -- no recovery, load refuses (fail-closed). Semantic tampering
of entry VALUES is undetectable here by design -- this book carries no hash chain;
tamper-evidence is the attest chain's job (the budget gate consumes the chain balance
when it exists, so the authenticated book outranks this one).

Ported from ledger.ps1; messages and exit codes are kept identical.
"""

import json
import os
import sys
from datetime import datetime

import trash  # same tools directory; the recovery scan quarantines bad tails, never raw-deletes

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


def _quarantine_tail(ledger_dir, dropped):
    """(bin_relpath, byte_count, "") or ("", 0, error): move the dropped bytes into the
    recycle bin under <ledger_dir>/trash -- the bin's iron rule, never a raw delete."""
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    tmp = os.path.join(ledger_dir, f"cost-ledger.tail-{stamp}-{os.getpid()}.part")
    try:
        with open(tmp, "wb") as fh:
            fh.write(dropped)
            fh.flush()
            os.fsync(fh.fileno())
        entries, err = trash.put_paths(ledger_dir, ledger_dir, [tmp],
                                       f"cost-ledger-tail-recovery:"
                                       f"{datetime.now().astimezone().isoformat(timespec='microseconds')}")
        if err:
            try:
                os.remove(tmp)
            except OSError:
                pass
            return "", 0, err
        return f"trash/{entries[0]['blob']}", len(dropped), ""
    except OSError as ex:
        try:
            if os.path.exists(tmp):
                os.remove(tmp)
        except OSError:
            pass
        return "", 0, f"quarantine write failed ({ex})"


# The rebuilt document closing: structure only, never bookkeeping (module docstring).
_SCAFFOLD = "\n]\n}\n"


def _recover_truncated(path):
    """Recovered entries, or None when there is no recoverable prefix.

    Scan for the LAST '}' that closes into a valid ledger document (largest intact
    prefix first), quarantine the dropped bytes, rewrite the file as the prefix plus
    the closing scaffolding. Any decode failure in the caller is a refusal -- this
    path only ever runs on a file that is valid UTF-8 but no longer valid JSON.
    """
    with open(path, "rb") as fh:
        raw = fh.read()
    text = raw.decode("utf-8-sig")
    for i in range(len(text) - 1, -1, -1):
        if text[i] != "}":
            continue
        candidate = text[:i + 1] + _SCAFFOLD
        try:
            doc = json.loads(candidate)
        except ValueError:
            continue
        if not isinstance(doc, dict):
            continue
        entries = doc.get("entries", [])
        if not isinstance(entries, list) or any(not isinstance(e, dict) for e in entries):
            continue
        dropped = text[i + 1:].encode("utf-8")
        if not dropped.strip():
            # only closing whitespace/brackets were lost -- nothing to quarantine,
            # but the file still needs its scaffolding to parse next time
            n = 0
            rel = "(nothing to quarantine)"
        else:
            ledger_dir = os.path.dirname(os.path.abspath(path)) or "."
            rel, n, err = _quarantine_tail(ledger_dir, dropped)
            if err:
                print(f"ledger: recovery refused -- the recycle bin would not take the "
                      f"bad tail ({err}) -- load refuses, bytes untouched (fail-closed)")
                return None
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(candidate)
            fh.flush()
            os.fsync(fh.fileno())
        print(f"ledger: recovered: truncated {n} bytes (quarantined at {rel}) -- "
              f"load continues on the intact entry prefix, totals re-derive from it")
        return entries
    return None


def load(path):
    """Entries, or None when the file exists but is not trustworthy.

    A corrupt ledger must never silently read as empty: an empty read makes the
    budget gate re-spend a budget that was already recorded. Callers fail closed
    on None (agent-verify's budget gate refuses; cost recording reports failed).
    Physical truncation is the one repairable shape (see _recover_truncated); a
    non-document JSON shape (e.g. a bare list) is logical corruption and refuses --
    reading it would crash downstream writers with a traceback instead of the
    clean refusal the budget gate knows how to report.
    """
    if not os.path.exists(path):
        return []
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            doc = json.load(fh)
    except UnicodeDecodeError as e:
        print(f"ledger: file is corrupt (not decodable: {e}) -- refusing to read "
              f"(a bit-flipped ledger must not read as empty)")
        return None
    except ValueError as e:
        recovered = _recover_truncated(path)
        if recovered is not None:
            return recovered
        print(f"ledger: file is corrupt (not valid JSON: {e}) -- refusing to read "
              f"(a half-written ledger must not read as empty)")
        return None
    if not isinstance(doc, dict) or not isinstance(doc.get("entries", []), list):
        print("ledger: file is corrupt (not a ledger document: no entries list) -- "
              "refusing to read (fail-closed)")
        return None
    return doc.get("entries", []) or []


def save(path, entries):
    doc = {"accounting": "totals derived from entries; a repeated key is recorded once",
           "entries": entries}
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False, indent=4)
        fh.write("\n")
        # flush then fsync the handle: Windows FlushFileBuffers semantics, the
        # durability boundary the module docstring states
        fh.flush()
        os.fsync(fh.fileno())


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
    if entries is None:
        return 2
    if a["mode"] == "add":
        return cmd_add(a, entries)
    return cmd_report(a, entries)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
