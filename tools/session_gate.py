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
  - Metrics: -Metrics writes decision/quality/per-user/per-domain/cost JSON; -Bench N
    reports p50/p95/p99 with a fresh state per iteration.
  - No deletion: skip means "do not write this session", never "evict old memory".

HONEST LIMITS
  These baselines are not the paper's LLM baselines. They exist to lock the gate boundary,
  state semantics and evaluation harness. F1/retention/metrics here are fixture numbers, not
  a claim about real users or Mem0/RMM.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from datetime import datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MEMORY = os.path.join(ROOT, "tools", "memory.py")
ID_RX = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
LABELS = {"long", "transient"}
DECISIONS = {"store", "skip"}
POLICIES = {"greedy", "context", "structure", "universal", "oracle", "external"}
METRICS_SCHEMA = 1
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


def _round(x):
    return round(float(x), 4)


def _safe_div(num, den):
    return float(num) / float(den) if den else 0.0


def _percentile(values, q):
    if not values:
        return 0.0
    vals = sorted(float(x) for x in values)
    if len(vals) == 1:
        return vals[0]
    pos = (len(vals) - 1) * q
    lo = int(pos)
    hi = min(lo + 1, len(vals) - 1)
    return vals[lo] + (vals[hi] - vals[lo]) * (pos - lo)


def _bytes(path):
    try:
        return os.path.getsize(path) if path and os.path.exists(path) else 0
    except OSError:
        return 0


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
    seen = set()
    for i, s in enumerate(data):
        if not isinstance(s, dict):
            raise ValueError(f"session #{i} is not an object")
        user, sid = str(s.get("user") or ""), str(s.get("session") or "")
        if not ID_RX.match(user) or not ID_RX.match(sid):
            raise ValueError(f"session #{i}: user/session must match {ID_RX.pattern}")
        key = (user, sid)
        if key in seen:
            raise ValueError(f"session #{i}: duplicate user/session {user}/{sid}")
        seen.add(key)
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
    specificity = tn / (tn + fp) if tn + fp else 0.0
    npv = tn / (tn + fn) if tn + fn else 0.0
    accuracy = (tp + tn) / len(sessions) if sessions else 0.0
    fpr = fp / (fp + tn) if fp + tn else 0.0
    fnr = fn / (fn + tp) if fn + tp else 0.0
    prevalence = (tp + fn) / len(sessions) if sessions else 0.0
    predicted_positive = (tp + fp) / len(sessions) if sessions else 0.0
    denom = ((tp + fp) * (tp + fn) * (tn + fp) * (tn + fn)) ** 0.5
    mcc = ((tp * tn) - (fp * fn)) / denom if denom else 0.0
    precision_t = tn / (tn + fn) if tn + fn else 0.0
    recall_t = tn / (tn + fp) if tn + fp else 0.0
    f1_t = 2 * precision_t * recall_t / (precision_t + recall_t) \
        if precision_t + recall_t else 0.0
    retention = retained / required if required else 0.0
    return {
        "sessions": len(sessions), "tp": tp, "fp": fp, "fn": fn, "tn": tn,
        "precision": round(precision, 4), "recall": round(recall, 4),
        "f1": round(f1, 4), "specificity": round(specificity, 4),
        "npv": round(npv, 4), "accuracy": round(accuracy, 4),
        "balanced_accuracy": round((recall + specificity) / 2, 4),
        "false_positive_rate": round(fpr, 4), "false_negative_rate": round(fnr, 4),
        "prevalence": round(prevalence, 4),
        "predicted_positive_rate": round(predicted_positive, 4),
        "mcc": round(mcc, 4), "mcc_defined": bool(denom),
        "specificity_defined": bool(tn + fp), "npv_defined": bool(tn + fn),
        "f1_transient": round(f1_t, 4),
        "f1_macro": round((f1 + f1_t) / 2, 4),
        "required_facts": required, "retained_facts": retained,
        "retention_proxy": round(retention, 4),
        "transient_facts_stored": transient_stored,
    }


def write_jsonl(path, records):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        for r in records:
            fh.write(json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n")


def write_json(path, doc):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False, indent=2, sort_keys=True)
        fh.write("\n")


