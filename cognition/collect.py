#!/usr/bin/env python3
"""collect: the collection pass of the cognition training layer (standalone).

    collect.py --root <owner-root> [--out <dir>]

Reads the owner root's .agent stores and writes two artifacts, by default next to
this script (--out moves both):

  corpus/events.jsonl        merged privacy projection of signals + watch: the
                             signal.py export shape {ts, kind, emotion_score, penalty,
                             text_len, signals}; watch-derived rows add {event,
                             verdict, agent_id, turn}. No raw text by design -- there
                             is no --with-text flag here; when the training layer
                             needs original text the owner hands it over manually.
                             The directory is gitignored: interaction corpora are
                             private and never leave the clone.
  knowledge/pitfalls.jsonl   verbatim snapshot of the pitfall corpus (public
                             knowledge, meant to be committed). Idempotent: an
                             unchanged store produces unchanged bytes.

A store that does not exist is skipped and named in the summary -- an empty skeleton
collects cleanly (no data is not an error). A store with a bad line refuses the whole
run with the line number before anything is written (fail closed: a corpus built by
half-reading a ledger teaches half the truth).

Independence: stdlib only, no import from tools/, no trit crate, no network. The only
coupling to the owner repo is the data shape contract documented in SCHEMA.md, next to
this file -- moving this folder to the model repo changes nothing.

Exit codes: 0 collected (even all-skipped) / 1 refused / 2 bad invocation. stdout is
one ASCII line.
"""

import json
import os
import re
import sys

VALUE_FLAGS = {"root", "out"}
CATS = ("git", "github", "infra", "api", "semantics")
CORPUS_REL = os.path.join("corpus", "events.jsonl")
KNOWLEDGE_REL = os.path.join("knowledge", "pitfalls.jsonl")

# Same shape as pitfalls.py's add-time gate, but this one is the last gate before the
# public tree: the knowledge snapshot is committed, and the store is hand-editable.
# LANDMINE: scan parsed string VALUES, never the raw line -- JSON doubles backslashes
# ("C:\\Users\\x" on disk), so a raw-line scan is structurally blind to drive-letter
# user paths while looking like it works.
USER_PATH = re.compile(
    r"[A-Za-z]:\\Users\\"
    r"|/Users/[A-Za-z0-9._-]+/"
    r"|/home/[A-Za-z0-9._-]+/"
    r"|/mnt/[a-z]/"
)


