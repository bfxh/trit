#!/usr/bin/env python3
"""signal: fast triage of work vs emotion, and the penalty/mood/research exits around it.

    signal.py classify (--text <s> | --file <path>) [--rules <inline-json|path>] --root <dir>
    signal.py penalty --score <1-3> --reason <why> [--now <iso>] --root <dir>
    signal.py brief [--now <iso>] --root <dir>
    signal.py export --jsonl <path> [--with-text] --root <dir>

classify is pure rule matching (a keyword table fixed at module top, replaceable by
--rules): it runs in milliseconds and calls nothing. penalty appends a signal the harness
and future model training can read. brief renders the last day of penalties as one ASCII
line for the next round's memory. export projects the ledger into a research corpus.

Honest boundary: classify is rule triage, not a model judgment -- keyword rules with
unknown false-positive/negative rates, and emotion_score is a signal to weigh, never a
fact to trust. penalty cannot change model behavior by itself: this tool calls no API
and controls nothing -- what it changes is what the harness does next (heavier
reflection, tighter gates). The ledger keeps raw text locally (.agent/signals.jsonl);
export is the privacy gate -- default output is structural only, --with-text is the
explicit opt-in, and a with-text corpus inside the owner root is refused (it could be
committed). No network access anywhere.
"""

import datetime
import json
import os
import re
import sys

MODES = {"classify", "penalty", "brief", "export"}

SCORE_MAX = 3
BRIEF_WINDOW_S = 86400
STORE_TEXT_MAX = 8192
MAX_INPUT_CHARS = 1000000

MOOD_LABELS = {1: "strained", 2: "frustrated", 3: "upset"}
BRIEF_ADVICE = "reflect harder, do not argue user intent"

DEFAULT_RULES = {
    "emotion": {
        "frustration": {"weight": 1, "any": [
            "frustrated", "frustrating", "annoyed", "annoying", "irritated",
            "fed up", "sick of", "tired of"]},
        "despair": {"weight": 2, "any": [
            "hopeless", "pointless", "never works", "always fails", "no point",
            "give up", "wasted"]},
        "hostility": {"weight": 2, "any": [
            "angry", "furious", "enraged", "unacceptable", "useless", "garbage"]},
        "crisis": {"weight": 3, "any": [
            "done with this", "scrap it", "start over", "revert everything",
            "never again"]},
    },
    "task": {
        "imperative": {"weight": 1, "any": [
            "fix", "add", "remove", "update", "refactor", "implement", "write",
            "run", "test", "build", "deploy", "create", "delete", "migrate",
            "revert", "rename", "extract", "bump", "port", "ship"]},
        "tooling": {"weight": 1, "any": [
            "cargo", "git", "npm", "pytest", "python", "pwsh", "make",
            "rustc", "docker", "clippy"]},
    },
}

FILE_RE = re.compile(
    r"[A-Za-z0-9_.\-]+(?:[/\\][A-Za-z0-9_.\-]+)+"
    r"|[A-Za-z0-9_\-]+\.(?:py|rs|ts|tsx|js|jsx|md|json|toml|ya?ml|sh|ps1|txt|c|h|cpp|go|java|rb|lock)\b",
    re.IGNORECASE,
)

VALUE_FLAGS = {"file", "text", "rules", "root", "score", "reason", "jsonl", "now"}


def parse_args(argv):
    out = {"mode": "", "file": "", "text": "", "rules": "", "root": "", "score": "",
           "reason": "", "jsonl": "", "now": "", "with_text": False, "bad_flags": [],
           "now_dt": None}
    positional = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            positional.append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name in ("with-text", "with_text"):
            out["with_text"] = True
            i += 1
            continue
        if name not in VALUE_FLAGS:
            out["bad_flags"].append(a)
            i += 1
            continue
        out[name] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    if positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def is_inside(full, root):
    f, r = full.lower().replace("/", "\\"), root.lower().replace("/", "\\")
    return f == r or f.startswith(r + "\\")


def display_path(path, root):
    p = os.path.abspath(path)
    if is_inside(p, root):
        return os.path.relpath(p, root).replace(os.sep, "/")
    return "(outside owner root)"


def store_path(root):
    return os.path.join(root, ".agent", "signals.jsonl")


def iso(dt):
    return dt.isoformat(timespec="seconds")


