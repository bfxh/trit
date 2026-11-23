#!/usr/bin/env python3
"""session-gate: decide whether a finished session is worth storing in memory.

WHY THIS EXISTS
  A universal memory policy spends the same budget on every session. The PerMem-Bench
  paper (arXiv:2605.25535) frames the alternative as a wrapper: after each session, a
  gate predicts long-horizon vs transient; transient sessions skip memory operations.
  This tool is the host-side primitive, not an LLM quality claim.

WHAT IT IS
  - Input: JSONL/JSON sessions with user, session id, turns (or text), optional domains,
    summary, facts and a label used only by --eval.
  - Policies: greedy, context, structure, universal, oracle, external.
    * greedy/context/structure are deterministic lexical baselines for plumbing tests.
    * oracle reads the fixture label and is an evaluation upper bound only.
    * external consumes a store/skip decision from an actual model/harness.
  - State: only redacted summaries for context and project terms for structure; raw
    transcripts are not persisted by this tool.
  - No deletion: skip means "do not write this session", never "evict old memory".

HONEST LIMITS
  These baselines are not the paper's LLM baselines. They exist to lock the gate boundary,
  state semantics and evaluation harness. F1/retention here are fixture numbers, not a
  claim about real users or Mem0/RMM.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MEMORY = os.path.join(ROOT, "tools", "memory.py")
ID_RX = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
LABELS = {"long", "transient"}
DECISIONS = {"store", "skip"}
POLICIES = {"greedy", "context", "structure", "universal", "oracle", "external"}
SECRET_RX = re.compile(
    r"(?i)(password|passwd|secret|api[_\-]?key|token)\s*[=:]\s*[^\s,;]+|"
    r"sk-[A-Za-z0-9_-]{8,}|ghp_[A-Za-z0-9]{20,}"
)

# Deliberately small and inspectable. A match is evidence of a surface cue, not a
# semantic judgment; the LLM baseline belongs behind --policy external.
LONG_CUES = [
    "项目", "计划", "目标", "上次", "继续", "进度", "下一步", "长期", "每周", "每天",
    "里程碑", "部署", "版本", "迭代", "坚持", "project", "plan", "goal", "continue",
    "progress", "next step", "long-term", "deadline", "milestone", "sprint", "roadmap",
]
TRANSIENT_CUES = [
    "今天", "现在", "这次", "翻译", "天气", "叫什么", "一次性", "顺便", "随口",
    "translate", "weather", "one-off", "right now", "quick question", "just asking",
]


def iso_now():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def redact(text):
    return SECRET_RX.sub("[redacted]", str(text or ""))


def cue_hits(text):
    low = str(text or "").lower()
    long_hits = sorted({c for c in LONG_CUES if c in low})
    trans_hits = sorted({c for c in TRANSIENT_CUES if c in low})
    return long_hits, trans_hits


def session_text(s):
    parts = []
    turns = s.get("turns")
    if isinstance(turns, list):
        for t in turns:
            if isinstance(t, dict):
                parts.append(str(t.get("text") or t.get("content") or ""))
            else:
                parts.append(str(t))
    parts.append(str(s.get("text") or ""))
    return "\n".join(x for x in parts if x.strip())


def session_summary(s):
    summary = str(s.get("summary") or "").strip()
    if not summary:
        summary = session_text(s).strip().replace("\n", " ")[:240]
    return redact(summary)[:240]


def session_domains(s):
    raw = s.get("domains") or []
    if isinstance(raw, str):
        raw = [raw]
    return [str(x).strip().lower() for x in raw if str(x).strip()][:8]


def fact_count(s):
    facts = s.get("facts") or []
    return len(facts) if isinstance(facts, list) else 0


def read_sessions(path):
    with open(path, "r", encoding="utf-8-sig") as fh:
        text = fh.read()
    if not text.strip():
        raise ValueError("sessions file is empty")
    if text.lstrip().startswith("["):
        data = json.loads(text)
    else:
        data = [json.loads(line) for line in text.splitlines() if line.strip()]
    if not isinstance(data, list):
        raise ValueError("sessions must be a JSON array or JSONL")
    out = []
    for i, s in enumerate(data):
        if not isinstance(s, dict):
            raise ValueError(f"session #{i} is not an object")
        user, sid = str(s.get("user") or ""), str(s.get("session") or "")
        if not ID_RX.match(user) or not ID_RX.match(sid):
            raise ValueError(f"session #{i}: user/session must match {ID_RX.pattern}")
        if not session_text(s).strip() and not session_summary(s):
            raise ValueError(f"session #{i}: empty text/summary")
        if "label" in s and s["label"] not in LABELS:
            raise ValueError(f"session #{i}: label must be long|transient")
        out.append(s)
    return out


def empty_state():
    return {"version": 1, "users": {}}


def load_state(path):
    if not os.path.exists(path):
        return empty_state()
    with open(path, "r", encoding="utf-8-sig") as fh:
        state = json.load(fh)
    if not isinstance(state, dict) or state.get("version") != 1:
        raise ValueError(f"bad state file: {path}")
    if not isinstance(state.get("users"), dict):
        raise ValueError(f"bad state users: {path}")
    return state


def save_state(path, state):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(state, fh, ensure_ascii=False, indent=2, sort_keys=True)
        fh.write("\n")
    os.replace(tmp, path)


def decide_greedy(s):
    text = session_text(s)
    lh, th = cue_hits(text)
    decision = "store" if len(lh) > len(th) else "skip"
    reason = f"cues long={lh[:4]} transient={th[:4]}"
    return decision, reason, len(lh) - len(th)


def decide_context(s, state):
    user = str(s["user"])
    urec = state["users"].setdefault(user, {})
    recent = urec.setdefault("recent", [])
    ctx = " ".join(str(x.get("summary") or "") for x in recent[-3:])
    text = session_text(s)
    lh, th = cue_hits(text)
    clh, cth = cue_hits(ctx)
    score = (len(lh) - len(th)) + (len(clh) - len(cth))
    decision = "store" if score > 0 else "skip"
    reason = f"cues long={lh[:3]} transient={th[:3]} context={clh[:3]}/{cth[:3]}"
    recent.append({"session": s["session"], "summary": session_summary(s), "decision": decision})
    del recent[:-5]
    return decision, reason, score


def decide_structure(s, state):
    user = str(s["user"])
    urec = state["users"].setdefault(user, {})
    projects = urec.setdefault("projects", [])
    isolated = urec.setdefault("isolated", [])
    domains = session_domains(s)
    text = session_text(s).lower()
    for p in projects:
        terms = [str(t).lower() for t in p.get("terms") or []]
        if any(t in text for t in terms) or any(d in terms for d in domains) \
                or (p.get("topic") and p["topic"] in text):
            p.setdefault("sessions", []).append(s["session"])
            return "store", f"project match {p.get('id')} topic={p.get('topic')}", 1
    lh, th = cue_hits(session_text(s))
    if len(lh) > len(th):
        pid = "p" + str(len(projects) + 1)
        topic = domains[0] if domains else (lh[0] if lh else "general")
        projects.append({"id": pid, "topic": topic, "terms": domains or [topic],
                         "sessions": [s["session"]], "status": "open"})
        return "store", f"new project {pid} topic={topic}", len(lh) - len(th)
    isolated.append(s["session"])
    return "skip", "isolated session", len(lh) - len(th)


def decide_session(s, policy, state, forced=None):
    if policy == "universal":
        return "store", "universal policy", 0
    if policy == "oracle":
        label = s.get("label")
        if label not in LABELS:
            raise ValueError("--policy oracle requires a long|transient label on every session")
        return ("store" if label == "long" else "skip"), f"oracle label={label}", 0
    if policy == "external":
        decision = forced or s.get("decision")
        if decision not in DECISIONS:
            raise ValueError("--policy external requires --decision store|skip or a decision field")
        return decision, str(s.get("reason") or "external decision"), 0
    if policy == "greedy":
        return decide_greedy(s)
    if policy == "context":
        return decide_context(s, state)
    if policy == "structure":
        return decide_structure(s, state)
    raise ValueError(f"unknown policy: {policy}")


def evaluate(sessions, records):
    labels = []
    for s in sessions:
        if s.get("label") not in LABELS:
            raise ValueError("--eval requires a long|transient label on every session")
        labels.append(s["label"])
    by_key = {(r["user"], r["session"]): r["decision"] for r in records}
    tp = fp = fn = tn = 0
    required = retained = transient_stored = 0
    for s in sessions:
        label = s["label"]
        decision = by_key.get((str(s["user"]), str(s["session"])))
        store = decision == "store"
        if label == "long" and store:
            tp += 1
        elif label == "long":
            fn += 1
        elif store:
            fp += 1
        else:
            tn += 1
        if label == "long":
            required += fact_count(s)
            if store:
                retained += fact_count(s)
        elif store:
            transient_stored += fact_count(s)
    precision = tp / (tp + fp) if tp + fp else 0.0
    recall = tp / (tp + fn) if tp + fn else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    retention = retained / required if required else 0.0
    return {
        "sessions": len(sessions), "tp": tp, "fp": fp, "fn": fn, "tn": tn,
        "precision": round(precision, 4), "recall": round(recall, 4),
        "f1": round(f1, 4), "required_facts": required, "retained_facts": retained,
        "retention_proxy": round(retention, 4),
        "transient_facts_stored": transient_stored,
    }


def write_jsonl(path, records):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        for r in records:
            fh.write(json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n")


def run_cli(a):
    sessions = read_sessions(a.sessions)
    state_arg = a.state or os.path.join(".agent", "session-gate", "state.json")
    state_path = state_arg if state_arg.lower().endswith(".json") else \
        os.path.join(state_arg, "state.json")
    state = load_state(state_path) if a.policy in ("context", "structure") else empty_state()
    records = []
    for s in sessions:
        decision, reason, score = decide_session(s, a.policy, state, a.decision)
        records.append({"at": iso_now(), "user": str(s["user"]), "session": str(s["session"]),
                        "policy": a.policy, "decision": decision,
                        "reason": redact(reason)[:240], "score": score})
        if not a.quiet:
            print(f"{s['user']}/{s['session']}: {decision}  ({reason})")
    if a.policy in ("context", "structure"):
        save_state(state_path, state)
    if a.out:
        write_jsonl(a.out, records)
    stores = sum(1 for r in records if r["decision"] == "store")
    print(f"session-gate: sessions={len(records)} policy={a.policy} "
          f"store={stores} skip={len(records) - stores}")
    if a.eval:
        ev = evaluate(sessions, records)
        print("eval: " + json.dumps(ev, ensure_ascii=False, sort_keys=True))
    return 0


def _call(args, cwd=None):
    return subprocess.run([sys.executable] + args, capture_output=True, text=True,
                          encoding="utf-8", errors="replace", cwd=cwd)


def self_test():
    passed = 0
    failed = []

    def check(name, cond, detail=""):
        nonlocal passed
        if cond:
            passed += 1
        else:
            failed.append(name + (": " + detail if detail else ""))

    with tempfile.TemporaryDirectory(prefix="session-gate-selftest-") as td:
        fixture = os.path.join(td, "sessions.jsonl")
        sessions = [
            {"user": "u1", "session": "s1", "text": "项目目标是下周继续迭代，上次进度已完成",
             "domains": ["recipe"], "facts": ["goal"], "label": "long"},
            {"user": "u1", "session": "s2", "text": "今天天气怎么样，帮我翻译这句话",
             "domains": ["weather"], "label": "transient"},
            {"user": "u1", "session": "s3", "text": "继续上周的食谱项目，下一步部署",
             "domains": ["recipe"], "facts": ["next"], "label": "long"},
            {"user": "u1", "session": "s4", "text": "随便问一下这个食谱项目的天气",
             "domains": ["recipe"], "label": "transient"},
            {"user": "u2", "session": "s1", "text": "my long-term project roadmap; continue sprint",
             "domains": ["code"], "facts": ["roadmap"], "label": "long"},
            {"user": "u2", "session": "s2", "text": "quick weather translation",
             "domains": ["weather"], "label": "transient"},
            {"user": "u2", "session": "s3", "text": "what should I do next",
             "domains": ["general"], "facts": ["next"], "label": "long"},
            {"user": "u2", "session": "s4", "text": "next milestone for the code project",
             "domains": ["code"], "facts": ["milestone"], "label": "long"},
        ]
        with open(fixture, "w", encoding="utf-8", newline="\n") as fh:
            for s in sessions:
                fh.write(json.dumps(s, ensure_ascii=False) + "\n")
        loaded = read_sessions(fixture)
        check("load", len(loaded) == 8, str(len(loaded)))

        greedy = [decide_session(s, "greedy", empty_state()) for s in loaded]
        check("greedy count", len(greedy) == 8)
        check("greedy has skip", any(x[0] == "skip" for x in greedy))

        oracle_records = []
        for s in loaded:
            decision, reason, score = decide_session(s, "oracle", empty_state())
            oracle_records.append({"user": s["user"], "session": s["session"],
                                   "decision": decision, "reason": reason, "score": score})
        ev = evaluate(loaded, oracle_records)
        check("oracle f1", ev["f1"] == 1.0, str(ev))
        check("oracle retention", ev["retention_proxy"] == 1.0, str(ev))

        st = empty_state()
        structure = [decide_session(s, "structure", st) for s in loaded]
        trap = next(x for s, x in zip(loaded, structure)
                    if s["user"] == "u1" and s["session"] == "s4")
        check("structure profile-shift trap", trap[0] == "store", trap[1])

        ctx = empty_state()
        c1 = decide_session(loaded[0], "context", ctx)
        c2 = decide_session(loaded[2], "context", ctx)
        check("context state", len(ctx["users"]["u1"]["recent"]) == 2 and c1[0] == "store" and c2[0] == "store")

        ext, ext_reason, _ = decide_session(loaded[1], "external", empty_state(), "skip")
        check("external forced", ext == "skip")

        bad_ext = False
        try:
            decide_session(loaded[1], "external", empty_state())
        except ValueError:
            bad_ext = True
        check("external missing", bad_ext)

        mem_root = os.path.join(td, "memroot")
        os.makedirs(mem_root)
        init = _call([MEMORY, "-Mode", "init", "-Root", mem_root])
        check("memory init", init.returncode == 0, init.stdout + init.stderr)
        skip = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "transient",
                      "-Evidence", "selftest", "-Session", "s1", "-Gate", "skip",
                      "-Gate-Reason", "one-off query"])
        check("memory skip rc", skip.returncode == 0, skip.stdout + skip.stderr)
        log = os.path.join(mem_root, ".agent", "memory", "log.jsonl")
        gate = os.path.join(mem_root, ".agent", "memory", "gate.jsonl")
        check("skip writes no entry", not os.path.exists(log) or os.path.getsize(log) == 0)
        check("skip writes gate", os.path.exists(gate) and len(open(gate, encoding="utf-8").readlines()) == 1)

        store = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "long",
                       "-Evidence", "selftest", "-Session", "s2", "-Gate", "store",
                       "-Gate-Reason", "long project"])
        check("memory store rc", store.returncode == 0, store.stdout + store.stderr)
        check("store writes entry", os.path.exists(log) and len(open(log, encoding="utf-8").readlines()) == 1)
        check("store writes gate", os.path.exists(gate) and len(open(gate, encoding="utf-8").readlines()) == 2)
        audit = _call([MEMORY, "-Mode", "audit", "-Root", mem_root])
        check("memory audit clean", audit.returncode == 0, audit.stdout + audit.stderr)

    total = passed + len(failed)
    print(f"session-gate self-test: {passed}/{total}")
    if failed:
        for f in failed:
            print("FAIL " + f)
        return 1
    return 0


def main(argv):
    p = argparse.ArgumentParser(prog="session-gate", add_help=True)
    p.add_argument("-Sessions", dest="sessions", default="")
    p.add_argument("-Policy", dest="policy", default="greedy")
    p.add_argument("-State", dest="state", default="")
    p.add_argument("-Out", dest="out", default="")
    p.add_argument("-Decision", dest="decision", default="", choices=["", "store", "skip"])
    p.add_argument("-Eval", dest="eval", action="store_true")
    p.add_argument("-Quiet", dest="quiet", action="store_true")
    p.add_argument("-SelfTest", dest="selftest", action="store_true")
    a = p.parse_args(argv)
    if a.selftest:
        return self_test()
    if not a.sessions:
        print("session-gate: -Sessions is required (or use -SelfTest)")
        return 2
    if a.policy not in POLICIES:
        print(f"session-gate: -Policy must be one of {sorted(POLICIES)}")
        return 2
    try:
        return run_cli(a)
    except (OSError, ValueError, json.JSONDecodeError) as e:
        print(f"session-gate: {e}")
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
