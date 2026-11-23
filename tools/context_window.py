#!/usr/bin/env python3
"""Deterministic context-window planning for agent transcripts.

The defaults encode the requested operating envelope:

* ``window_tokens = 1_000_000``
* compaction starts at ``850_000``
* after compaction the target is ``700_000``

This module is deliberately not an LLM summarizer.  It estimates tokens
conservatively, keeps system/first-user/last-user/pinned/tool-evidence
messages, drops the lowest-retention middle messages, and writes a compact
extractive digest of what was dropped (indices, roles, salient lines).  The
digest makes the omission auditable instead of silently truncating the
conversation.  If the anchors plus digest cannot fit the target, the planner
refuses rather than returning a plausible-looking unsafe transcript.

Token counts are estimates (ASCII ~4 chars/token, non-ASCII ~1.5 chars/token)
unless a message supplies an explicit integer ``tokens`` field.  They are a
budget signal, not a model tokenizer's exact count.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import random
import re
import sys

SCHEMA = "context-window/1"
ROLES = ("system", "user", "assistant", "tool")
ROLE_PRIORITY = {"system": 10, "user": 6, "assistant": 4, "tool": 3}

DEFAULTS = {
    "window_tokens": 1_000_000,
    "auto_compact_at": 850_000,
    "target_tokens": 700_000,
    "recent_keep": 6,
    "preserve_tool_evidence": True,
    "fail_closed": True,
}

DIGEST_MAX_CHARS = 12_000
DIGEST_LINE_CHARS = 180


def estimate_tokens(text: str) -> int:
    """Conservative, deterministic token estimate.

    English prose is usually near 4 chars/token; CJK is usually near 1-2
    chars/token.  Using 1.5 for non-ASCII deliberately overestimates rather
    than under-budgeting.  Empty content still costs a small role envelope.
    """
    if not text:
        return 0
    ascii_chars = sum(1 for ch in text if ord(ch) < 128)
    non_ascii = len(text) - ascii_chars
    return max(1, math.ceil(ascii_chars / 4.0 + non_ascii / 1.5))


def _content_text(value) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        parts = []
        for item in value:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                # OpenAI-style content blocks: keep the text, not the envelope.
                text = item.get("text")
                parts.append(text if isinstance(text, str) else json.dumps(
                    item, ensure_ascii=False, sort_keys=True))
            else:
                parts.append(str(item))
        return "\n".join(parts)
    if isinstance(value, dict):
        text = value.get("text")
        if isinstance(text, str):
            return text
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    return str(value)


def message_tokens(message: dict) -> int:
    explicit = message.get("tokens")
    if explicit is not None:
        if isinstance(explicit, bool) or not isinstance(explicit, int) or explicit < 0:
            raise ValueError("message.tokens must be a non-negative integer")
        return explicit
    return estimate_tokens(str(message.get("role", ""))) + \
        estimate_tokens(_content_text(message.get("content"))) + 4


def _priority(message: dict) -> int:
    value = message.get("priority")
    if value is None:
        return ROLE_PRIORITY.get(str(message.get("role")), 1)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError("message.priority must be a number")
    return int(value)


def _validate_messages(messages):
    if not isinstance(messages, list) or not messages:
        raise ValueError("transcript.messages must be a non-empty list")
    normalized = []
    for index, msg in enumerate(messages):
        if not isinstance(msg, dict):
            raise ValueError(f"messages[{index}] must be an object")
        role = msg.get("role")
        if role not in ROLES:
            raise ValueError(f"messages[{index}].role must be one of {ROLES}")
        if "tokens" in msg:
            message_tokens(msg)  # validates
        normalized.append(msg)
    if not any(m.get("role") == "user" for m in normalized):
        raise ValueError("transcript has no user message")
    return normalized


def _validate_config(window_tokens, auto_compact_at, target_tokens, recent_keep):
    for name, value in (("window_tokens", window_tokens),
                        ("auto_compact_at", auto_compact_at),
                        ("target_tokens", target_tokens)):
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"{name} must be a positive integer")
    if isinstance(recent_keep, bool) or not isinstance(recent_keep, int) or recent_keep < 0:
        raise ValueError("recent_keep must be a non-negative integer")
    if auto_compact_at > window_tokens:
        raise ValueError("auto_compact_at must be <= window_tokens")
    if target_tokens > auto_compact_at:
        raise ValueError("target_tokens must be <= auto_compact_at")


def _salient_line(text: str) -> str:
    text = re.sub(r"\s+", " ", text).strip()
    if not text:
        return "(empty message)"
    # Prefer a line with an identifier, number, path, or quoted claim; fall
    # back to the first line.  This is extraction, not a quality claim.
    lines = [ln.strip() for ln in text.splitlines() if ln.strip()]
    if not lines:
        lines = [text]
    for line in lines:
        if re.search(r"(?:\d|https?://|[/\\][A-Za-z0-9_.-]+|\"[^\"]{3,}\")", line):
            return line[:DIGEST_LINE_CHARS]
    return lines[0][:DIGEST_LINE_CHARS]


class _DigestOverflow(ValueError):
    """Raised when the evidence digest would exceed its audited cap."""


def _build_digest(messages, dropped):
    if not dropped:
        return None
    lines = []
    for index in sorted(dropped):
        msg = messages[index]
        line = _salient_line(_content_text(msg.get("content")))
        lines.append(f"[{index}] {msg.get('role')}: {line}")
    body = "\n".join(lines)
    if len(body) > DIGEST_MAX_CHARS:
        raise _DigestOverflow(
            f"omitted evidence digest would be {len(body)} chars, above cap "
            f"{DIGEST_MAX_CHARS}; refusing instead of silently truncating evidence")
    content = ("[context-compressed] deterministic extract of omitted messages; "
               "indices refer to the original transcript.\n" + body)
    return {
        "role": "system",
        "kind": "compressed_summary",
        "content": content,
        "tokens": estimate_tokens(content) + 4,
        "compressed": True,
    }


def _canonical_hash(messages) -> str:
    blob = json.dumps(messages, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def plan(messages, window_tokens=None, auto_compact_at=None, target_tokens=None,
         recent_keep=None, preserve_tool_evidence=None):
    """Return a plan/report; never silently drops without recording it."""
    cfg = dict(DEFAULTS)
    for key, value in (("window_tokens", window_tokens),
                       ("auto_compact_at", auto_compact_at),
                       ("target_tokens", target_tokens),
                       ("recent_keep", recent_keep),
                       ("preserve_tool_evidence", preserve_tool_evidence)):
        if value is not None:
            cfg[key] = value
    if not isinstance(cfg["preserve_tool_evidence"], bool):
        raise ValueError("preserve_tool_evidence must be boolean")
    _validate_config(cfg["window_tokens"], cfg["auto_compact_at"],
                     cfg["target_tokens"], cfg["recent_keep"])
    messages = _validate_messages(messages)
    tokens = [message_tokens(m) for m in messages]
    before = sum(tokens)
    digest_hash = _canonical_hash(messages)
    base = {
        "schema": SCHEMA,
        "window_tokens": cfg["window_tokens"],
        "auto_compact_at": cfg["auto_compact_at"],
        "target_tokens": cfg["target_tokens"],
        "before_tokens": before,
        "transcript_sha256": digest_hash,
        "notes": [
            "token counts are deterministic estimates, not tokenizer output",
            "compaction is extractive and auditable, not a model-quality summary",
        ],
    }
    if before <= cfg["auto_compact_at"]:
        return dict(base, status="noop", ok=True,
                    compressed_messages=list(messages),
                    after_tokens=before, saved_tokens=0, dropped=[],
                    preserved={"systems": sum(1 for m in messages if m.get("role") == "system")})

    anchors = set()
    for index, msg in enumerate(messages):
        role = msg.get("role")
        if role == "system":
            anchors.add(index)
        if msg.get("pinned") is True or _priority(msg) >= 9:
            anchors.add(index)
        if cfg["preserve_tool_evidence"] and role == "tool" and (
                msg.get("evidence") is True or msg.get("pinned") is True):
            anchors.add(index)
    first_user = next(i for i, m in enumerate(messages) if m.get("role") == "user")
    last_user = max(i for i, m in enumerate(messages) if m.get("role") == "user")
    anchors.add(first_user)
    anchors.add(last_user)

    anchor_tokens = sum(tokens[i] for i in anchors)
    candidates = [i for i in range(len(messages)) if i not in anchors]
    ordered = sorted(candidates, key=lambda i: (_priority(messages[i]), i))
    # Keep a recent tail unless the budget still cannot be met.
    recent = ordered[-cfg["recent_keep"]:] if cfg["recent_keep"] and ordered else []
    drop_order = [i for i in ordered if i not in set(recent)] + list(recent)
    dropped = set()

    def _total_after_drop():
        digest = _build_digest(messages, dropped)
        kept = anchor_tokens + sum(tokens[i] for i in candidates if i not in dropped)
        return kept + (message_tokens(digest) if digest else 0), digest

    while True:
        try:
            total, digest = _total_after_drop()
        except _DigestOverflow as exc:
            return dict(base, status="refused", ok=False,
                        compressed_messages=[], after_tokens=0,
                        saved_tokens=0, dropped=[],
                        error=str(exc), preserved={"anchors": len(anchors)})
        if total <= cfg["target_tokens"]:
            break
        next_drop = next((i for i in drop_order if i not in dropped), None)
        if next_drop is None:
            if cfg["fail_closed"]:
                return dict(base, status="refused", ok=False,
                            compressed_messages=[], after_tokens=total,
                            saved_tokens=0, dropped=[],
                            error="anchors + digest exist above target; refusing to truncate silently",
                            preserved={"anchors": len(anchors)})
            raise ValueError("cannot fit context target without dropping an anchor")
        dropped.add(next_drop)

    digest = _build_digest(messages, dropped)
    kept = [m for i, m in enumerate(messages) if i not in dropped]
    systems = [m for m in kept if m.get("role") == "system"
               and m.get("kind") != "compressed_summary"]
    rest = [m for m in kept if m.get("role") != "system"]
    out = systems + ([digest] if digest else []) + rest
    after = sum(message_tokens(m) for m in out)
    if after > cfg["target_tokens"]:
        return dict(base, status="refused", ok=False, compressed_messages=[],
                    after_tokens=after, saved_tokens=0, dropped=[],
                    error="post-compaction token count exceeds target; refusing",
                    preserved={"anchors": len(anchors)})
    return dict(
        base,
        status="compacted",
        ok=True,
        compressed_messages=out,
        after_tokens=after,
        saved_tokens=before - after,
        dropped=[{"index": i, "role": messages[i].get("role"),
                  "tokens": tokens[i], "reason": "low-retention middle segment"}
                 for i in sorted(dropped)],
        preserved={
            "systems": sum(1 for i in anchors if messages[i].get("role") == "system"),
            "first_user": first_user,
            "last_user": last_user,
            "pinned": sum(1 for i in anchors if messages[i].get("pinned") is True),
            "tool_evidence": sum(1 for i in anchors if messages[i].get("role") == "tool"),
            "anchors": len(anchors),
        },
    )


def context_config(settings):
    ctx = settings.get("context", {}) if isinstance(settings, dict) else {}
    merged = dict(DEFAULTS)
    for key in DEFAULTS:
        if key in ctx:
            merged[key] = ctx[key]
    return merged


def _self_test():
    checks = 0
    failures = []

    def ok(condition, label):
        nonlocal checks
        checks += 1
        if not condition:
            failures.append(label)

    # 1. defaults are the requested envelope.
    ok(DEFAULTS["window_tokens"] == 1_000_000, "default window is 1M")
    ok(DEFAULTS["auto_compact_at"] == 850_000, "default trigger is 850K")
    ok(DEFAULTS["target_tokens"] == 700_000, "default target is 700K")

    # 2. explicit-token under-trigger transcript is a no-op.
    under = [{"role": "system", "content": "sys", "tokens": 1},
             {"role": "user", "content": "u", "tokens": 800_000}]
    p_under = plan(under)
    ok(p_under["status"] == "noop" and p_under["after_tokens"] == 800_001,
       "below trigger is noop")

    # 3. over-trigger transcript compacts, keeping anchors and last user exactly.
    over = [{"role": "system", "content": "sys", "tokens": 5_000, "pinned": True},
            {"role": "user", "content": "first", "tokens": 5_000}]
    over += [{"role": "assistant", "content": f"middle-{i} id={i}", "tokens": 20_000}
             for i in range(43)]
    over.append({"role": "user", "content": "last question", "tokens": 5_000})
    p_over = plan(over)
    ok(p_over["status"] == "compacted" and p_over["after_tokens"] <= 700_000,
       "900K transcript compacts under target")
    ok(any(m.get("content") == "first" for m in p_over["compressed_messages"]),
       "first user is preserved as anchor")
    ok(any(m.get("content") == "last question" for m in p_over["compressed_messages"]),
       "last user is preserved exactly")
    ok(any(m.get("content") == "sys" for m in p_over["compressed_messages"]),
       "system is preserved")
    ok(any(m.get("kind") == "compressed_summary" for m in p_over["compressed_messages"]),
       "digest is present")
    ok(len(p_over["dropped"]) > 0 and p_over["saved_tokens"] > 0,
       "dropped segments are recorded")

    # 4. anchors alone above target refuse instead of truncating.
    huge = [{"role": "system", "content": "s", "tokens": 800_000},
            {"role": "user", "content": "u", "tokens": 100_000}]
    p_huge = plan(huge)
    ok(p_huge["status"] == "refused" and not p_huge["compressed_messages"],
       "anchor overflow is refused without output")

    # 4b. Too many long omitted lines cannot be silently truncated: refuse.
    long_msgs = [{"role": "system", "content": "s", "tokens": 5_000},
                 {"role": "user", "content": "u", "tokens": 5_000}]
    long_msgs += [{"role": "assistant", "content": "x" * 180 + f" id={i}",
                  "tokens": 10_000} for i in range(100)]
    long_msgs.append({"role": "user", "content": "last", "tokens": 5_000})
    p_digest = plan(long_msgs, target_tokens=200_000)
    ok(p_digest["status"] == "refused" and not p_digest["compressed_messages"]
       and "digest" in p_digest.get("error", ""),
       "digest cap overflow is refused, not truncated")

    # 5. shape validation.
    for label, kwargs in (
            ("target > trigger", {"auto_compact_at": 100, "target_tokens": 101,
                                  "window_tokens": 200}),
            ("zero window", {"window_tokens": 0}),
            ("bad role", None)):
        try:
            if label == "bad role":
                plan([{"role": "user", "content": "u"}, {"role": "robot", "content": "x"}])
            else:
                plan(under, **kwargs)
            ok(False, label)
        except ValueError:
            ok(True, label)

    # 6. CJK estimate is conservative relative to ASCII.
    ok(estimate_tokens("中" * 100) > estimate_tokens("a" * 100),
       "CJK estimate is conservative")
    ok(message_tokens({"role": "user", "content": "x", "tokens": 7}) == 7,
       "explicit tokens win")

    # 7. deterministic output and changed content changes hash.
    p_again = plan(over)
    ok(p_again["after_tokens"] == p_over["after_tokens"]
       and p_again["transcript_sha256"] == p_over["transcript_sha256"],
       "plan is deterministic")
    changed = [dict(m) for m in over]
    changed[2]["content"] = "different"
    ok(plan(changed)["transcript_sha256"] != p_over["transcript_sha256"],
       "content hash changes with content")

    # 8. kept originals are byte-for-byte messages (no truncation).
    kept_originals = [m for m in over if m.get("content") in
                      {x.get("content") for x in p_over["compressed_messages"]}]
    ok(all(m in p_over["compressed_messages"] for m in kept_originals),
       "kept messages are not rewritten")
    ok(p_over["preserved"]["first_user"] == 1
       and p_over["preserved"]["last_user"] == len(over) - 1,
       "preserved indexes are reported")

    # 9. config loader cannot silently change defaults.
    ok(context_config({})["auto_compact_at"] == 850_000,
       "empty settings use defaults")
    ok(context_config({"context": {"target_tokens": 500_000}})["target_tokens"] == 500_000,
       "settings override target")

    total = checks
    print(f"context-window self-test: {checks - len(failures)}/{total} passed")
    for failure in failures:
        print(f"FAIL: {failure}")
    return 0 if not failures else 1


def _stress(cases=100, seed=7):
    """Property/chaos pass: random shapes still obey the safety contract."""
    rng = random.Random(seed)
    failures = []
    for case in range(cases):
        n = rng.randint(2, 70)
        msgs = [{"role": "system", "content": f"sys-{case}-{s}",
                 "tokens": rng.randint(1_000, 20_000), "pinned": s == 0}
                for s in range(rng.randint(1, 3))]
        msgs.append({"role": "user", "content": f"first-{case}",
                     "tokens": rng.randint(1_000, 20_000)})
        first_user_content = msgs[-1]["content"]
        for i in range(n):
            role = rng.choice(["assistant", "assistant", "assistant", "user", "tool"])
            msg = {"role": role, "content": f"middle-{case}-{i} id={i}",
                   "tokens": rng.randint(5_000, 50_000)}
            if role == "tool" and rng.random() < 0.5:
                msg["evidence"] = True
            if rng.random() < 0.1:
                msg["pinned"] = True
            if rng.random() < 0.2:
                msg["priority"] = rng.randint(1, 9)
            msgs.append(msg)
        last = {"role": "user", "content": f"last-{case}",
                "tokens": rng.randint(1_000, 20_000)}
        msgs.append(last)
        snapshot = json.dumps(msgs, ensure_ascii=False, sort_keys=True)
        cfg = {
            "window_tokens": 1_000_000,
            "auto_compact_at": 850_000,
            "target_tokens": rng.randint(500_000, 840_000),
            "recent_keep": rng.randint(0, 10),
            "preserve_tool_evidence": rng.random() < 0.5,
        }
        report = plan(msgs, **cfg)
        again = plan(msgs, **cfg)
        problems = []
        if report["status"] not in ("noop", "compacted", "refused"):
            problems.append("unknown status")
        if report["status"] == "refused" and report["compressed_messages"]:
            problems.append("refused returned messages")
        if report["status"] == "compacted":
            if report["after_tokens"] > cfg["target_tokens"]:
                problems.append("compacted over target")
            if not report["dropped"]:
                problems.append("compacted without dropped records")
            digest = [m for m in report["compressed_messages"]
                      if m.get("kind") == "compressed_summary"]
            if len(digest) != 1:
                problems.append("missing or duplicate digest")
            contents = {m.get("content") for m in report["compressed_messages"]}
            if last["content"] not in contents:
                problems.append("last user missing")
            if first_user_content not in contents:
                problems.append("first user missing")
            if any(m["content"] not in contents for m in msgs if m["role"] == "system"):
                problems.append("system missing")
            originals = {json.dumps(m, ensure_ascii=False, sort_keys=True) for m in msgs}
            for m in report["compressed_messages"]:
                if m.get("kind") != "compressed_summary" \
                        and json.dumps(m, ensure_ascii=False, sort_keys=True) not in originals:
                    problems.append("output content invented")
        if report["status"] == "noop" and report["after_tokens"] != report["before_tokens"]:
            problems.append("noop changed token count")
        if again["status"] != report["status"] or again["after_tokens"] != report["after_tokens"]:
            problems.append("nondeterministic")
        if json.dumps(msgs, ensure_ascii=False, sort_keys=True) != snapshot:
            problems.append("input mutated")
        if problems:
            failures.append(f"case {case}: {problems}")
    print(f"context-window stress: {cases - len(failures)}/{cases} cases passed (seed={seed})")
    for failure in failures[:10]:
        print(f"FAIL: {failure}")
    return 0 if not failures else 1


def _load_transcript(path):
    with open(path, "r", encoding="utf-8-sig") as fh:
        doc = json.load(fh)
    if isinstance(doc, list):
        return doc
    if not isinstance(doc, dict) or not isinstance(doc.get("messages"), list):
        raise ValueError("transcript must be a message list or an object with messages[]")
    return doc["messages"]


def main(argv=None):
    parser = argparse.ArgumentParser(description="deterministic context-window planner")
    sub = parser.add_subparsers(dest="command", required=True)
    p_plan = sub.add_parser("plan", help="plan compaction")
    p_plan.add_argument("--transcript", required=True)
    p_plan.add_argument("--root", default=os.getcwd())
    p_plan.add_argument("--settings", default="")
    p_plan.add_argument("--window", type=int)
    p_plan.add_argument("--auto-compact-at", type=int)
    p_plan.add_argument("--target", type=int)
    p_plan.add_argument("--recent-keep", type=int)
    p_plan.add_argument("--out", default="")
    sub.add_parser("self-test", help="run built-in checks")
    p_stress = sub.add_parser("stress", help="randomized invariant pass")
    p_stress.add_argument("--cases", type=int, default=100)
    p_stress.add_argument("--seed", type=int, default=7)
    args = parser.parse_args(argv)
    if args.command == "self-test":
        return _self_test()
    if args.command == "stress":
        if args.cases <= 0:
            print("stress: --cases must be > 0")
            return 2
        return _stress(args.cases, args.seed)
    try:
        cfg = dict(DEFAULTS)
        settings = None
        if args.settings or os.path.isfile(os.path.join(args.root, ".agent", "settings.json")):
            from agent_settings import load_settings
            settings, _ = load_settings(args.root, explicit=args.settings)
            cfg = context_config(settings)
        for cli_key, cfg_key in (("window", "window_tokens"),
                                 ("auto_compact_at", "auto_compact_at"),
                                 ("target", "target_tokens"),
                                 ("recent_keep", "recent_keep")):
            value = getattr(args, cli_key)
            if value is not None:
                cfg[cfg_key] = value
        report = plan(_load_transcript(args.transcript),
                      window_tokens=cfg["window_tokens"],
                      auto_compact_at=cfg["auto_compact_at"],
                      target_tokens=cfg["target_tokens"],
                      recent_keep=cfg["recent_keep"],
                      preserve_tool_evidence=cfg["preserve_tool_evidence"])
    except (ValueError, OSError, json.JSONDecodeError) as exc:
        print(json.dumps({"schema": SCHEMA, "status": "error", "error": str(exc)},
                         ensure_ascii=False, indent=2))
        return 2
    text = json.dumps(report, ensure_ascii=False, indent=2)
    if args.out:
        parent = os.path.dirname(os.path.abspath(args.out))
        os.makedirs(parent, exist_ok=True)
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    else:
        print(text)
    return 0 if report["status"] in ("noop", "compacted") else 3


if __name__ == "__main__":
    sys.exit(main())