def parse_ts(s):
    v = str(s).strip()
    if v.endswith("Z"):
        v = v[:-1] + "+00:00"
    try:
        dt = datetime.datetime.fromisoformat(v)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=datetime.datetime.now().astimezone().tzinfo)
    return dt


def _refuse(msg):
    print(f"signal: refuse -- {msg}")
    return 1


def _keyword_hit(kw, lower):
    if " " in kw:
        p = lower.find(kw.lower())
        return p >= 0, p
    m = re.search(r"(?<!\w)" + re.escape(kw.lower()) + r"(?!\w)", lower)
    return m is not None, m.start() if m else -1


def load_rules(spec):
    spec = spec.strip()
    if spec.startswith("{"):
        raw = spec
    else:
        try:
            with open(spec, "r", encoding="utf-8-sig") as fh:
                raw = fh.read()
        except OSError:
            return None, f"rules file not readable: {spec}"
    try:
        doc = json.loads(raw)
    except ValueError as exc:
        return None, f"rules json invalid ({exc})"
    if not isinstance(doc, dict) or not isinstance(doc.get("emotion"), dict) \
            or not isinstance(doc.get("task"), dict):
        return None, 'rules json must be {"emotion": {...}, "task": {...}}'
    table = {"emotion": {}, "task": {}}
    for kind in ("emotion", "task"):
        for name, rule in doc[kind].items():
            if not isinstance(name, str) or not name.strip():
                return None, "rule names must be non-empty strings"
            any_list = rule.get("any") if isinstance(rule, dict) else None
            if not isinstance(any_list, list) or not any_list \
                    or not all(isinstance(k, str) and k.strip() for k in any_list):
                return None, f"rule '{name}' needs a non-empty 'any' keyword list"
            w = rule.get("weight", 1)
            if not isinstance(w, int) or isinstance(w, bool) or not 0 <= w <= SCORE_MAX:
                return None, f"rule '{name}' weight must be an int in 0..{SCORE_MAX}"
            table[kind][name] = {"any": any_list, "weight": w}
    return table, ""


def classify_text(text, rules):
    lower = text.lower()
    hits = {"emotion": [], "task": []}
    for kind in ("emotion", "task"):
        for name, rule in rules[kind].items():
            for kw in rule["any"]:
                ok, pos = _keyword_hit(kw, lower)
                if ok:
                    hits[kind].append((pos, name, kw))
                    break
    score = min(SCORE_MAX, sum(rules["emotion"][name]["weight"] for _, name, _ in hits["emotion"]))
    signals = [name for _, name, _ in hits["emotion"]] + [name for _, name, _ in hits["task"]]
    raw_hints = [(pos, kw.lower()) for pos, _, kw in hits["task"]]
    for m in FILE_RE.finditer(text):
        raw_hints.append((m.start(), m.group(0)))
    raw_hints.sort(key=lambda t: (t[0], t[1]))
    hints = []
    for _, h in raw_hints:
        if h not in hints:
            hints.append(h)
    hints = hints[:8]
    if score > 0:
        kind = "mixed" if (hits["task"] or hints) else "emotion"
    else:
        kind = "task"
    return {"kind": kind, "emotion_score": score, "signals": signals, "task_hints": hints}


# The ledger is append-only, one JSON object per line, ensure_ascii only. brief and
# export refuse on any deviation instead of skipping it -- a pretty-printed or
# hand-truncated store is a corrupt store, not a smaller one.
def append_event(root, rec):
    try:
        os.makedirs(os.path.dirname(store_path(root)), exist_ok=True)
        with open(store_path(root), "a", encoding="utf-8", newline="\n") as fh:
            fh.write(json.dumps(rec, ensure_ascii=True) + "\n")
    except OSError as exc:
        return f"cannot append to the ledger ({exc}) -- refusing to act off the record"
    return ""