def _group_stats(sessions, records):
    labels = [s.get("label") for s in sessions]
    out = {
        "sessions": len(sessions),
        "store": sum(1 for r in records if r["decision"] == "store"),
        "skip": sum(1 for r in records if r["decision"] == "skip"),
    }
    out["store_rate"] = _round(_safe_div(out["store"], len(sessions)))
    out["long"] = sum(1 for x in labels if x == "long")
    out["transient"] = sum(1 for x in labels if x == "transient")
    if sessions and all(x in LABELS for x in labels):
        q = evaluate(sessions, records)
        for k in ("precision", "recall", "f1", "specificity", "accuracy"):
            out[k] = q[k]
        # Undefined rates stay null instead of looking like real zeros.
        if not q["specificity_defined"]:
            out["specificity"] = None
        if not q["npv_defined"]:
            out["npv"] = None
        if not q["mcc_defined"]:
            out["mcc"] = None
        out.update({k: q[k] for k in ("balanced_accuracy", "npv", "mcc")
                    if k not in out})
    return out


def build_metrics(sessions, records, policy, elapsed_ms=0.0, input_path="",
                  state_path="", out_path="", bench=None):
    n = len(records)
    by_key = {(r["user"], r["session"]): r for r in records}
    labels = [s.get("label") for s in sessions]
    labels_present = bool(sessions) and all(x in LABELS for x in labels)
    quality = evaluate(sessions, records) if labels_present else None

    def group(primary):
        buckets = {}
        for s in sessions:
            key = primary(s)
            buckets.setdefault(key, []).append(s)
        out = {}
        for key, ss in sorted(buckets.items()):
            rr = [by_key.get((str(s["user"]), str(s["session"]))) for s in ss]
            out[key] = _group_stats(ss, [r for r in rr if r])
        return out

    def user(s):
        return str(s["user"])

    def domain(s):
        ds = session_domains(s)
        return ds[0] if ds else "(none)"

    scores = [r.get("score") for r in records if isinstance(r.get("score"), (int, float))]
    facts_total = sum(fact_count(s) for s in sessions)
    facts_long = sum(fact_count(s) for s in sessions if s.get("label") == "long")
    facts_transient = sum(fact_count(s) for s in sessions if s.get("label") == "transient")
    long_stored = transient_stored = 0
    for s in sessions:
        r = by_key.get((str(s["user"]), str(s["session"])))
        store = bool(r and r["decision"] == "store")
        if store and s.get("label") == "long":
            long_stored += fact_count(s)
        if store and s.get("label") == "transient":
            transient_stored += fact_count(s)

    stores = sum(1 for r in records if r["decision"] == "store")
    return {
        "schema": METRICS_SCHEMA,
        "policy": policy,
        "input": {
            "sessions": n,
            "users": len({str(s["user"]) for s in sessions}),
            "primary_domains": len({domain(s) for s in sessions}),
            "labels_present": labels_present,
            "label_distribution": {
                "long": sum(1 for x in labels if x == "long"),
                "transient": sum(1 for x in labels if x == "transient"),
                "missing": sum(1 for x in labels if x not in LABELS),
            },
            "input_bytes": _bytes(input_path),
        },
        "decisions": {
            "store": stores,
            "skip": n - stores,
            "store_rate": _round(_safe_div(stores, n)),
            "skip_rate": _round(_safe_div(n - stores, n)),
        },
        "facts": {
            "total": facts_total,
            "long": facts_long,
            "transient": facts_transient,
            "long_stored": long_stored,
            "transient_stored": transient_stored,
        },
        "quality": quality,
        "per_user": group(user),
        "per_domain": group(domain),
        "score": {
            "count": len(scores),
            "min": _round(min(scores)) if scores else 0.0,
            "max": _round(max(scores)) if scores else 0.0,
            "mean": _round(_safe_div(sum(scores), len(scores))) if scores else 0.0,
            "p50": _round(_percentile(scores, 0.50)),
            "p95": _round(_percentile(scores, 0.95)),
        },
        "cost": {
            "wall_ms": _round(elapsed_ms),
            "ms_per_session": _round(_safe_div(elapsed_ms, n)),
            "sessions_per_second": _round(_safe_div(n * 1000.0, elapsed_ms)),
            "state_bytes": _bytes(state_path),
            "out_bytes": _bytes(out_path),
        },
        "bench": bench,
    }


