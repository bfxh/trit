#!/usr/bin/env python3
"""文档卫生门：CHANGELOG 不能被写坏，仓内文档不许新增本机盘符路径。

用法：
  python scripts/docs_hygiene.py              # 判定；违规判红，退出码 1
  python scripts/docs_hygiene.py --update     # 收紧棘轮（任何计数变大就整体拒绝写入）
  python scripts/docs_hygiene.py --self-test  # 正例 + 五类反证

为什么要有这个（2026-11-05 的真实事故）：一次脚本化追加 CHANGELOG 时，同一个名字先当路径
再当内容用，于是整个 173 KB 的文件被写成一行人机绝对路径 —— 它还跟着 PR 合进了 main。
`docs/SCOPE.md` §4 说「仓内文档记读数与口径，不记机器状态」，没有门就只是句客气话。

口径：
  · CHANGELOG.md 首行必须 `# Changelog`、必须含 `## [Unreleased]`、字节数不得低于棘轮基线
    （跌破 = 有人把整段历史删了或覆盖丢了）；
  · 仓内可见文件里的**盘符绝对路径**（`X:/...` 或 `X:\\...`）按文件计数，只许变少不许变多；
  · 基线读不出、文件形状不对 ⇒ 判红（"不知道有多少"不等于"没有"）。
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = pathlib.Path(__file__).resolve().parent / "docs_hygiene_baseline.json"

DRIVE_PATH = re.compile(r"[A-Za-z]:[/\\][A-Za-z0-9_.~-]")
SCANNED_TOP = ["CHANGELOG.md", "README.md"]


def scanned_files(root: pathlib.Path) -> list[pathlib.Path]:
    out: list[pathlib.Path] = []
    for name in SCANNED_TOP:
        path = root / name
        if path.is_file():
            out.append(path)
    docs = root / "docs"
    if docs.is_dir():
        out += sorted(docs.rglob("*.md"))
    return out


def count_drive_paths(root: pathlib.Path, files: list[pathlib.Path]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for path in files:
        hits = len(DRIVE_PATH.findall(path.read_text(encoding="utf-8", errors="replace")))
        if hits:
            counts[path.relative_to(root).as_posix()] = hits
    return counts


def check(payload: dict, changelog: str) -> list[str]:
    """纯判定：payload = 基线，changelog = CHANGELOG.md 全文。返回违规清单。"""
    problems: list[str] = []
    if "changelog" not in payload or "drive_paths" not in payload:
        return [f"基线缺字段：{payload!r:.80} —— 没表不等于没违规，判红"]
    first, _, rest = changelog.partition("\n")
    if first.strip() != "# Changelog":
        problems.append(
            f"CHANGELOG 首行是 {first[:40]!r}，应为 '# Changelog'（整文件被覆盖成一行人机路径就是这样）"
        )
    if "## [Unreleased]" not in changelog:
        problems.append("CHANGELOG 里没有 ## [Unreleased] 段落")
    floor = int(payload["changelog"])
    if len(changelog.encode("utf-8")) < floor:
        problems.append(
            f"CHANGELOG 字节数低于棘轮基线 {floor}：删历史或覆盖丢失不会让任何测试变红，所以在这里拦"
        )
    for path, allowed in payload["drive_paths"].items():
        if allowed < 0:
            problems.append(f"基线计数为负：{path} = {allowed}")
    return problems


def check_drive_paths(found: dict[str, int], baseline: dict[str, int]) -> list[str]:
    problems: list[str] = []
    for path, count in sorted(found.items()):
        allowed = baseline.get(path, 0)
        if count > allowed:
            problems.append(
                f"{path} 有 {count} 处盘符绝对路径，基线允许 {allowed} ——"
                " 仓内文档记读数与口径，不记机器路径（docs/SCOPE.md §4）"
            )
    for path, allowed in sorted(baseline.items()):
        if path not in found and allowed > 0:
            problems.append(f"陈旧基线：{path} 的 {allowed} 处已经清了，把基线 --update 归零")
    return problems


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    if not BASELINE.is_file():
        print(f"FAIL 基线读不出：{BASELINE} 不存在")
        return 1
    payload = json.loads(BASELINE.read_text(encoding="utf-8"))
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    problems = check(payload, changelog)
    problems += check_drive_paths(count_drive_paths(ROOT, scanned_files(ROOT)), payload["drive_paths"])
    print(f"文档卫生：{len(scanned_files(ROOT))} 个文件在扫，CHANGELOG 基线 {payload['changelog']} 字节")
    for problem in problems:
        print(f"  拦：{problem}")
    if problems:
        print(f"FAIL 拦 {len(problems)} 条")
        return 1
    print("OK CHANGELOG 形状完好，盘符路径没有新增")
    return 0


def self_test() -> int:
    failures = 0

    def expect(label: str, problems: list[str], want: str | None) -> None:
        nonlocal failures
        hit = (not problems) if want is None else any(want in line for line in problems)
        if hit:
            print(f"  ok   {label}")
        else:
            failures += 1
            print(f"  FAIL {label}: {problems}")

    good_payload = {"changelog": 100, "drive_paths": {"docs/A.md": 2}}
    good_text = "# Changelog\n\n## [Unreleased]\n" + "x" * 200
    expect("正例：形状完好 + 路径不超基线", check(good_payload, good_text), None)
    expect(
        "正例 2：路径扫描按基线放行",
        check_drive_paths({"docs/A.md": 2}, good_payload["drive_paths"]),
        None,
    )
    expect(
        "反证 1：整文件被覆盖成一行人机路径",
        check(good_payload, "C:/repo/CHANGELOG.md"),
        "首行",
    )
    expect("反证 2：丢了 Unreleased 段", check(good_payload, "# Changelog\n" + "y" * 200), "Unreleased")
    expect("反证 3：字节数跌破棘轮基线", check(good_payload, "# Changelog\n\n## [Unreleased]\ntiny"), "低于棘轮基线")
    expect(
        "反证 4：新增了盘符路径",
        check_drive_paths({"docs/A.md": 3, "docs/B.md": 1}, good_payload["drive_paths"]),
        "docs/A.md",
    )
    expect(
        "反证 5：基线形状不对（没表不等于没违规）",
        check({"oops": 1}, good_text),
        "基线缺字段",
    )
    expect(
        "反证 6：陈旧基线要归零",
        check_drive_paths({}, {"docs/A.md": 2}),
        "陈旧基线",
    )
    print("自测结论：%s" % ("绿" if failures == 0 else f"红 {failures} 条"))
    return 1 if failures else 0


def update() -> int:
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    found = count_drive_paths(ROOT, scanned_files(ROOT))
    old = json.loads(BASELINE.read_text(encoding="utf-8")) if BASELINE.is_file() else {}
    problems = check(old or {"changelog": 0, "drive_paths": {}}, changelog)
    problems += check_drive_paths(found, old.get("drive_paths", {}))
    grew = [
        f"{path}: {old.get('drive_paths', {}).get(path, 0)} → {count}"
        for path, count in sorted(found.items())
        if count > old.get("drive_paths", {}).get(path, 0)
    ]
    if grew and old:
        print("REFUSED 盘符路径变多了，--update 不接受：" + "；".join(grew))
        return 1
    if problems and old:
        print("REFUSED 当前文件形状已不合格，先修再登记：" + "；".join(problems))
        return 1
    payload = {"changelog": len(changelog.encode("utf-8")), "drive_paths": found}
    BASELINE.write_text(
        json.dumps(payload, ensure_ascii=False, indent=1, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"台账已写：CHANGELOG {payload['changelog']} 字节，{len(found)} 个文件仍有盘符路径 → {BASELINE.name}")
    return 0


if __name__ == "__main__":
    if "--update" in sys.argv[1:]:
        raise SystemExit(update())
    raise SystemExit(main(sys.argv[1:]))
