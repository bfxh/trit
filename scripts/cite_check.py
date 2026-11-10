#!/usr/bin/env python3
"""引用门：文档里每一条引用都必须"可核验 + 被用到"，否则判红。

用法：
  python scripts/cite_check.py docs/12-*.md
  python scripts/cite_check.py docs/12-*.md --json
  python scripts/cite_check.py --self-test

口径（照本仓"引用分两档"的规矩）：
- 引用表每行：`| [S1] | kind | 标题 | 链接/出处 | status |`
  - `kind ∈ paper / project / spec / blog / book / standard`
  - `status ∈ verified`（页面实读）/ `abstract`（只读了摘要或项目页）/ `classic`（经典文献，未逐页核对）
- **verified / abstract** 必须给 `http(s)` 链接；**classic** 必须给"出处"（期刊/会议/年份），可以没有链接。
- 表里每条引用必须在正文里出现过（`[S1]` 形式）；正文里出现的 `[S9]` 必须在表里 ——
  两向都查，防"挂一堆没人用的引用"和"引用了不存在的来源"。
- 同一 id 不许重复；id 必须是 `S<数字>`。

为什么要有：文档里的"某论文说了"是最便宜的话术。这条门不判断内容对不对，
只保证**每条引用都能点开、且真的被用到**；内容真伪仍要人读（那是审稿，不是门）。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROW = re.compile(
    r"^\|\s*\[(?P<id>S\d+)\]\s*\|\s*(?P<kind>[a-z]+)\s*\|\s*(?P<title>[^|]+?)\s*\|\s*"
    r"(?P<where>[^|]*?)\s*\|\s*(?P<status>[a-z]+)\s*\|"
)
USED = re.compile(r"\[(?P<id>S\d+)\]")
KINDS = {"paper", "project", "spec", "blog", "book", "standard"}
STATUSES = {"verified", "abstract", "classic"}
URL = re.compile(r"^https?://\S+$")


def parse(text: str) -> tuple[list[dict], list[str]]:
    rows: list[dict] = []
    errors: list[str] = []
    for line in text.splitlines():
        if not line.lstrip().startswith("| [S"):
            continue
        match = ROW.match(line.strip())
        if not match:
            errors.append(f"引用表行格式不对：{line.strip()[:80]}")
            continue
        row = match.groupdict()
        row["title"] = row["title"].strip()
        row["where"] = row["where"].strip()
        rows.append(row)
    return rows, errors


def audit(text: str) -> tuple[list[str], list[str], dict]:
    """返回 (拦, 警, 统计)。"""
    red: list[str] = []
    warn: list[str] = []
    rows, parse_errors = parse(text)
    red.extend(parse_errors)

    # 表头存在但一条都没解析出来 ⇒ 当成"没表"，不许静默绿
    if "引用" in text and not rows and "[S" not in text:
        red.append("文档提到引用但既没有引用表也没有 [S#] 标记（要么补表，要么删掉\"引用\"字样）")

    seen: dict[str, dict] = {}
    for row in rows:
        rid = row["id"]
        if rid in seen:
            red.append(f"{rid} 重复登记")
            continue
        seen[rid] = row
        if row["kind"] not in KINDS:
            red.append(f"{rid} 的 kind 非法：{row['kind']!r}（允许 {', '.join(sorted(KINDS))}）")
        if row["status"] not in STATUSES:
            red.append(f"{rid} 的 status 非法：{row['status']!r}")
        if row["status"] in ("verified", "abstract") and not URL.match(row["where"]):
            red.append(f"{rid} 标了 {row['status']} 却没有可点的 http(s) 链接：{row['where']!r}")
        if row["status"] == "classic" and not row["where"]:
            red.append(f"{rid} 是经典文献，至少要写出处（期刊/会议/年份）")
        if not row["title"]:
            red.append(f"{rid} 没有标题")

    body = "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("| [S")
    )
    used = [m.group("id") for m in USED.finditer(body)]
    for rid in sorted(set(used)):
        if rid not in seen:
            red.append(f"正文引用了 {rid}，但引用表里没有")
    for rid in sorted(seen):
        if rid not in used:
            red.append(f"{rid} 登记了却从没在正文里用过（挂引用不加分）")
    duplicated = sorted({rid for rid in used if used.count(rid) > 3})
    if duplicated:
        warn.append(f"这些引用在正文里出现超过 3 次：{', '.join(duplicated)}（检查是否重复论证）")

    return red, warn, {"rows": len(rows), "used": len(set(used))}


def self_test() -> int:
    checks: list[tuple[str, bool]] = []

    def check(label: str, got, want) -> None:
        checks.append((label, got == want))

    def doc(rows: str, body: str) -> str:
        return ("# 标题\n\n| id | kind | 标题 | 链接 | status |\n|---|---|---|---|---|\n" + rows + "\n\n" + body + "\n")

    good = doc(
        "| [S1] | paper | 某论文 | https://arxiv.org/abs/1311.3903 | verified |\n"
        "| [S2] | book | 某经典 | 期刊A 2013 | classic |",
        "正文引用 [S1] 与 [S2]。",
    )
    check("合规文档 => 绿", audit(good)[0], [])

    check("verified 没链接 => 红",
          any("没有可点" in line for line in audit(doc(
              "| [S1] | paper | 某论文 | 见附件 | verified |", "正文 [S1]。"))[0]), True)
    check("classic 没出处 => 红",
          any("至少要写出处" in line for line in audit(doc(
              "| [S1] | book | 某经典 |  | classic |", "正文 [S1]。"))[0]), True)
    check("kind 非法 => 红",
          any("kind 非法" in line for line in audit(doc(
              "| [S1] | video | 某视频 | https://x.y | verified |", "正文 [S1]。"))[0]), True)
    check("status 非法 => 红",
          any("status 非法" in line for line in audit(doc(
              "| [S1] | paper | 某论文 | https://x.y | probably |", "正文 [S1]。"))[0]), True)
    check("id 重复 => 红",
          any("重复登记" in line for line in audit(doc(
              "| [S1] | paper | 甲 | https://x.y | verified |\n"
              "| [S1] | paper | 乙 | https://x.z | verified |", "正文 [S1]。"))[0]), True)
    check("登记但没用 => 红",
          any("从没在正文里用过" in line for line in audit(doc(
              "| [S1] | paper | 甲 | https://x.y | verified |\n"
              "| [S2] | paper | 乙 | https://x.z | verified |", "正文只用 [S1]。"))[0]), True)
    check("正文引用不存在 => 红",
          any("引用表里没有" in line for line in audit(doc(
              "| [S1] | paper | 甲 | https://x.y | verified |", "正文 [S9]。"))[0]), True)
    check("表头坏行 => 红",
          any("格式不对" in line for line in audit(doc(
              "| [S1] | paper | 甲 | https://x.y | verified |\n| [S2] x | 不是表 |", "正文 [S1]。"))[0]),
          True)
    repeated = doc("| [S1] | paper | 甲 | https://x.y | verified |",
                   "正文 [S1] [S1] [S1] [S1]。")
    check("同一引用出现超过 3 次 => 只警告不判红",
          (audit(repeated)[0], bool(audit(repeated)[1])), ([], True))
    check("空文档 => 绿（没有引用就没有债）", audit("# 空\n")[0], [])

    failed = [label for label, ok in checks if not ok]
    for label, ok in checks:
        print(f"  {'FAILED' if not ok else 'ok    '} {label}")
    print(f"self-test: {len(checks) - len(failed)}/{len(checks)} passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("path", nargs="?")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    if not args.path:
        print("FAIL 需要文档路径", file=sys.stderr)
        return 2
    path = pathlib.Path(args.path)
    if not path.is_file():
        print(f"FAIL 文件不存在：{path}", file=sys.stderr)
        return 2
    red, warn, stats = audit(path.read_text(encoding="utf-8"))
    if args.json:
        print(json.dumps({"tool": "cite-check", "rows": stats["rows"], "used": stats["used"],
                          "red": red, "warn": warn, "green": not red}, ensure_ascii=False, indent=2))
    else:
        print(f"引用 {stats['rows']} 条（正文用到 {stats['used']} 条）｜拦 {len(red)}｜警 {len(warn)}")
        for line in red:
            print(f"  [拦] {line}")
        for line in warn:
            print(f"  [警] {line}")
        print("结论：" + ("绿（每条引用都能点开、且真的被用到）" if not red else "红（引用不可核验或没被用到）"))
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
