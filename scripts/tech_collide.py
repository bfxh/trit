#!/usr/bin/env python3
"""同技术撞车门：两条在效期的声明指向**同一项技术**就双方点名；**换了域不算重复**。

用法：
  python scripts/tech_collide.py                      # 判定（拦 → 退出码 1）
  python scripts/tech_collide.py --claims <md> --techmap <json> --now <epoch>
  python scripts/tech_collide.py --self-test          # 正例 + 四类反证

为什么要有这个（2026-11-05 用户口径）：
  认领门只比**路径**是否相交，所以两个人在**不同文件里造同一个轮子**永远不会撞车；
  反过来，同一个技术在两个不同领域各自实现又常常是**合理的**
  ——「你这个东西不行不代表其他领域不行」。所以本门分三档：
  · 拦：两条在效期声明命中技术索引的**同一条目**（同技术同实现面）⇒ 双方必须对齐；
  · 警：命中的是**不同条目但标签高度重叠**（≥2 个共享标签）⇒ 不算违规，但双方都要看到对方的
    条目摘要、理由与实测读数，PR 里要写清适用域差异；
  · 警：声明命中不到任何条目 ⇒ 新技术先 `techmap.py add` 再开工（这才是防造轮子的正向面）。

索引正本不在本仓：`_handoff/TECHMAP.json` 与 repo-tools 的 `techmap.py`
（技术标签索引那次建起来），本门只读不写，避免两处真相。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import time

DEFAULT_CLAIMS = pathlib.Path("D:/KF/_handoff/CLAIMS.md")
DEFAULT_TECHMAP = pathlib.Path("D:/KF/_handoff/TECHMAP.json")
SHARED_TAGS_TO_WARN = 2
DEFAULT_LEDGER = pathlib.Path("D:/KF/_handoff/LEDGER.md")
STOPWORDS = {"ledger", "done", "note", "that", "with", "from", "see", "详见", "完成", "开工", "负责", "会话", "提出"}



ENTRY_RE = re.compile(r"^- \[[ xX]\] \*\*(A[0-9]+)([^*]*)\*\*")
OWNER_RE = re.compile(r"负责[：:]\s*([^｜|）)]+)")


def parse_entries(text: str) -> list[dict]:
    """读 LEDGER 的条目行：编号、开/关、标题与负责会话。"""
    rows: list[dict] = []
    for line in text.splitlines():
        match = ENTRY_RE.match(line)
        if not match:
            continue
        num, rest = match.group(1), line
        closed = line[3].lower() == "x"
        owner = OWNER_RE.search(line)
        rows.append(
            {
                "id": num,
                "title": match.group(2).strip(" ·")[:40],
                "closed": closed,
                "owner": (owner.group(1).strip() if owner else "?"),
                "words": set(
                    word
                    for word in re.findall(r"[A-Za-z]{4,}|[一-鿿]{2,}", rest)
                    if word not in STOPWORDS
                ),
            }
        )
    return rows


def judge_ledger(entries: list[dict]) -> list[tuple[str, str]]:
    """重复登记、状态自相矛盾、疑似同一件事被两家做。"""
    findings: list[tuple[str, str]] = []
    by_id: dict[str, list[dict]] = {}
    for entry in entries:
        by_id.setdefault(entry["id"], []).append(entry)
    for num, group in sorted(by_id.items()):
        if len(group) > 1:
            titles = {item["title"] for item in group}
            label = "编号被两件事复用" if len(titles) > 1 else "同一件事重复登记"
            findings.append(
                (
                    "拦",
                    f"LEDGER {label}：{num} 出现 {len(group)} 次，负责 "
                    + "、".join(item["owner"] for item in group)
                    + f"；标题 {sorted(titles)[:2]} ⇒ "
                    + ("说「A 完成了」指的是哪一件？必须重新取号" if len(titles) > 1 else "各自推进会互相覆盖"),
                )
            )
        if any(item["closed"] for item in group) and any(not item["closed"] for item in group):
            findings.append(("拦", f"LEDGER 里 {num} 同时有 open 与 [x] 两种状态，自相矛盾"))
    open_rows = [entry for entry in entries if not entry["closed"]]
    for first in range(len(open_rows)):
        for second in range(first + 1, len(open_rows)):
            a, b = open_rows[first], open_rows[second]
            if a["id"] == b["id"]:
                continue
            shared = a["words"] & b["words"]
            smaller = min(len(a["words"]), len(b["words"])) or 1
            if len(shared) >= 4 and len(shared) / smaller >= 0.5:
                findings.append(
                    (
                        "警",
                        f"疑似重复开工（不拦）：{a['id']}（{a['owner']}）与 {b['id']}（{b['owner']}）"
                        f"标题共享 {sorted(shared)[:6]} ⇒ 双方对齐是同一件事还是两件",
                    )
                )
    return findings


def parse_claims(text: str) -> list[dict]:
    rows: list[dict] = []
    for line in text.splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 7 or cells[0] in ("id", "") or set(cells[0]) <= {"-"}:
            continue
        try:
            claimed = int(cells[4])
            ttl = int(cells[5])
        except ValueError:
            continue
        rows.append(
            {
                "id": cells[0],
                "who": cells[1],
                "repo": cells[2],
                "paths": [path.strip() for path in cells[3].split(",") if path.strip()],
                "claimed_at": claimed,
                "ttl_min": ttl,
                "note": cells[6],
            }
        )
    return rows


def parse_techmap(payload: dict) -> list[dict]:
    entries = payload.get("entries", payload if isinstance(payload, list) else [])
    out: list[dict] = []
    for entry in entries:
        out.append(
            {
                "id": entry.get("id", "?"),
                "title": entry.get("title", ""),
                "tags": set(entry.get("tags", [])),
                "paths": list(entry.get("paths", [])),
                "why": entry.get("why", ""),
                "status": entry.get("status", ""),
                "repo": entry.get("repo", ""),
                "evidence": "; ".join(
                    item.get("value", "")
                    for item in entry.get("evidence", [])
                    if item.get("type") == "measurement"
                ),
            }
        )
    return out


def path_hit(claim_path: str, entry_path: str) -> bool:
    claim = claim_path.replace("\\", "/").strip("/")
    entry = entry_path.replace("\\", "/").strip("/")
    return claim == entry or entry.startswith(claim + "/") or claim.startswith(entry + "/")


def active(claim: dict, now: int) -> bool:
    return claim["claimed_at"] + claim["ttl_min"] * 60 >= now


def entry_for(claim: dict, entries: list[dict]) -> list[dict]:
    matched: list[dict] = []
    for entry in entries:
        if entry["repo"] != claim["repo"]:
            continue
        if any(path_hit(path, entry_path) for path in claim["paths"] for entry_path in entry["paths"]):
            matched.append(entry)
    return matched


def describe(entry: dict) -> str:
    evidence = f"｜读数 {entry['evidence']}" if entry["evidence"] else "｜读数（索引里没有 measurement 证据）"
    return f"{entry['id']}《{entry['title']}》[{entry['status']}]{evidence}：{entry['why']}"


def judge(claims: list[dict], entries: list[dict], now: int) -> list[tuple[str, str]]:
    """返回 (档位, 说明) 列表；档位 ∈ {拦, 警}，空列表 = 绿。"""
    live = [claim for claim in claims if active(claim, now)]
    hits = {f"{claim['id']}|{claim['who']}": entry_for(claim, entries) for claim in live}
    findings: list[tuple[str, str]] = []

    for claim in live:
        if not hits[f"{claim['id']}|{claim['who']}"]:
            findings.append(
                (
                    "警",
                    f"{claim['who']} 的声明（{claim['id']}）没命中任何技术索引条目：{claim['note'][:40]}… "
                    "⇒ 新技术先 `techmap.py add` 再开工，否则下一个智能体不知道该复用谁",
                )
            )

    for first in range(len(live)):
        for second in range(first + 1, len(live)):
            a, b = live[first], live[second]
            if a["repo"] != b["repo"]:
                continue
            a_hits = hits[f"{a['id']}|{a['who']}"]
            b_hits = hits[f"{b['id']}|{b['who']}"]
            shared = sorted({x["id"] for x in a_hits} & {y["id"] for y in b_hits})
            if shared:
                for entry_id in shared:
                    entry = next(x for x in entries if x["id"] == entry_id)
                    findings.append(
                        (
                            "拦",
                            f"同技术撞车：{a['who']}（{a['id']}）与 {b['who']}（{b['id']}）都在动条目 "
                            f"{describe(entry)} ⇒ 双方停一下对齐，别各造一份",
                        )
                    )
                continue
            for x in a_hits:
                for y in b_hits:
                    overlap = sorted(x["tags"] & y["tags"])
                    if len(overlap) >= SHARED_TAGS_TO_WARN:
                        findings.append(
                            (
                                "警",
                                f"异域同技术（不拦）：{a['who']} 在做 {describe(x)}；{b['who']} 在做 "
                                f"{describe(y)}；共享标签 {overlap} ⇒ 各自领域可以各自成立，"
                                "但 PR 里要写清适用域差异与为什么不复用",
                            )
                        )
    return findings


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="同技术撞车门（只读，不改认领与索引）")
    parser.add_argument("--claims", default=str(DEFAULT_CLAIMS))
    parser.add_argument("--techmap", default=str(DEFAULT_TECHMAP))
    parser.add_argument("--now", type=int, default=None)
    parser.add_argument("--ledger", default=str(DEFAULT_LEDGER))
    parser.add_argument("--no-ledger", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    claims_path = pathlib.Path(args.claims)
    tech_path = pathlib.Path(args.techmap)
    if not claims_path.is_file() or not tech_path.is_file():
        print(f"FAIL 读不到声明或技术索引：{claims_path} / {tech_path} —— 不知道有没有撞车不等于没撞车")
        return 1
    try:
        payload = json.loads(tech_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        print(f"FAIL 技术索引解析失败：{error}")
        return 1
    findings = judge(
        parse_claims(claims_path.read_text(encoding="utf-8", errors="replace")),
        parse_techmap(payload),
        args.now if args.now is not None else int(time.time()),
    )
    ledger_path = pathlib.Path(args.ledger)
    if not args.no_ledger:
        if not ledger_path.is_file():
            print(f"FAIL 读不到交接账本：{ledger_path} —— 不知道有没有重复登记不等于没重复")
            return 1
        findings = judge_ledger(parse_entries(ledger_path.read_text(encoding="utf-8", errors="replace"))) + findings
    blocked = [message for kind, message in findings if kind == "拦"]
    print(f"同技术撞车：拦 {len(blocked)} 条｜警 {len(findings) - len(blocked)} 条")
    for kind, message in findings:
        print(f"  [{kind}] {message}")
    if blocked:
        print("FAIL 撞车或账本重复未处理（见上表：同技术撞车 / 编号复用 / 重复登记）")
        return 1
    print("OK 没有同技术撞车（异域重叠只报警告）")
    return 0


def self_test() -> int:
    failures = 0

    def claim(who: str, paths: list[str], note: str = "在做扫描", claimed: int = 1000, ttl: int = 60) -> dict:
        return {
            "id": who[-1],
            "who": who,
            "repo": "o/r",
            "paths": paths,
            "claimed_at": claimed,
            "ttl_min": ttl,
            "note": note,
        }

    entries = parse_techmap(
        {
            "entries": [
                {
                    "id": "scan-boost",
                    "title": "更强的扫描器",
                    "tags": ["scanner", "perf"],
                    "paths": ["crates/a/src/lib.rs"],
                    "why": "一条路走到底",
                    "status": "landed",
                    "repo": "o/r",
                    "evidence": [{"type": "measurement", "value": "1.9×"}],
                },
                {
                    "id": "scan-vision",
                    "title": "视觉侧扫描",
                    "tags": ["scanner", "perf", "vision"],
                    "paths": ["crates/b/src/lib.rs"],
                    "why": "另一个领域，结论不通用",
                    "status": "wip",
                    "repo": "o/r",
                    "evidence": [{"type": "measurement", "value": "召回 92%"}],
                },
            ]
        }
    )

    def expect(label: str, findings: list[tuple[str, str]], want: tuple[str, str] | None) -> None:
        nonlocal failures
        if want is None:
            ok = not findings
        else:
            kind, needle = want
            ok = any(k == kind and needle in message for k, message in findings)
        if ok:
            print(f"  ok   {label}")
        else:
            failures += 1
            print(f"  FAIL {label}: {findings}")

    now = 1000 + 30 * 60
    expect(
        "正例：只有一条声明且命中条目 ⇒ 不响",
        judge([claim("A", ["crates/a/src"])], entries, now),
        None,
    )
    expect(
        "反证 1：两条声明命中同一条目 ⇒ 拦，双方都点名",
        judge([claim("A", ["crates/a/src"]), claim("B", ["crates/a/src/lib.rs"])], entries, now),
        ("拦", "同技术撞车"),
    )
    mixed = judge([claim("A", ["crates/a/src"]), claim("B", ["crates/b/src"])], entries, now)
    expect(
        "反证 2：异域同标签 ⇒ 只警，并把对方条目与读数印出来",
        mixed,
        ("警", "异域同技术"),
    )
    if mixed and not any("1.9×" in message and "召回 92%" in message for _, message in mixed):
        failures += 1
        print(f"  FAIL 反证 2 的警告没带上双方读数：{mixed}")
    expect(
        "反证 3：命中不到条目 ⇒ 提示先登记索引",
        judge([claim("C", ["crates/z/src"])], entries, now),
        ("警", "techmap.py add"),
    )
    expect(
        "反证 4：过期声明不参与（否则永远拦）",
        judge([claim("A", ["crates/a/src"]), claim("B", ["crates/a/src"], claimed=1, ttl=1)], entries, now),
        None,
    )

    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        missing = pathlib.Path(tmp) / "nope.md"
        blocked = main(["--claims", str(missing), "--techmap", str(missing)])
        if blocked == 1:
            print("  ok   反证 5：读不到索引/声明必须 fail-closed（rc=1）")
        else:
            failures += 1
            print(f"  FAIL 反证 5：读不到索引却返回 rc={blocked}，等于放行")

    print("自测结论：%s" % ("绿" if failures == 0 else f"红 {failures} 条"))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