def read_events(root):
    store = store_path(root)
    if not os.path.exists(store):
        return [], ""
    events = []
    with open(store, "r", encoding="utf-8-sig") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except ValueError as exc:
                return None, (f"ledger line {lineno} is not valid JSON ({exc}) -- "
                              "investigate before briefing or exporting")
            if not isinstance(rec, dict) or not isinstance(rec.get("ts"), str) \
                    or not isinstance(rec.get("kind"), str):
                return None, f"ledger line {lineno} is missing ts/kind"
            if rec["kind"] == "penalty":
                s = rec.get("score")
                if not isinstance(s, int) or isinstance(s, bool) or not 1 <= s <= SCORE_MAX \
                        or not isinstance(rec.get("reason"), str):
                    return None, f"ledger line {lineno} is a malformed penalty event"
            else:
                es = rec.get("emotion_score")
                ok = isinstance(es, int) and not isinstance(es, bool) and 0 <= es <= SCORE_MAX
                ok = ok and isinstance(rec.get("signals"), list) and isinstance(rec.get("task_hints"), list)
                tl = rec.get("text_len")
                ok = ok and isinstance(tl, int) and not isinstance(tl, bool) and tl >= 0
                ok = ok and isinstance(rec.get("text"), str)
                if not ok:
                    return None, f"ledger line {lineno} is a malformed classify event"
            events.append(rec)
    return events, ""


def cmd_classify(a, root):
    text_in = a["text"]
    file_in = a["file"].strip()
    if text_in and file_in:
        print("signal: classify takes --text or --file, not both")
        return 2
    if not text_in and not file_in:
        print("signal: classify needs --text <s> or --file <path>")
        return 2
    if a["rules"].strip():
        rules, err = load_rules(a["rules"])
        if err:
            print(f"signal: {err}")
            return 2
    else:
        rules = DEFAULT_RULES
    if file_in:
        path = os.path.abspath(file_in)
        if not os.path.isfile(path):
            return _refuse(f"file does not exist: {file_in}")
        try:
            if os.path.getsize(path) > MAX_INPUT_CHARS:
                return _refuse("file is too large for triage (over 1,000,000 bytes)")
            with open(path, "r", encoding="utf-8-sig") as fh:
                text = fh.read(MAX_INPUT_CHARS + 1)
        except (OSError, ValueError):
            return _refuse("file is not readable as text (fail closed)")
    else:
        text = text_in
    if not text.strip():
        print("signal: nothing to classify (empty input)")
        return 2
    result = classify_text(text, rules)
    rec = {
        "ts": iso(datetime.datetime.now().astimezone()),
        "kind": result["kind"],
        "emotion_score": result["emotion_score"],
        "signals": result["signals"],
        "task_hints": result["task_hints"],
        "text_len": len(text),
        "text": text if len(text) <= STORE_TEXT_MAX else "",
    }
    err = append_event(root, rec)
    if err:
        return _refuse(err)
    print(json.dumps(result, ensure_ascii=True))
    return 0


def cmd_penalty(a, root):
    try:
        s = int(str(a["score"]).strip())
    except ValueError:
        print("signal: --score must be an integer in 1..3")
        return 2
    if not 1 <= s <= SCORE_MAX:
        print(f"signal: --score must be 1..{SCORE_MAX} (0 carries no signal -- refusing, not recording)")
        return 2
    reason = a["reason"].strip()
    if not reason:
        print("signal: penalty needs --reason <why> -- a number without a stated cause is not a signal")
        return 2
    events, err = read_events(root)
    if err:
        return _refuse(err)
    ts = a["now_dt"] or datetime.datetime.now().astimezone()
    err = append_event(root, {"ts": iso(ts), "kind": "penalty", "score": s, "reason": reason})
    if err:
        return _refuse(err)
    total = sum(1 for e in events if e["kind"] == "penalty") + 1
    print(f"signal: penalty score={s} recorded (penalty events in ledger: {total})")
    return 0


def cmd_brief(a, root):
    events, err = read_events(root)
    if err:
        return _refuse(err)
    now = a["now_dt"] or datetime.datetime.now().astimezone()
    window = []
    for e in events:
        if e["kind"] != "penalty":
            continue
        ts = parse_ts(e["ts"])
        if ts is None:
            return _refuse(f"penalty event at {e['ts']} has an unreadable timestamp -- investigate")
        age = (now - ts).total_seconds()
        if 0 <= age <= BRIEF_WINDOW_S:
            window.append(e["score"])
    if not window:
        print("mood: neutral")
        return 0
    peak = max(window)
    n = len(window)
    plural = "" if n == 1 else "s"
    print(f"mood: {MOOD_LABELS[peak]}(score={peak}, {n} event{plural} in last day) -- {BRIEF_ADVICE}")
    return 0


