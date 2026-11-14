#!/usr/bin/env python3
"""把仓外交接账本同步成**脱敏只读快照**，让 `tech_collide` 在 CI 里也能跑。

用法：
  python scripts/sync_handoff_snapshot.py                 # 从默认账本目录生成快照
  python scripts/sync_handoff_snapshot.py --src <dir>     # 账本在别处
  python scripts/sync_handoff_snapshot.py --check         # 只比对快照是否已过期，不写盘
  python scripts/sync_handoff_snapshot.py --self-test

为什么这么设计：
  · 账本正本在**仓外**（多会话共用），CI runner 上没有它 ⇒ 原先这道门只能当「本机门」，等于
    CI 上没人盯着编号复用。#503 讨论的结论是：仓内留一份**脱敏快照**，门默认读快照。
  · 快照只保留判定需要的字段：编号、开/关状态、标题、负责会话、repo、路径、标签。
    正文叙述、绝对路径、token 一律丢弃或替换 —— 快照要能公开进仓，就不能带本机痕迹。
  · 快照是**生成物**：别手改，改正本再跑本脚本。`--check` 用哈希判断是否过期。

脱敏规则（都在 `sanitize()` 里，自测逐条覆盖）：
  1. 本机绝对路径 `X:/a/b/c` 或 `X:\\a\\b\\c` → `<local>/c`（保留最后一段，判定仍能区分不同项目）；
  2. `C:\\Users\\<name>` 这类用户目录 → `<local>/users`；
  3. token 形状（`ghp_` / `gho_` / `sk-` / `AKIA` / `Bearer <token>`）→ `<redacted>`。
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SNAPSHOT = ROOT / "scripts" / "ledger-snapshot"
DEFAULT_SRC = pathlib.Path("D:/KF/_handoff")

# LEDGER 里判定只需要条目行（`- [ ] **A123 · 标题**（…负责：…）`），正文不进口快照。
ENTRY_RE = re.compile(r"^- \[[ xX]\] \*\*A[0-9]+[^*]*\*\*.*$")
ABS_PATH = re.compile(r"[A-Za-z]:[\\/][^\s`'\"）)，,；;]*")
USER_DIR = re.compile(r"[A-Za-z]:[\\/]Users[\\/][^\s`'\"）)，,；;\\/]+", re.IGNORECASE)
SECRET = re.compile(
    r"(?:ghp_[A-Za-z0-9]{10,}|gho_[A-Za-z0-9]{10,}|github_pat_[A-Za-z0-9_]{10,}"
    r"|sk-[A-Za-z0-9]{10,}|AKIA[0-9A-Z]{12,}|Bearer\s+[A-Za-z0-9._\-]{10,})"
)

HEADER_TAIL = (
    "> **这是生成物，别手改。** 由 `python scripts/sync_handoff_snapshot.py` 从仓外正本生成，\n"
    "> 只保留 `tech_collide` 判定需要的字段：编号、开/关、标题、负责会话。\n"
    "> 正文叙述与本机绝对路径已丢弃/替换。\n"
    "> 快照超过 30 天时 `tech_collide.py` 只告警不判红；判「与正本是否一致」跑 `--check`。\n\n"
)
STAMP_LINE = re.compile(r"^> 生成时间：.*$", re.MULTILINE)


def header(stamp: str) -> str:
    return (
        "# 交接账本快照（脱敏，只读）\n\n"
        f"> 生成时间：{stamp}（UTC）。正本在仓外（多会话共用的 `_handoff/LEDGER.md`）。\n"
        + HEADER_TAIL
    )


def sanitize(text: str) -> str:
    text = USER_DIR.sub("<local>/users", text)
    text = ABS_PATH.sub(lambda m: "<local>/" + re.split(r"[\\/]", m.group(0).rstrip("\\/"))[-1], text)
    return SECRET.sub("<redacted>", text)


def sanitize_deep(value):
    if isinstance(value, str):
        return sanitize(value)
    if isinstance(value, list):
        return [sanitize_deep(item) for item in value]
    if isinstance(value, dict):
        return {key: sanitize_deep(item) for key, item in value.items()}
    return value


def build(src: pathlib.Path) -> dict[str, str]:
    """返回 {文件名: 快照内容}；缺文件就抛错（不静默跳过）。"""
    ledger = src / "LEDGER.md"
    claims = src / "CLAIMS.md"
    techmap = src / "TECHMAP.json"
    missing = [p.name for p in (ledger, claims, techmap) if not p.is_file()]
    if missing:
        raise FileNotFoundError(f"账本目录缺文件：{missing}（--src 现在指向 {src}）")

    entry_lines = [
        sanitize(line)
        for line in ledger.read_text(encoding="utf-8", errors="replace").splitlines()
        if ENTRY_RE.match(line)
    ]
    if not entry_lines:
        raise ValueError("LEDGER 里一条条目行都没抓到 —— 格式变了还是路径错了？失明不等于没重复")
    payload = json.loads(techmap.read_text(encoding="utf-8", errors="replace"))
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    return {
        "LEDGER.md": header(stamp) + "\n".join(entry_lines) + "\n",
        "CLAIMS.md": sanitize(claims.read_text(encoding="utf-8", errors="replace")),
        "TECHMAP.json": json.dumps(sanitize_deep(payload), ensure_ascii=False, indent=1) + "\n",
    }


def write(files: dict[str, str]) -> None:
    SNAPSHOT.mkdir(parents=True, exist_ok=True)
    for name, content in files.items():
        (SNAPSHOT / name).write_text(content, encoding="utf-8")
    print(f"快照已写：{SNAPSHOT.relative_to(ROOT).as_posix()}（{len(files)} 个文件）")


def digest(files: dict[str, str]) -> str:
    """内容哈希。**剔除「生成时间」行**：否则跨天重跑会让 `--check` 永远报过期，
    而它要判的是「快照与正本内容是否一致」，不是「哪天生成的」。"""
    h = hashlib.sha256()
    for name in sorted(files):
        h.update(name.encode())
        h.update(STAMP_LINE.sub("", files[name]).encode())
    return h.hexdigest()[:16]


def current_digest() -> str | None:
    try:
        files = {
            name: (SNAPSHOT / name).read_text(encoding="utf-8")
            for name in ("LEDGER.md", "CLAIMS.md", "TECHMAP.json")
        }
    except FileNotFoundError:
        return None
    return digest(files)


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--src", default=str(DEFAULT_SRC))
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    src = pathlib.Path(args.src)
    try:
        files = build(src)
    except (FileNotFoundError, ValueError, json.JSONDecodeError) as error:
        print(f"FAIL {error}")
        return 1
    want = digest(files)
    have = current_digest()
    if args.check:
        if have is None:
            print("FAIL 快照不存在 —— 先跑一次不带 --check 的同步")
            return 1
        if have != want:
            print(f"FAIL 快照已过期：账本现算 {want}，仓内快照 {have} ⇒ 重跑同步")
            return 1
        print(f"OK 快照与账本一致（{want}）")
        return 0
    write(files)
    print(f"digest {want}")
    return 0


def self_test() -> int:
    ok = True

    def check(label: str, actual, expected) -> None:
        nonlocal ok
        good = actual == expected
        print(("ok   " if good else "FAIL ") + label)
        ok = ok and good

    check("绝对路径只留最后一段", sanitize(r"仓 D:\KF\BSHSQ 里"), "仓 <local>/BSHSQ 里")
    check("正斜杠绝对路径", sanitize("见 D:/KF/trit/crates/trit-core 下"), "见 <local>/trit-core 下")
    check("用户目录整体替换", sanitize(r"C:\Users\lbx13\AppData\Local"), "<local>/users\\AppData\\Local")
    check("token 被抹掉", sanitize("token ghp_ABCDEFGHIJ1234"), "token <redacted>")
    check("Bearer 被抹掉", sanitize("Authorization: Bearer abc.def-123456"), "Authorization: <redacted>")
    check("相对路径不动", sanitize("crates/trit-net/src"), "crates/trit-net/src")
    check("条目行能抓", bool(ENTRY_RE.match("- [x] **A420 · 标题**（完成｜负责：某人）")), True)
    check("正文行不抓", bool(ENTRY_RE.match("  仓 <local>/BSHSQ，提交 10f684b")), False)

    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        src = pathlib.Path(tmp)
        (src / "LEDGER.md").write_text(
            "# 账本\n\n- [ ] **A1 · 甲**（负责：s1）\n  正文在 D:/KF/xyz 下\n- [x] **A1 · 乙**（负责：s2）\n",
            encoding="utf-8",
        )
        (src / "CLAIMS.md").write_text("| id | who |\n| --- | --- |\n", encoding="utf-8")
        (src / "TECHMAP.json").write_text('{"entries": []}', encoding="utf-8")
        files = build(src)
        check("快照保留两条条目行", files["LEDGER.md"].count("- ["), 2)
        check("快照丢掉正文路径", "D:/KF" in files["LEDGER.md"], False)

        (src / "LEDGER.md").unlink()
        try:
            build(src)
            check("缺文件必须抛错", False, True)
        except FileNotFoundError:
            check("缺文件必须抛错", True, True)

    print("自测结论：" + ("绿" if ok else "红"))
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