def parse_args(argv):
    out = {"root": "", "out": "", "bad_flags": [], "extra": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            out["extra"].append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name not in VALUE_FLAGS:
            out["bad_flags"].append(a)
            i += 1
            continue
        out[name] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    return out


def refuse(msg):
    print(f"collect: refuse -- {msg}")
    return 1


def iter_strings(value):
    stack = [value]
    while stack:
        v = stack.pop()
        if isinstance(v, str):
            yield v
        elif isinstance(v, list):
            stack.extend(v)
        elif isinstance(v, dict):
            stack.extend(v.values())


def read_signals(path):
    """(rows, "") or (None, reason). Same strictness as signal.py export: the row
    schema is decided by kind -- penalty rows carry score/reason, classify rows carry
    emotion_score/signals/task_hints/text_len/text."""
    rows = []
    with open(path, "r", encoding="utf-8-sig") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except ValueError as exc:
                return None, f"signals line {lineno}: not valid JSON ({exc})"
            if not isinstance(rec, dict) or not isinstance(rec.get("ts"), str) \
                    or not isinstance(rec.get("kind"), str):
                return None, f"signals line {lineno}: missing ts/kind"
            if rec["kind"] == "penalty":
                s = rec.get("score")
                if not isinstance(s, int) or isinstance(s, bool) or not 1 <= s <= 3 \
                        or not isinstance(rec.get("reason"), str):
                    return None, f"signals line {lineno}: malformed penalty event"
                rows.append({"ts": rec["ts"], "kind": "penalty", "emotion_score": s,
                             "penalty": s, "text_len": len(rec["reason"]), "signals": []})
            else:
                es = rec.get("emotion_score")
                ok = isinstance(es, int) and not isinstance(es, bool) and 0 <= es <= 3
                ok = ok and isinstance(rec.get("signals"), list)
                ok = ok and isinstance(rec.get("task_hints"), list)
                tl = rec.get("text_len")
                ok = ok and isinstance(tl, int) and not isinstance(tl, bool) and tl >= 0
                ok = ok and isinstance(rec.get("text"), str)
                if not ok:
                    return None, f"signals line {lineno}: malformed classify event"
                rows.append({"ts": rec["ts"], "kind": rec["kind"], "emotion_score": es,
                             "penalty": 0, "text_len": tl, "signals": rec["signals"]})
    return rows, ""


def read_watch(path):
    """(rows, "") or (None, reason). The ledger is always the same six keys
    {ts, event, agent_id, turn, verdict, note}; shape drift is a bad line here,
    not a row to work around."""
    rows = []
    with open(path, "r", encoding="utf-8-sig") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except ValueError as exc:
                return None, f"watch line {lineno}: not valid JSON ({exc})"
            turn = rec.get("turn") if isinstance(rec, dict) else None
            ok = isinstance(rec, dict) and isinstance(rec.get("ts"), str) \
                and isinstance(rec.get("event"), str) \
                and isinstance(rec.get("agent_id"), str) \
                and isinstance(rec.get("verdict"), str) \
                and isinstance(rec.get("note"), str)
            ok = ok and (turn is None
                         or (isinstance(turn, int) and not isinstance(turn, bool)))
            if not ok:
                return None, (f"watch line {lineno}: malformed event (six keys: "
                              "ts, event, agent_id, turn, verdict, note)")
            rows.append({"ts": rec["ts"], "kind": "watch", "emotion_score": 0,
                         "penalty": 0, "text_len": len(rec["note"]), "signals": [],
                         "event": rec["event"], "verdict": rec["verdict"],
                         "agent_id": rec["agent_id"], "turn": turn})
    return rows, ""


def entry_valid(e):
    return (isinstance(e, dict) and isinstance(e.get("id"), str) and e["id"]
            and e.get("cat") in CATS
            and isinstance(e.get("trigger"), str) and e["trigger"]
            and isinstance(e.get("consequence"), str) and e["consequence"]
            and isinstance(e.get("prevention"), str) and e["prevention"])


def read_pitfalls(path):
    """(rows, "") or (None, reason). A snapshot that teaches must be complete and
    public-safe: malformed entries and machine user paths are refused with the line
    number, not carried into the committed knowledge file."""
    rows = []
    with open(path, "r", encoding="utf-8-sig") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except ValueError as exc:
                return None, f"pitfalls line {lineno}: not valid JSON ({exc})"
            if not entry_valid(rec):
                return None, (f"pitfalls line {lineno}: malformed entry (needs "
                              "id/cat/trigger/consequence/prevention)")
            if any(USER_PATH.search(s) for s in iter_strings(rec)):
                return None, (f"pitfalls line {lineno}: carries a machine user path -- "
                              "knowledge is committed to the public tree")
            rows.append(rec)
    return rows, ""


def write_lines(path, lines):
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = path + ".partial"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        for ln in lines:
            fh.write(ln + "\n")
    os.replace(tmp, path)


def usage(code):
    print("""collect: the cognition training layer's collection pass (stdlib only, no
import from tools/, no network).

usage:
  cognition/collect.py --root <owner-root> [--out <dir>]

Reads <root>/.agent/{signals.jsonl,watch.jsonl,pitfalls.jsonl}. A missing store is
skipped and named in the summary (an empty skeleton collects cleanly, exit 0); a store
with a bad line refuses the whole run with the line number before anything is written.

Outputs (default next to this script; --out moves both):
  corpus/events.jsonl       merged projection of signals + watch -- the signal.py
                            export shape {ts, kind, emotion_score, penalty, text_len,
                            signals}; watch rows add {event, verdict, agent_id, turn}.
                            No raw text by design: there is no --with-text flag, the
                            owner hands over original text manually when needed. The
                            directory is gitignored (interaction corpora stay local).
  knowledge/pitfalls.jsonl  verbatim snapshot of the pitfall corpus (public knowledge,
                            meant to be committed). Idempotent: unchanged store ->
                            unchanged bytes. Entries carrying a machine user path are
                            refused (the snapshot faces the public tree).

Row order is ledger order, signals block first, watch block second -- no cross-source
time sort (the two ledgers stamp local-offset timestamps; lexicographic order across
different offsets is not a timeline).

Exit codes: 0 collected (even all-skipped), 1 refused, 2 bad invocation.""")
    return code


def main(argv):
    a = parse_args(argv)
    if a["extra"]:
        print("collect: unexpected argument(s): " + ", ".join(a["extra"]))
        return 2
    if a["bad_flags"]:
        print("collect: unknown flag(s): " + ", ".join(a["bad_flags"]))
        return 2
    root_in = a["root"].strip()
    if not root_in:
        print("collect: --root <owner-root> is required (the directory holding .agent/)")
        return 2
    if not os.path.isdir(root_in):
        print("collect: --root must be an existing directory (no cwd guessing)")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    out_dir = os.path.abspath(a["out"].strip() or os.path.dirname(os.path.abspath(__file__)))

    skipped = []
    corpus_rows = []
    knowledge_rows = None

    sig_path = os.path.join(root, ".agent", "signals.jsonl")
    if os.path.exists(sig_path):
        rows, err = read_signals(sig_path)
        if err:
            return refuse(err)
        corpus_rows.extend(rows)
    else:
        skipped.append("signals absent")

    watch_path = os.path.join(root, ".agent", "watch.jsonl")
    if os.path.exists(watch_path):
        rows, err = read_watch(watch_path)
        if err:
            return refuse(err)
        corpus_rows.extend(rows)
    else:
        skipped.append("watch absent")

    pit_path = os.path.join(root, ".agent", "pitfalls.jsonl")
    if os.path.exists(pit_path):
        knowledge_rows, err = read_pitfalls(pit_path)
        if err:
            return refuse(err)
    else:
        skipped.append("pitfalls absent")

    try:
        if len(skipped) < 2:
            write_lines(os.path.join(out_dir, CORPUS_REL),
                        [json.dumps(r, ensure_ascii=True) for r in corpus_rows])
        if knowledge_rows is not None:
            write_lines(os.path.join(out_dir, KNOWLEDGE_REL),
                        [json.dumps(r, ensure_ascii=False) for r in knowledge_rows])
    except OSError as exc:
        return refuse(f"cannot write the artifacts ({exc})")

    summary = (f"collect: corpus={len(corpus_rows)} events "
               f"knowledge={len(knowledge_rows) if knowledge_rows is not None else 0} "
               f"pitfalls")
    if skipped:
        summary += " (skipped: " + ", ".join(skipped) + ")"
    print(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