def cmd_export(a, root):
    out_in = a["jsonl"].strip()
    if not out_in:
        print("signal: export needs --jsonl <path>")
        return 2
    if not os.path.exists(store_path(root)):
        return _refuse("no ledger under this root -- nothing to export")
    events, err = read_events(root)
    if err:
        return _refuse(err)
    out_abs = os.path.abspath(out_in)
    with_text = a["with_text"]
    if with_text and is_inside(out_abs, root):
        return _refuse("a --with-text corpus inside the owner root risks being committed -- "
                       "write it outside the root (fail closed)")
    if os.path.exists(out_abs):
        return _refuse(f"output exists, refusing to overwrite: {display_path(out_in, root)}")
    rows = []
    for e in events:
        if e["kind"] == "penalty":
            row = {"ts": e["ts"], "kind": "penalty", "emotion_score": e["score"],
                   "penalty": e["score"], "text_len": len(e["reason"]), "signals": []}
            if with_text:
                row["text"] = e["reason"]
        else:
            row = {"ts": e["ts"], "kind": e["kind"], "emotion_score": e["emotion_score"],
                   "penalty": 0, "text_len": e["text_len"], "signals": e["signals"]}
            if with_text and e["text"]:
                row["text"] = e["text"]
        rows.append(row)
    try:
        parent = os.path.dirname(out_abs)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(out_abs, "w", encoding="utf-8", newline="\n") as fh:
            for row in rows:
                fh.write(json.dumps(row, ensure_ascii=True) + "\n")
    except OSError as exc:
        return _refuse(f"cannot write the corpus ({exc})")
    print(f"signal: exported {len(rows)} event(s) -> {display_path(out_in, root)} "
          f"(with-text={'yes' if with_text else 'no'})")
    return 0


def usage(code):
    print("""signal: fast triage of work vs emotion + the penalty/mood/research exits around it

usage:
  tools/signal.py classify (--text <s> | --file <path>) [--rules <inline-json|path>] --root <dir>
  tools/signal.py penalty --score <1-3> --reason <why> [--now <iso>] --root <dir>
  tools/signal.py brief [--now <iso>] --root <dir>
  tools/signal.py export --jsonl <path> [--with-text] --root <dir>

classify prints one JSON line {kind, emotion_score, signals, task_hints} and logs the
triage to <root>/.agent/signals.jsonl (logs need a writable root; an unloggable triage
refuses rather than run off the record). Rules are a keyword table fixed at module top;
--rules replaces the whole table (inline JSON or a file path). Single tokens match on
word boundaries, phrases match as substrings. No signal hits -> kind task with empty
lists: a triage default, not evidence.

penalty appends {ts, kind:"penalty", score, reason}; brief renders the last 24h of
penalties as one ASCII line (peak score decides the label; older events do not speak);
export projects the ledger into a research corpus {ts, kind, emotion_score, penalty,
text_len, signals} -- default output carries no text, --with-text adds the stored text
(inputs over 8192 chars are triaged and counted but not stored; text:'' means 'not
stored', never 'empty input').

honest boundary: classify is rule triage, not a model judgment -- keyword rules with
unknown false-positive/negative rates; emotion_score is a signal to weigh, never a fact
to trust. penalty cannot change model behavior by itself: this tool calls no API and
controls nothing -- what it changes is what the harness does next (heavier reflection,
tighter gates). The ledger keeps raw text locally; export is the privacy gate, and a
--with-text corpus inside the owner root is refused. No network access anywhere.""")
    return code


def main(argv):
    a = parse_args(argv)
    if not a["mode"]:
        return usage(2)
    if a["bad_flags"]:
        print("signal: unknown flag(s): " + ", ".join(a["bad_flags"]))
        return 2
    root_in = a["root"].strip()
    if not root_in or not os.path.isdir(root_in):
        print("signal: --root must be an existing directory (no cwd guessing)")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    if a["mode"] in ("penalty", "brief") and a["now"].strip():
        now = parse_ts(a["now"])
        if now is None:
            print(f"signal: cannot parse --now {a['now']!r} (ISO expected)")
            return 2
        a["now_dt"] = now
    if a["mode"] == "classify":
        return cmd_classify(a, root)
    if a["mode"] == "penalty":
        return cmd_penalty(a, root)
    if a["mode"] == "brief":
        return cmd_brief(a, root)
    return cmd_export(a, root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