def bench_policy(sessions, policy, iterations, forced=None):
    if iterations < 1:
        return None
    totals, per_session = [], []
    for _ in range(iterations):
        state = empty_state()
        t0 = time.perf_counter()
        for s in sessions:
            decide_session(s, policy, state, forced)
        ms = (time.perf_counter() - t0) * 1000.0
        totals.append(ms)
        per_session.append(_safe_div(ms, len(sessions)))
    return {
        "iterations": iterations,
        "sessions": len(sessions),
        "total_ms": _round(sum(totals)),
        "mean_ms": _round(_safe_div(sum(totals), len(totals))),
        "per_session_ms": _round(_safe_div(sum(per_session), len(per_session))),
        "p50_ms": _round(_percentile(totals, 0.50)),
        "p95_ms": _round(_percentile(totals, 0.95)),
        "p99_ms": _round(_percentile(totals, 0.99)),
    }


def run_cli(a):
    sessions = read_sessions(a.sessions)
    state_arg = a.state or os.path.join(".agent", "session-gate", "state.json")
    state_path = state_arg if state_arg.lower().endswith(".json") else \
        os.path.join(state_arg, "state.json")
    state = load_state(state_path) if a.policy in ("context", "structure") else empty_state()
    records = []
    started = time.perf_counter()
    for s in sessions:
        decision, reason, score = decide_session(s, a.policy, state, a.decision)
        records.append({"at": iso_now(), "user": str(s["user"]), "session": str(s["session"]),
                        "policy": a.policy, "decision": decision,
                        "reason": redact(reason)[:240], "score": score})
        if not a.quiet:
            print(f"{s['user']}/{s['session']}: {decision}  ({reason})")
    elapsed_ms = (time.perf_counter() - started) * 1000.0
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
    if a.metrics:
        bench = bench_policy(sessions, a.policy, a.bench, a.decision) if a.bench else None
        m = build_metrics(sessions, records, a.policy, elapsed_ms=elapsed_ms,
                          input_path=a.sessions, state_path=state_path, out_path=a.out,
                          bench=bench)
        write_json(a.metrics, m)
        q = m.get("quality") or {}
        f1 = q.get("f1", "-") if q else "-"
        print(f"metrics: sessions={m['input']['sessions']} users={m['input']['users']} "
              f"store_rate={m['decisions']['store_rate']} f1={f1} "
              f"wall_ms={m['cost']['wall_ms']}")
        if bench:
            print(f"bench: iterations={bench['iterations']} p50={bench['p50_ms']}ms "
                  f"p95={bench['p95_ms']}ms p99={bench['p99_ms']}ms")
        print(f"metrics: wrote {a.metrics}")
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

    def write_sessions(path, items, as_array=False):
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            if as_array:
                json.dump(items, fh, ensure_ascii=False)
            else:
                for s in items:
                    fh.write(json.dumps(s, ensure_ascii=False) + "\n")

    def line_count(path):
        return len(open(path, encoding="utf-8").readlines()) if os.path.exists(path) else 0

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
        write_sessions(fixture, sessions)
        loaded = read_sessions(fixture)
        check("load", len(loaded) == 8, str(len(loaded)))

        # Matrix: 4 users x 30 sessions = 120 labeled sessions. 60 long / 60 transient.
        # Twelve per user are ambiguous (long with no surface cue, transient with long cues),
        # so greedy is not allowed to look perfect by accident.
        matrix = []
        for u in range(4):
            for i in range(30):
                sid = f"m{i}"
                dom = ["recipe", "code", "travel", "finance", "health"][(u * 30 + i) % 5]
                if i < 12:
                    text = f"项目 计划 目标 进度 下一步 继续 matrix-{u}-{i}"
                    label, facts = "long", ["f0", "f1", "f2"][:1 + i % 3]
                elif i < 15:
                    text = f"what should I do next matrix-{u}-{i}"
                    label, facts = "long", ["f0"]
                elif i < 27:
                    text = f"今天 天气 翻译 一次性 随口 matrix-{u}-{i}"
                    label, facts = "transient", []
                else:
                    text = f"continue the weather translation one-off matrix-{u}-{i}"
                    label, facts = "transient", []
                matrix.append({"user": f"mu{u}", "session": sid, "text": text,
                               "domains": [dom], "facts": facts, "label": label})
        matrix_path = os.path.join(td, "matrix.jsonl")
        write_sessions(matrix_path, matrix)
        mloaded = read_sessions(matrix_path)
        check("matrix load", len(mloaded) == 120, str(len(mloaded)))
        check("matrix labels", sum(1 for s in mloaded if s["label"] == "long") == 60 and
              sum(1 for s in mloaded if s["label"] == "transient") == 60)
        check("matrix users", len({s["user"] for s in mloaded}) == 4)

        mrecords = []
        for s in mloaded:
            d, reason, score = decide_session(s, "greedy", empty_state())
            check(f"matrix greedy {s['user']}/{s['session']}", d in DECISIONS, d)
            mrecords.append({"user": s["user"], "session": s["session"],
                             "decision": d, "reason": reason, "score": score})
        orecords = []
        for s in mloaded:
            d, reason, score = decide_session(s, "oracle", empty_state())
            check(f"matrix oracle {s['user']}/{s['session']}",
                  d == ("store" if s["label"] == "long" else "skip"), d)
            orecords.append({"user": s["user"], "session": s["session"],
                             "decision": d, "reason": reason, "score": score})
        mg = build_metrics(mloaded, mrecords, "greedy", elapsed_ms=1.0)
        mo = build_metrics(mloaded, orecords, "oracle", elapsed_ms=1.0)
        check("matrix metrics schema", mg["schema"] == METRICS_SCHEMA)
        check("matrix metrics counts", mg["decisions"]["store"] + mg["decisions"]["skip"] == 120)
        check("matrix metrics labels", mg["input"]["label_distribution"]["long"] == 60)
        check("matrix metrics per-user sum", sum(x["sessions"] for x in mg["per_user"].values()) == 120)
        check("matrix metrics per-domain sum", sum(x["sessions"] for x in mg["per_domain"].values()) == 120)
        check("matrix metrics quality", mg["quality"] is not None and
              mg["quality"]["tp"] + mg["quality"]["fp"] +
              mg["quality"]["fn"] + mg["quality"]["tn"] == 120)
        check("matrix greedy not perfect", 0.0 < mg["quality"]["f1"] < 1.0, str(mg["quality"]["f1"]))
        check("matrix oracle perfect", mo["quality"]["f1"] == 1.0 and
              mo["quality"]["retention_proxy"] == 1.0)
        check("matrix facts", mg["facts"]["long"] > 0 and mg["facts"]["transient"] == 0)
        check("matrix score stats", mg["score"]["count"] == 120 and
              mg["score"]["min"] <= mg["score"]["max"])

        # Input parsing and rejection boundary.
        arr_path = os.path.join(td, "array.json")
        write_sessions(arr_path, sessions, as_array=True)
        check("json array load", len(read_sessions(arr_path)) == 8)
        for name, body, needle in [
            ("empty file", "", "empty"),
            ("bad json", "{nope", "Expecting"),
            ("not object", "[1]", "not an object"),
            ("bad id", json.dumps([{"user": "bad id", "session": "s", "text": "x"}]), "must match"),
            ("empty text", json.dumps([{"user": "u", "session": "s", "text": ""}]), "empty text"),
            ("bad label", json.dumps([{"user": "u", "session": "s", "text": "x", "label": "maybe"}]), "label"),
            ("duplicate", json.dumps([
                {"user": "u", "session": "s", "text": "x"},
                {"user": "u", "session": "s", "text": "y"}]), "duplicate"),
        ]:
            path = os.path.join(td, "bad-" + name.replace(" ", "-") + ".json")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(body)
            raised = False
            try:
                read_sessions(path)
            except ValueError as e:
                raised = needle in str(e)
            check("reject " + name, raised)

        check("redact api key", redact("api_key=abc123") == "[redacted]")
        long_text = "x" * 300
        check("summary truncation", len(session_summary({"text": long_text})) == 240)
        check("summary redaction", "secret=" not in session_summary({"text": "secret=abc"}))
        check("domains string", session_domains({"domains": "Code"}) == ["code"])
        check("cue hits", cue_hits("PROJECT 今天").__len__() == 2 and
              cue_hits("PROJECT 今天")[0] == ["project"])

        # Decision primitives.
        check("greedy tie skip", decide_greedy({"text": "project weather"})[0] == "skip")
        uni = decide_session({"user": "u", "session": "s", "text": "今天"}, "universal", empty_state())
        check("universal store", uni[0] == "store")
        ext_field = decide_session({"user": "u", "session": "s", "text": "x", "decision": "skip"},
                                   "external", empty_state())
        check("external field", ext_field[0] == "skip")
        for name, s in [
            ("oracle missing label", {"user": "u", "session": "s", "text": "x"}),
            ("external invalid field", {"user": "u", "session": "s", "text": "x", "decision": "maybe"}),
        ]:
            raised = False
            try:
                decide_session(s, "oracle" if "oracle" in name else "external", empty_state())
            except ValueError:
                raised = True
            check(name, raised)

        ctx7 = empty_state()
        for i in range(7):
            decide_session({"user": "u", "session": f"s{i}", "text": "今天 天气",
                            "summary": f"sum{i}"}, "context", ctx7)
        check("context recent cap", len(ctx7["users"]["u"]["recent"]) == 5)

        st = empty_state()
        first_long = decide_session(sessions[0], "structure", st)
        check("structure new project", first_long[0] == "store" and len(st["users"]["u1"]["projects"]) == 1)
        st2 = empty_state()
        first_trans = decide_session(sessions[1], "structure", st2)
        check("structure isolated", first_trans[0] == "skip" and
              st2["users"]["u1"]["isolated"] == ["s2"])
        structure = [decide_session(s, "structure", st) for s in loaded]
        trap = next(x for s, x in zip(loaded, structure)
                    if s["user"] == "u1" and s["session"] == "s4")
        check("structure profile-shift trap", trap[0] == "store", trap[1])

        # State round-trip and corruption boundary.
        state_file = os.path.join(td, "state", "state.json")
        save_state(state_file, ctx7)
        check("state round-trip", load_state(state_file)["users"]["u"]["recent"][0]["session"] == "s2")
        check("state atomic tmp gone", not os.path.exists(state_file + ".tmp"))
        for name, body in [
            ("state bad version", json.dumps({"version": 2, "users": {}})),
            ("state bad users", json.dumps({"version": 1, "users": []})),
            ("state not object", "[]"),
        ]:
            path = os.path.join(td, "bad-state-" + name.split()[-1] + ".json")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(body)
            raised = False
            try:
                load_state(path)
            except ValueError:
                raised = True
            check(name, raised)

        # Evaluation metrics: perfect oracle and a known imperfect matrix.
        oracle_records = []
        for s in loaded:
            decision, reason, score = decide_session(s, "oracle", empty_state())
            oracle_records.append({"user": s["user"], "session": s["session"],
                                   "decision": decision, "reason": reason, "score": score})
        ev = evaluate(loaded, oracle_records)
        check("oracle f1", ev["f1"] == 1.0, str(ev))
        check("oracle retention", ev["retention_proxy"] == 1.0, str(ev))
        check("metrics full schema", all(k in ev for k in (
            "accuracy", "balanced_accuracy", "specificity", "npv", "f1_macro",
            "false_positive_rate", "false_negative_rate", "mcc")))
        check("metrics counts total", ev["tp"] + ev["fp"] + ev["fn"] + ev["tn"] == 8)
        check("metrics ranges", 0.0 <= ev["precision"] <= 1.0 and
              0.0 <= ev["recall"] <= 1.0 and -1.0 <= ev["mcc"] <= 1.0)

        no_label = os.path.join(td, "no-label.jsonl")
        write_sessions(no_label, [{"user": "u", "session": "s", "text": "项目 计划"}])
        no_label_metrics = build_metrics(
            read_sessions(no_label),
            [{"user": "u", "session": "s", "decision": "store", "score": 1}],
            "greedy")
        check("metrics no labels", no_label_metrics["quality"] is None and
              no_label_metrics["input"]["labels_present"] is False)

        bench = bench_policy(loaded, "greedy", 3)
        check("bench keys", bench and all(k in bench for k in (
            "iterations", "sessions", "p50_ms", "p95_ms", "p99_ms", "per_session_ms")))

        # CLI: output + metrics + benchmark + quiet.
        cli_out = os.path.join(td, "decisions.jsonl")
        cli_metrics = os.path.join(td, "metrics.json")
        cli = _call([os.path.abspath(__file__), "-Sessions", fixture, "-Policy", "greedy",
                     "-Out", cli_out, "-Metrics", cli_metrics, "-Bench", "3",
                     "-Eval", "-Quiet"])
        check("cli metrics rc", cli.returncode == 0, cli.stdout + cli.stderr)
        check("cli quiet", "u1/s1:" not in cli.stdout)
        check("cli out file", line_count(cli_out) == 8)
        with open(cli_metrics, encoding="utf-8") as fh:
            cli_doc = json.load(fh)
        check("cli metrics schema", cli_doc["schema"] == METRICS_SCHEMA and
              cli_doc["input"]["sessions"] == 8)
        check("cli metrics quality", cli_doc["quality"]["f1"] == 0.8889)
        check("cli metrics bench", cli_doc["bench"]["iterations"] == 3 and
              cli_doc["bench"]["p95_ms"] >= 0)
        check("cli metrics cost", cli_doc["cost"]["wall_ms"] > 0 and
              cli_doc["cost"]["out_bytes"] > 0)

        check("cli bad policy", _call([os.path.abspath(__file__), "-Sessions", fixture,
                                       "-Policy", "nope"]).returncode == 2)
        check("cli bench range", _call([os.path.abspath(__file__), "-Sessions", fixture,
                                       "-Bench", "-1"]).returncode == 2)
        check("cli missing sessions", _call([os.path.abspath(__file__)]).returncode == 2)
        eval_bad = _call([os.path.abspath(__file__), "-Sessions", no_label, "-Eval"])
        check("cli eval needs labels", eval_bad.returncode == 2 and "requires" in eval_bad.stdout)
        no_label_cli = _call([os.path.abspath(__file__), "-Sessions", no_label,
                              "-Metrics", os.path.join(td, "no-label-metrics.json")])
        check("cli metrics no labels", no_label_cli.returncode == 0)

        # State must hold only redacted summaries, not the full transcript.
        raw_text = "x" * 300 + "RAW_MARKER"
        raw_path = os.path.join(td, "raw.jsonl")
        write_sessions(raw_path, [{"user": "raw", "session": "s", "text": raw_text,
                                   "domains": ["raw"], "label": "long"}])
        raw_state_dir = os.path.join(td, "raw-state")
        raw_out = os.path.join(td, "raw-out.jsonl")
        raw_cli = _call([os.path.abspath(__file__), "-Sessions", raw_path,
                         "-Policy", "context", "-State", raw_state_dir, "-Out", raw_out,
                         "-Quiet"])
        check("raw cli rc", raw_cli.returncode == 0, raw_cli.stdout + raw_cli.stderr)
        state_raw = open(os.path.join(raw_state_dir, "state.json"), encoding="utf-8").read()
        out_raw = open(raw_out, encoding="utf-8").read()
        check("state no raw transcript", "RAW_MARKER" not in state_raw)
        check("out no raw transcript", "RAW_MARKER" not in out_raw)

        # Memory gate: skip is evidence, not deletion; store carries provenance.
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
        check("skip writes gate", line_count(gate) == 1)
        store = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "long",
                       "-Evidence", "selftest", "-Session", "s2", "-Gate", "store",
                       "-Gate-Reason", "long project"])
        check("memory store rc", store.returncode == 0, store.stdout + store.stderr)
        check("store writes entry", line_count(log) == 1)
        check("store writes gate", line_count(gate) == 2)
        audit = _call([MEMORY, "-Mode", "audit", "-Root", mem_root])
        check("memory audit clean", audit.returncode == 0, audit.stdout + audit.stderr)

        # Skip must not modify an existing entry.
        preserve = os.path.join(td, "preserve")
        os.makedirs(preserve)
        _call([MEMORY, "-Mode", "init", "-Root", preserve])
        old = _call([MEMORY, "-Mode", "note", "-Root", preserve, "-Title", "old",
                     "-Evidence", "selftest", "-Session", "old", "-Gate", "store"])
        check("preserve store", old.returncode == 0, old.stdout + old.stderr)
        plog = os.path.join(preserve, ".agent", "memory", "log.jsonl")
        pgate = os.path.join(preserve, ".agent", "memory", "gate.jsonl")
        before = line_count(plog)
        kept = _call([MEMORY, "-Mode", "note", "-Root", preserve, "-Title", "new",
                      "-Evidence", "selftest", "-Session", "new", "-Gate", "skip"])
        check("preserve skip rc", kept.returncode == 0, kept.stdout + kept.stderr)
        check("preserve log unchanged", line_count(plog) == before)
        check("preserve gate appended", line_count(pgate) == 2)

        # Same key updates provenance instead of duplicating the entry.
        update = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "long",
                        "-Evidence", "selftest2", "-Detail", "", "-Session", "s3",
                        "-Gate", "store", "-Gate-Policy", "external"])
        check("memory update rc", update.returncode == 0, update.stdout + update.stderr)
        check("memory update no duplicate", line_count(log) == 1)
        with open(log, encoding="utf-8") as fh:
            entry = json.loads(fh.readline())
        check("memory update provenance", entry.get("session") == "s3" and
              entry.get("gate") == "store" and entry.get("gate_policy") == "external")

        # Gate pairing and audit failures are refused, not silently accepted.
        for name, args in [
            ("gate without session", ["-Gate", "skip"]),
            ("session without gate", ["-Session", "s"]),
            ("invalid gate", ["-Session", "s", "-Gate", "maybe"]),
        ]:
            res = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "x",
                         "-Evidence", "selftest"] + args)
            check("memory " + name, res.returncode == 2, res.stdout + res.stderr)
        no_ev = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "x",
                       "-Session", "s", "-Gate", "skip"])
        check("memory skip needs evidence", no_ev.returncode == 1, no_ev.stdout + no_ev.stderr)
        secret_reason = _call([MEMORY, "-Mode", "note", "-Root", mem_root, "-Title", "x",
                               "-Evidence", "selftest", "-Session", "s",
                               "-Gate", "skip", "-Gate-Reason", "token=abc123"])
        check("memory skip secret reason", secret_reason.returncode == 1,
              secret_reason.stdout + secret_reason.stderr)

        bad_gate = os.path.join(td, "bad-gate")
        os.makedirs(bad_gate)
        _call([MEMORY, "-Mode", "init", "-Root", bad_gate])
        with open(os.path.join(bad_gate, ".agent", "memory", "gate.jsonl"), "w",
                  encoding="utf-8") as fh:
            fh.write("{not-json}\n")
        bad_audit = _call([MEMORY, "-Mode", "audit", "-Root", bad_gate])
        check("memory audit bad json", bad_audit.returncode == 1, bad_audit.stdout + bad_audit.stderr)
        with open(os.path.join(bad_gate, ".agent", "memory", "gate.jsonl"), "w",
                  encoding="utf-8") as fh:
            fh.write('{"session":"s"}\n')
        missing_audit = _call([MEMORY, "-Mode", "audit", "-Root", bad_gate])
        check("memory audit missing decision", missing_audit.returncode == 1,
              missing_audit.stdout + missing_audit.stderr)

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
    p.add_argument("-Metrics", dest="metrics", default="")
    p.add_argument("-Bench", dest="bench", type=int, default=0)
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
    if a.bench < 0 or a.bench > 1000:
        print("session-gate: -Bench must be between 0 and 1000")
        return 2
    try:
        return run_cli(a)
    except (OSError, ValueError, json.JSONDecodeError) as e:
        print(f"session-gate: {e}")
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
