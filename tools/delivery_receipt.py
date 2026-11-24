#!/usr/bin/env python3
"""delivery_receipt: 交接包的 Sent -> Delivered -> Read -> Acted 四态账本。

    delivery_receipt.py sent    -Root <dir> -Packet <handoff packet> [--actor <id>]
    delivery_receipt.py deliver -Root <dir> --packet-id <id> --actor <id> [--note <text>]
    delivery_receipt.py read    -Root <dir> --packet-id <id> --actor <id> [--note <text>]
    delivery_receipt.py act     -Root <dir> --packet-id <id> --actor <id>
                                --evidence <text> [--note <text>]
    delivery_receipt.py verify  -Root <dir> --packet-id <id> [--packet <file>] [--json]
    delivery_receipt.py show    -Root <dir> --packet-id <id>

为什么需要它：
`handoff.py` 能证明“交接时与接收时是同一棵树”，但不能证明下一棒真的读到了包、
有没有动手。这个工具只补这一层：每个状态是一条带时间、actor、证据的追加记录，
顺序不可跳、actor 必须是包里的 `to`，`acted` 必须带证据。

诚实边界：
- 这是**本地自报账本**，不是密码学签名；能写账本的人也能伪造状态。
- `sent` 只证明本地写下了这条记录；`delivered/read/acted` 是接收方宿主自报。
- 包哈希绑定的是交接包字节；包被改写时 verify 会判红，但不会替人判断树对不对。
- 账本默认 `<Root>/.agent/delivery/<packet-id>.json`，已进 .gitignore，不随公开树走。
"""

import argparse
import hashlib
import importlib.util
import json
import os
import re
import sys
import tempfile
import time

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

TOOL = "delivery-receipt/1"
STATES = ("sent", "delivered", "read", "acted")
ID_RX = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
MACHINE_PATH_RX = re.compile(r"(?:[A-Za-z]:[\\/]|\\\\)")
SHA_RX = re.compile(r"[0-9a-f]{64}")
MAX_TEXT = 2000

