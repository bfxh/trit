#!/usr/bin/env python3
"""handoff: 跨智能体/跨会话交接包，带状态哈希门。

    handoff.py create -Root <dir> --id <id> --from <id> --to <id> --goal <text>
                      --next-action <text> [--decision <text>]... [--open-question <text>]...
                      [--claim <path>]... [--evidence <text>]... [--ttl-minutes 90]
                      [--out <file>] [--base-commit <sha>] [--state-hash <hex>]
    handoff.py gate -Root <dir> -Packet <file> [--json]
    handoff.py state -Root <dir> [--json]
    handoff.py show -Packet <file>
    handoff.py list -Root <dir>

为什么需要它：调度器能排出“下一个跑谁”，排不出“对方接住的是不是我以为的那棵树”。
`trit-orch::receipt::handoff_gate` 的语义在这里按同一张三态表落地：

    True     结构合法、未过期、base_commit 与 state_hash 都对得上 -> 接收
    Unknown  过期 / base_commit 变了 / state_hash 不一致 / 状态不可证 -> 先对账，不许接
    False    结构不合法（空字段、坏 JSON、类型不对）              -> 拒绝

`state_hash` 是宿主定义的“树状态”指纹：HEAD + `git status --porcelain -z` +
`git diff HEAD --binary` 的哈希 + 未跟踪文件内容哈希。它证明的是“交接时与接收时是同一棵树”，
不证明“树是对的”；后者是测试/回执的活。

落盘：默认 `<Root>/.agent/handoff/<id>.json`（已进 .gitignore，本机协调状态，不入树）。
--out 可放树外；包内不允许本机盘符路径（回执/文档随仓走，路径必须是仓库相对的）。

诚实边界：
- 不是分布式锁，不做共识；单机文件 + 调用方传 now。
- 不合并文件内容冲突；那归 VCS。
- state_hash 只覆盖 `-Root` 之下、git 看得见的东西；树外写入不在范围。
- `.journal/`、`.agent/handoff/`、`.agent/resume/`、`.agent/last-*` 等本机账本
  不计入 state_hash：它们是宿主状态，不是下一棒继承的源码状态；这也意味着
  state_hash 是出处指纹，不是安全边界。
- 过期不是 False（旧线索可能仍有用），是 Unknown。
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

MACHINE_PATH_RX = re.compile(r"(?:[A-Za-z]:[\\/]|\\\\)")
ID_RX = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
REQUIRED = ("id", "from", "to", "goal", "base_commit", "state_hash",
            "next_action", "idempotency_key")
LISTS = ("decisions", "open_questions", "claims", "evidence")
MAX_STATE_BYTES = 256 * 1024 * 1024
MAX_UNTRACKED_FILES = 20000
SKIP_UNTRACKED = (".journal/", ".agent/handoff/", ".agent/resume/",
                  ".agent/memory/", ".agent/signals.jsonl",
                  ".agent/last-selftest.json", ".agent/last-simchain.json",
                  ".agent/doctor-cache.json")


def git(root, args, binary=False):
    p = subprocess.run(["git", "-C", root, *args], capture_output=True,
                       text=not binary)
    if binary:
        return p.returncode, p.stdout
    return p.returncode, (p.stdout or "")


def head(root):
    rc, out = git(root, ["rev-parse", "--verify", "HEAD"])
    return out.strip() if rc == 0 and out.strip() else ""


def sha256_bytes(data):
    import hashlib
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    import hashlib
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def state(root):
    """(hash, reason). 不可证时 hash=None；调用方把不可证当 Unknown，不许当 True。"""
    root = os.path.abspath(root)
    h = head(root)
    if not h:
        return None, "not a git repository (HEAD unavailable)"
    rc, status = git(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all",
                            "--no-renames"])
    if rc != 0:
        return None, "git status failed"
    untracked, total, rows = [], 0, []
    for entry in status.split("\0"):
        if not entry:
            continue
        code, path = entry[:2], entry[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if not path or code != "??":
            rows.append(entry)
            continue
        # Local coordination/state dirs are not part of the source state the next
        # agent inherits: .journal/ is the chain's snapshot/lock area, .agent/handoff/
        # is where packets themselves live, and .agent/resume//last-*/doctor-cache
        # are per-host bookkeeping. Skipping them keeps the gate usable even when a
        # repository has not added them to .gitignore. This is a provenance hash,
        # not a security boundary (see docs/engineering/AGENT-COORDINATION.md).
        if path.startswith(SKIP_UNTRACKED):
            continue
        rows.append(entry)
        full = os.path.join(root, path.replace("/", os.sep))
        try:
            size = os.path.getsize(full)
        except OSError:
            return None, f"untracked file unreadable: {path}"
        total += size
        if total > MAX_STATE_BYTES or len(untracked) >= MAX_UNTRACKED_FILES:
            return None, "working tree too large to hash (state unprovable)"
        try:
            untracked.append([path, sha256_file(full)])
        except OSError:
            return None, f"untracked file unreadable: {path}"
    rc, diff = git(root, ["diff", "HEAD", "--binary", "--no-color", "--no-ext-diff"],
                   binary=True)
    if rc != 0:
        return None, "git diff failed"
    if len(diff) > MAX_STATE_BYTES:
        return None, "git diff too large to hash (state unprovable)"
    payload = json.dumps({"head": h, "status": "\0".join(rows),
                          "diff_sha256": sha256_bytes(diff),
                          "untracked": sorted(untracked)},
                         ensure_ascii=True, sort_keys=True, separators=(",", ":"))
    return sha256_bytes(payload.encode("utf-8")), ""


def validate(packet, now):
    """结构校验，返回 (ok, kind, reason)。kind: blank/expired/bad。"""
    if not isinstance(packet, dict):
        return False, "bad", "packet is not a JSON object"
    for field in REQUIRED:
        value = packet.get(field)
        if not isinstance(value, str) or not value.strip():
            return False, "blank", f"field {field} is blank or not a string"
    for field in LISTS:
        value = packet.get(field, [])
        if not isinstance(value, list):
            return False, "bad", f"field {field} is not a list"
        for item in value:
            if not isinstance(item, str) or not item.strip():
                return False, "blank", f"field {field} contains a blank item"
    exp = packet.get("expires_at")
    if not isinstance(exp, int) or isinstance(exp, bool) or exp <= 0:
        return False, "bad", "field expires_at must be a positive integer"
    if exp <= now:
        return False, "expired", f"packet expired at {exp} (now {now})"
    return True, "", ""


def gate(root, packet, now):
    ok, kind, reason = validate(packet, now)
    if not ok:
        return ("Unknown" if kind in ("expired",) else "False"), reason
    current_head = head(root)
    if not current_head:
        return "Unknown", "state unprovable: not a git repository (HEAD unavailable)"
    if packet["base_commit"] != current_head:
        return "Unknown", "base_commit moved since the packet was written"
    current, why = state(root)
    if current is None:
        return "Unknown", f"state unprovable: {why}"
    if packet["state_hash"] != current:
        return "Unknown", "state_hash mismatch: the tree changed since the packet was written"
    return "True", ""


def packet_id(packet):
    return packet.get("id") if isinstance(packet, dict) else None


def read_packet(path):
    try:
        with open(path, "rb") as fh:
            return json.loads(fh.read().decode("utf-8-sig"))
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        return {"__error__": str(exc)}


def write_packet(path, packet):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".handoff-", dir=os.path.dirname(os.path.abspath(path)))
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(packet, fh, ensure_ascii=False, indent=2)
            fh.write("\n")
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def default_out(root, packet_id):
    return os.path.join(os.path.abspath(root), ".agent", "handoff", f"{packet_id}.json")


def rel_label(root, path):
    try:
        rel = os.path.relpath(os.path.abspath(path), os.path.abspath(root))
    except ValueError:
        return path
    if rel.startswith(".."):
        return path
    return rel.replace("\\", "/")


def cmd_create(a):
    root = os.path.abspath(a.root)
    pid = (a.id or "").strip()
    if not ID_RX.fullmatch(pid):
        print("handoff: --id 必须是 [A-Za-z0-9][A-Za-z0-9._-]{0,63}")
        return 2
    if a.ttl_minutes <= 0:
        print("handoff: --ttl-minutes 必须 > 0（0 不是永久，过期要能发生）")
        return 2
    for label, attr in (("from", "from_"), ("to", "to"), ("goal", "goal"),
                        ("next-action", "next_action")):
        value = getattr(a, attr)
        if not value or not value.strip():
            print(f"handoff: --{label} 不能为空")
            return 2
    for field in ("decisions", "open_questions", "claims", "evidence"):
        for item in getattr(a, field):
            if not item.strip():
                print(f"handoff: --{field.replace('_', '-')} 不能有空项")
                return 2
    strings = [a.from_, a.to, a.goal, a.next_action] + a.decisions + \
        a.open_questions + a.claims + a.evidence
    bad = [s for s in strings if MACHINE_PATH_RX.search(s)]
    if bad:
        print("handoff: 包内不许带本机盘符路径（回执/文档随仓走，路径请写仓库相对的）")
        return 2
    base = a.base_commit.strip() or head(root)
    if not base:
        print("handoff: 读不到 HEAD，--base-commit 必须显式给")
        return 2
    if a.state_hash.strip():
        state_hash = a.state_hash.strip()
    else:
        state_hash, why = state(root)
        if state_hash is None:
            print(f"handoff: 状态不可证（{why}）-- 不许造一个哈希出来")
            return 2
    now = int(time.time())
    packet = {
        "id": pid,
        "from": a.from_.strip(),
        "to": a.to.strip(),
        "goal": a.goal.strip(),
        "base_commit": base,
        "state_hash": state_hash,
        "decisions": [s.strip() for s in a.decisions],
        "open_questions": [s.strip() for s in a.open_questions],
        "claims": [s.strip() for s in a.claims],
        "evidence": [s.strip() for s in a.evidence],
        "next_action": a.next_action.strip(),
        "idempotency_key": (a.idempotency_key.strip() or f"{a.from_}|{a.to}|{pid}"),
        "expires_at": now + a.ttl_minutes * 60,
    }
    ok, kind, reason = validate(packet, now)
    if not ok:
        print(f"handoff: 包结构不合法（{kind}: {reason}）")
        return 2
    out = a.out.strip() or default_out(root, pid)
    if os.path.exists(out):
        existing = read_packet(out)
        if isinstance(existing, dict) and existing.get("idempotency_key") == packet["idempotency_key"]:
            print(f"handoff: 已存在同一 idempotency_key 的包（幂等，不覆盖）：{rel_label(root, out)}")
            return 0
        print(f"handoff: 目标已存在且 idempotency_key 不同，拒绝覆盖：{rel_label(root, out)}")
        return 2
    write_packet(out, packet)
    print(f"handoff: 已写 {rel_label(root, out)}  "
          f"(base={base[:12]} state={state_hash[:12]} expires={packet['expires_at']})")
    return 0


def cmd_gate(a):
    packet = read_packet(a.packet)
    now = int(time.time())
    if isinstance(packet, dict) and "__error__" in packet:
        verdict, reason = "False", f"packet unreadable: {packet['__error__']}"
    else:
        verdict, reason = gate(a.root, packet, now)
    doc = {"tool": "handoff-gate", "packet": os.path.basename(a.packet),
           "id": packet_id(packet), "verdict": verdict, "reason": reason,
           "base_commit": packet.get("base_commit") if isinstance(packet, dict) else None,
           "state_hash": packet.get("state_hash") if isinstance(packet, dict) else None,
           "expires_at": packet.get("expires_at") if isinstance(packet, dict) else None}
    if a.json:
        print(json.dumps(doc, ensure_ascii=True, sort_keys=True))
    else:
        print(f"handoff-gate: {verdict}  {reason}")
        print(f"  packet={doc['packet']} id={doc['id']} base={doc['base_commit']} "
              f"state={doc['state_hash']}")
    return {"True": 0, "Unknown": 1, "False": 2}[verdict]


def cmd_state(a):
    digest, why = state(a.root)
    if digest is None:
        if a.json:
            print(json.dumps({"tool": "handoff-state", "verdict": "unknown",
                              "reason": why}, ensure_ascii=True, sort_keys=True))
        else:
            print(f"handoff-state: unknown  {why}")
        return 1
    head_sha = head(a.root)
    if a.json:
        print(json.dumps({"tool": "handoff-state", "verdict": "known",
                          "base_commit": head_sha, "state_hash": digest},
                         ensure_ascii=True, sort_keys=True))
    else:
        print(f"handoff-state: base={head_sha} state={digest}")
    return 0


def cmd_show(a):
    packet = read_packet(a.packet)
    if isinstance(packet, dict) and "__error__" in packet:
        print(f"handoff: 读不了 {a.packet}（{packet['__error__']}）")
        return 2
    print(json.dumps(packet, ensure_ascii=False, indent=2))
    return 0


def cmd_list(a):
    folder = os.path.join(os.path.abspath(a.root), ".agent", "handoff")
    if not os.path.isdir(folder):
        print("handoff: 没有 .agent/handoff（还没人写过交接包）")
        return 0
    rows = 0
    for name in sorted(os.listdir(folder)):
        if not name.endswith(".json"):
            continue
        packet = read_packet(os.path.join(folder, name))
        if not isinstance(packet, dict) or "__error__" in packet:
            print(f"  {name}: 坏包（{packet.get('__error__') if isinstance(packet, dict) else 'not json'}）")
            rows += 1
            continue
        print(f"  {packet.get('id')}: {packet.get('from')} -> {packet.get('to')}  "
              f"goal={str(packet.get('goal'))[:60]}  expires={packet.get('expires_at')}")
        rows += 1
    if rows == 0:
        print("handoff: 目录是空的")
    return 0


def build_parser():
    p = argparse.ArgumentParser(prog="handoff.py", description="agent handoff packet + state gate")
    sub = p.add_subparsers(dest="cmd", required=True)

    def root_arg(sp):
        sp.add_argument("-Root", "--root", default=os.getcwd())

    c = sub.add_parser("create")
    root_arg(c)
    c.add_argument("--id", required=True)
    c.add_argument("--from", dest="from_", required=True)
    c.add_argument("--to", required=True)
    c.add_argument("--goal", required=True)
    c.add_argument("--next-action", dest="next_action", required=True)
    c.add_argument("--decision", dest="decisions", action="append", default=[])
    c.add_argument("--open-question", dest="open_questions", action="append", default=[])
    c.add_argument("--claim", dest="claims", action="append", default=[])
    c.add_argument("--evidence", action="append", default=[])
    c.add_argument("--ttl-minutes", dest="ttl_minutes", type=int, default=90)
    c.add_argument("--out", default="")
    c.add_argument("--base-commit", dest="base_commit", default="")
    c.add_argument("--state-hash", dest="state_hash", default="")
    c.add_argument("--idempotency-key", dest="idempotency_key", default="")
    c.set_defaults(func=cmd_create)

    g = sub.add_parser("gate")
    root_arg(g)
    g.add_argument("-Packet", "--packet", required=True)
    g.add_argument("--json", action="store_true")
    g.set_defaults(func=cmd_gate)

    s = sub.add_parser("state")
    root_arg(s)
    s.add_argument("--json", action="store_true")
    s.set_defaults(func=cmd_state)

    sh = sub.add_parser("show")
    sh.add_argument("-Packet", "--packet", required=True)
    sh.set_defaults(func=cmd_show)

    ls = sub.add_parser("list")
    root_arg(ls)
    ls.set_defaults(func=cmd_list)
    return p


def main(argv):
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