# Load handoff.py by explicit file path. Do NOT put the tools directory on
# sys.path: tools/signal.py shadows the stdlib `signal` module on POSIX, and
# handoff.py imports subprocess. One packet schema, no shadowing side effect.
def _load_handoff():
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "handoff.py")
    spec = importlib.util.spec_from_file_location("_trit_handoff", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


try:
    handoff = _load_handoff()
except Exception as exc:  # pragma: no cover - only if the tree is incomplete
    handoff = None
    _HANDOFF_IMPORT_ERROR = str(exc)
else:
    _HANDOFF_IMPORT_ERROR = ""


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def default_ledger(root, packet_id):
    return os.path.join(os.path.abspath(root), ".agent", "delivery", f"{packet_id}.json")


def rel_label(root, path):
    try:
        rel = os.path.relpath(os.path.abspath(path), os.path.abspath(root))
    except ValueError:
        return path
    if rel.startswith(".."):
        return path
    return rel.replace("\\", "/")


def read_json(path):
    try:
        with open(path, "rb") as fh:
            return json.loads(fh.read().decode("utf-8-sig"))
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        return {"__error__": str(exc)}


def write_json_atomic(path, doc):
    parent = os.path.dirname(os.path.abspath(path))
    os.makedirs(parent, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".delivery-", dir=parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(doc, fh, ensure_ascii=False, indent=2)
            fh.write("\n")
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def _text(value, label):
    if value is None:
        return ""
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a string")
    value = value.strip()
    if len(value) > MAX_TEXT:
        raise ValueError(f"{label} is longer than {MAX_TEXT} characters")
    if MACHINE_PATH_RX.search(value):
        raise ValueError(f"{label} must not contain a machine path")
    return value


def validate_ledger(doc):
    if not isinstance(doc, dict):
        return False, "ledger is not a JSON object"
    if doc.get("tool") != TOOL:
        return False, f"tool must be {TOOL}"
    for key in ("packet_id", "packet_sha256", "from", "to", "goal", "created_at", "states"):
        if key not in doc:
            return False, f"missing field {key}"
    if not isinstance(doc["packet_id"], str) or not ID_RX.fullmatch(doc["packet_id"]):
        return False, "packet_id is not a valid id"
    if not isinstance(doc["packet_sha256"], str) or not SHA_RX.fullmatch(doc["packet_sha256"]):
        return False, "packet_sha256 must be 64 lowercase hex characters"
    for key in ("from", "to"):
        if not isinstance(doc[key], str) or not doc[key].strip():
            return False, f"{key} must be a non-empty string"
    if not isinstance(doc["goal"], str) or not doc["goal"].strip():
        return False, "goal must be a non-empty string"
    if len(doc["goal"]) > MAX_TEXT or MACHINE_PATH_RX.search(doc["goal"]):
        return False, "goal is invalid"
    if not isinstance(doc["created_at"], int) or isinstance(doc["created_at"], bool) \
            or doc["created_at"] <= 0:
        return False, "created_at must be a positive integer"
    states = doc["states"]
    if not isinstance(states, list) or not states:
        return False, "states must be a non-empty list"
    expected = list(STATES[:len(states)])
    actual = [s.get("state") if isinstance(s, dict) else None for s in states]
    if actual != expected:
        return False, f"states must start with the exact prefix {expected}, got {actual}"
    last_at = 0
    for idx, state in enumerate(states):
        if not isinstance(state, dict):
            return False, f"state[{idx}] is not an object"
        name = state["state"]
        at = state.get("at")
        actor = state.get("actor")
        evidence = state.get("evidence", "")
        note = state.get("note", "")
        if not isinstance(at, int) or isinstance(at, bool) or at <= 0:
            return False, f"state[{idx}].at must be a positive integer"
        if at < last_at:
            return False, f"state[{idx}].at goes backwards"
        last_at = at
        if not isinstance(actor, str) or not actor.strip():
            return False, f"state[{idx}].actor must be non-empty"
        if name == "sent" and actor != doc["from"]:
            return False, "sent actor must be packet from"
        if name in ("delivered", "read", "acted") and actor != doc["to"]:
            return False, f"{name} actor must be packet to"
        for label, value in (("evidence", evidence), ("note", note)):
            if not isinstance(value, str):
                return False, f"state[{idx}].{label} must be a string"
            if len(value) > MAX_TEXT or MACHINE_PATH_RX.search(value):
                return False, f"state[{idx}].{label} is invalid"
        if name == "acted" and not evidence.strip():
            return False, "acted requires non-empty evidence"
    return True, ""


def load_ledger(path):
    doc = read_json(path)
    if isinstance(doc, dict) and "__error__" in doc:
        return None, f"ledger unreadable: {doc['__error__']}"
    ok, why = validate_ledger(doc)
    if not ok:
        return None, why
    return doc, ""


def _packet(path):
    if handoff is None:
        return None, f"handoff.py import failed: {_HANDOFF_IMPORT_ERROR}"
    packet = handoff.read_packet(path)
    if isinstance(packet, dict) and "__error__" in packet:
        return None, f"packet unreadable: {packet['__error__']}"
    if not isinstance(packet, dict):
        return None, "packet is not a JSON object"
    return packet, ""


def _append(root, packet_id, state, actor, evidence="", note="", at=None):
    path = default_ledger(root, packet_id)
    doc, why = load_ledger(path)
    if doc is None:
        return None, path, why, 2
    try:
        actor = _text(actor, "actor")
        evidence = _text(evidence, "evidence")
        note = _text(note, "note")
    except ValueError as exc:
        return None, path, str(exc), 2
    if not actor:
        return None, path, "actor must be non-empty", 2
    at = int(time.time()) if at is None else int(at)
    states = doc["states"]
    next_state = STATES[len(states)] if len(states) < len(STATES) else None
    if state != next_state:
        return None, path, f"next state is {next_state or 'none'}, got {state}", 1
    if state == "sent" and actor != doc["from"]:
        return None, path, "sent actor must be packet from", 1
    if state in ("delivered", "read", "acted") and actor != doc["to"]:
        return None, path, f"{state} actor must be packet to", 1
    if at < states[-1]["at"]:
        return None, path, "state timestamp goes backwards", 1
    if state == "acted" and not evidence:
        return None, path, "acted requires --evidence", 1
    states.append({"state": state, "at": at, "actor": actor,
                   "evidence": evidence, "note": note})
    write_json_atomic(path, doc)
    return doc, path, "", 0


def cmd_sent(a):
    root = os.path.abspath(a.root)
    packet, why = _packet(a.packet)
    if packet is None:
        print(f"delivery: {why}")
        return 2
    if handoff is None:
        print(f"delivery: {why}")
        return 2
    now = int(a.at) if a.at is not None else int(time.time())
    ok, kind, reason = handoff.validate(packet, now)
    if not ok:
        print(f"delivery: 交接包结构不合法（{kind}: {reason}）")
        return 2
    pid = handoff.packet_id(packet)
    if not isinstance(pid, str) or not ID_RX.fullmatch(pid):
        print("delivery: 交接包 id 不合法")
        return 2
    try:
        packet_sha = sha256_file(a.packet)
    except OSError as exc:
        print(f"delivery: 读不了交接包（{exc}）")
        return 2
    path = default_ledger(root, pid)
    if os.path.exists(path):
        doc, why = load_ledger(path)
        if doc is None:
            print(f"delivery: 已有账本不可读（{why}）")
            return 2
        if doc["packet_sha256"] != packet_sha:
            print("delivery: 同一 packet id 的包字节已变；拒绝在旧账上继续")
            return 2
        print(f"delivery: 同一包已登记 sent（幂等，不追加）：{rel_label(root, path)}")
        return 0
    actor = (a.actor or packet["from"]).strip()
    if actor != packet["from"]:
        print("delivery: sent actor 必须等于包的 from")
        return 2
    try:
        goal = _text(packet.get("goal", ""), "goal")
    except ValueError as exc:
        print(f"delivery: {exc}")
        return 2
    doc = {
        "tool": TOOL,
        "packet_id": pid,
        "packet_sha256": packet_sha,
        "from": packet["from"],
        "to": packet["to"],
        "goal": goal,
        "created_at": now,
        "states": [{"state": "sent", "at": now, "actor": actor,
                    "evidence": "", "note": ""}],
    }
    write_json_atomic(path, doc)
    print(f"delivery: sent {rel_label(root, path)} (packet={pid} sha={packet_sha[:12]})")
    return 0


def _transition(a):
    root = os.path.abspath(a.root)
    pid = (a.packet_id or "").strip()
    if not ID_RX.fullmatch(pid):
        print("delivery: --packet-id 不合法")
        return 2
    state = {"deliver": "delivered", "act": "acted"}.get(a.cmd, a.cmd)
    doc, path, why, rc = _append(root, pid, state, a.actor, a.evidence, a.note, a.at)
    if rc != 0:
        print(f"delivery: {why}")
        return rc
    print(f"delivery: {state} {rel_label(root, path)} "
          f"({doc['states'][-1]['state']} by {doc['states'][-1]['actor']})")
    return 0


def cmd_deliver(a):
    return _transition(a)


def cmd_read(a):
    return _transition(a)


def cmd_act(a):
    return _transition(a)


def verify(root, packet_id, packet_path=""):
    if not ID_RX.fullmatch(packet_id or ""):
        return {"tool": TOOL, "status": "invalid", "reason": "bad packet id"}, 2
    path = default_ledger(root, packet_id)
    if not os.path.isfile(path):
        return {"tool": TOOL, "status": "missing", "reason": "no delivery ledger"}, 1
    doc, why = load_ledger(path)
    if doc is None:
        return {"tool": TOOL, "status": "invalid", "reason": why}, 2
    if packet_path:
        try:
            actual = sha256_file(packet_path)
        except OSError as exc:
            return {"tool": TOOL, "status": "invalid",
                    "reason": f"packet unreadable: {exc}"}, 2
        if actual != doc["packet_sha256"]:
            return {"tool": TOOL, "status": "invalid",
                    "reason": "packet bytes changed since sent"}, 2
    complete = len(doc["states"]) == len(STATES)
    return {
        "tool": TOOL,
        "status": "complete" if complete else "incomplete",
        "packet_id": doc["packet_id"],
        "packet_sha256": doc["packet_sha256"],
        "from": doc["from"],
        "to": doc["to"],
        "states": [s["state"] for s in doc["states"]],
        "complete": complete,
        "ledger": rel_label(root, path),
    }, 0 if complete else 1


def cmd_verify(a):
    root = os.path.abspath(a.root)
    pid = (a.packet_id or "").strip()
    doc, rc = verify(root, pid, a.packet)
    if a.json:
        print(json.dumps(doc, ensure_ascii=True, sort_keys=True))
    else:
        print(f"delivery-verify: {doc['status']}  {doc.get('reason', '')}")
        if doc.get("states"):
            print(f"  packet={doc['packet_id']} states={' -> '.join(doc['states'])}")
    return rc


def cmd_show(a):
    root = os.path.abspath(a.root)
    pid = (a.packet_id or "").strip()
    if not ID_RX.fullmatch(pid):
        print("delivery: --packet-id 不合法")
        return 2
    path = default_ledger(root, pid)
    doc, why = load_ledger(path)
    if doc is None:
        print(f"delivery: {why}")
        return 2
    print(json.dumps(doc, ensure_ascii=False, indent=2))
    return 0


def _selftest():
    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    with tempfile.TemporaryDirectory() as td:
        root = os.path.join(td, "repo")
        os.makedirs(os.path.join(root, ".agent"))
        packet_path = os.path.join(root, ".agent", "handoff", "h-001.json")
        os.makedirs(os.path.dirname(packet_path))
        packet = {
            "id": "h-001", "from": "qoder:abc", "to": "zcode:def",
            "goal": "证明下一棒读到了", "base_commit": "deadbeef",
            "state_hash": "0" * 64, "decisions": [], "open_questions": [],
            "claims": [], "evidence": [], "next_action": "跑 delivery 自测",
            "idempotency_key": "qoder:abc|zcode:def|h-001",
            "expires_at": int(time.time()) + 3600,
        }
        with open(packet_path, "w", encoding="utf-8") as fh:
            json.dump(packet, fh, ensure_ascii=False)

        def run(argv):
            old = sys.argv
            sys.argv = ["delivery_receipt.py"] + argv
            try:
                return main(argv)
            finally:
                sys.argv = old

        rc = run(["sent", "-Root", root, "-Packet", packet_path,
                  "--at", "1000"])
        ok(rc == 0, "sent writes a ledger")
        ledger = default_ledger(root, "h-001")
        doc, why = load_ledger(ledger)
        ok(doc is not None and len(doc["states"]) == 1, f"sent state present: {why}")
        ok(doc["states"][0]["state"] == "sent", "first state is sent")
        ok(doc["packet_sha256"] == sha256_file(packet_path), "packet hash bound")

        rc = run(["sent", "-Root", root, "-Packet", packet_path,
                  "--at", "1001"])
        ok(rc == 0, "repeated sent is idempotent")
        doc, _ = load_ledger(ledger)
        ok(len(doc["states"]) == 1, "repeated sent does not append")

        rc = run(["act", "-Root", root, "--packet-id", "h-001",
                  "--actor", "zcode:def", "--evidence", "commit:abc",
                  "--at", "1002"])
        ok(rc == 1, "act cannot skip delivered/read")
        doc, _ = load_ledger(ledger)
        ok(len(doc["states"]) == 1, "rejected transition did not append")

        rc = run(["deliver", "-Root", root, "--packet-id", "h-001",
                  "--actor", "qoder:abc", "--at", "1002"])
        ok(rc == 1, "deliver actor must be packet to")
        rc = run(["deliver", "-Root", root, "--packet-id", "h-001",
                  "--actor", "zcode:def", "--note", "received", "--at", "1003"])
        ok(rc == 0, "deliver by to succeeds")
        rc = run(["read", "-Root", root, "--packet-id", "h-001",
                  "--actor", "zcode:def", "--note", "read goal", "--at", "1004"])
        ok(rc == 0, "read after deliver succeeds")

        rc = run(["act", "-Root", root, "--packet-id", "h-001",
                  "--actor", "zcode:def", "--at", "1005"])
        ok(rc == 1, "act without evidence refused")
        rc = run(["act", "-Root", root, "--packet-id", "h-001",
                  "--actor", "zcode:def", "--evidence", "commit:abc", "--at", "1006"])
        ok(rc == 0, "act with evidence succeeds")

        doc, _ = load_ledger(ledger)
        ok([s["state"] for s in doc["states"]] ==
           ["sent", "delivered", "read", "acted"], "four states in order")
        out, rc = verify(root, "h-001", packet_path)
        ok(rc == 0 and out["status"] == "complete", "verify complete returns 0")

        rc = run(["act", "-Root", root, "--packet-id", "h-001",
                  "--actor", "zcode:def", "--evidence", "C:/tmp/x", "--at", "1007"])
        ok(rc == 2, "machine path in evidence refused")

        # A second ledger that stops at sent must report incomplete.
        packet2 = dict(packet)
        packet2["id"] = "h-002"
        p2 = os.path.join(root, ".agent", "handoff", "h-002.json")
        with open(p2, "w", encoding="utf-8") as fh:
            json.dump(packet2, fh, ensure_ascii=False)
        rc = run(["sent", "-Root", root, "-Packet", p2, "--at", "1010"])
        out, rc = verify(root, "h-002")
        ok(rc == 1 and out["status"] == "incomplete", "sent-only ledger is incomplete")

        # Tampering with the packet bytes must invalidate verify.
        with open(p2, "a", encoding="utf-8") as fh:
            fh.write("\n")
        out, rc = verify(root, "h-002", p2)
        ok(rc == 2 and out["status"] == "invalid", "changed packet bytes invalidate ledger")

        # Bad ledger shape is refused, not silently accepted.
        bad = os.path.join(root, ".agent", "delivery", "h-003.json")
        os.makedirs(os.path.dirname(bad), exist_ok=True)
        with open(bad, "w", encoding="utf-8") as fh:
            fh.write('{"tool": "delivery-receipt/1", "states": []}')
        out, rc = verify(root, "h-003")
        ok(rc == 2 and out["status"] == "invalid", "malformed ledger is invalid")

    print(f"delivery_receipt self-test: {checks}/{checks}")
    return 0


def build_parser():
    p = argparse.ArgumentParser(prog="delivery_receipt.py",
                                description="handoff Sent/Delivered/Read/Acted ledger")
    sub = p.add_subparsers(dest="cmd", required=True)

    def root_arg(sp):
        sp.add_argument("-Root", "--root", default=os.getcwd())

    sent = sub.add_parser("sent")
    root_arg(sent)
    sent.add_argument("-Packet", "--packet", required=True)
    sent.add_argument("--actor", default="")
    sent.add_argument("--at", type=int, default=None)
    sent.set_defaults(func=cmd_sent)

    for name, func in (("deliver", cmd_deliver), ("read", cmd_read), ("act", cmd_act)):
        sp = sub.add_parser(name)
        root_arg(sp)
        sp.add_argument("--packet-id", dest="packet_id", required=True)
        sp.add_argument("--actor", required=True)
        sp.add_argument("--evidence", default="")
        sp.add_argument("--note", default="")
        sp.add_argument("--at", type=int, default=None)
        sp.set_defaults(func=func)

    ver = sub.add_parser("verify")
    root_arg(ver)
    ver.add_argument("--packet-id", dest="packet_id", required=True)
    ver.add_argument("--packet", default="")
    ver.add_argument("--json", action="store_true")
    ver.set_defaults(func=cmd_verify)

    show = sub.add_parser("show")
    root_arg(show)
    show.add_argument("--packet-id", dest="packet_id", required=True)
    show.set_defaults(func=cmd_show)
    return p


def main(argv):
    if "--self-test" in argv:
        return _selftest()
    try:
        args = build_parser().parse_args(argv)
    except SystemExit as exc:
        return int(exc.code)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
