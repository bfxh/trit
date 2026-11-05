#!/usr/bin/env python3
"""完成度表对账门：`docs/STATUS.md` §2 那张表的数字必须是**刚量出来的**。

动机（反"我以为做完了"）：那张表是人工数出来的（内联 `#[test]`、被 bin 用、`tests/` 提及、判定词）。
代码在动，表不动 —— 于是"已跑通/组件"的判定会悄悄变成假话，而这正是用户最反感的迷惑性来源
（"我以为那些小模型是各种功能，结果是组件"）。本门把表里的四列计数和判定词全部按同一把尺复算，
对不上就判红；表读不出来也判红（不知道有没有对账 ≠ 对上了）。

用法：
  python scripts/model_status.py                 # 对账
  python scripts/model_status.py --json          # 机器可读
  python scripts/model_status.py --self-test     # 正例 + 四类反证
  python scripts/model_status.py --show          # 打印复算出来的表（贴回文档用）
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
DOC = ROOT / "docs" / "STATUS.md"

# §1 的判定词表（新增档位要同步改 docs/STATUS.md，否则本门不认识它）。
VERDICTS = ("已跑通", "已实现未测", "组件", "已否证")

ROW = re.compile(r"^\| `?([A-Za-z0-9_]+)`? \|")
NUM = re.compile(r"(\d+)")
FILE_REF = re.compile(r"`([A-Za-z0-9_./-]+\.rs):?(\d+)?`")

# 表头：| 名字 | 定义处 | 内联 `#[test]` | 被 bin 用 | 被仓内其他模块用 | `tests/` 端到端 | 读数在哪 | 按 §1 判 |
HEADER = ("名字", "定义处", "内联")


def mentions(name: str, path: pathlib.Path) -> int:
    """该标识符出现的**代码行**数（注释/文档行不算，否则"提了一句"会冒充"用了"）。

    同一个尺也用来数 `#[test]`：那是字面子串，没有词边界可言。
    """
    code = [
        line
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines()
        if not line.lstrip().startswith(("//", "/*", "*"))
    ]
    if name == "#[test]":
        return sum(1 for line in code if "#[test]" in line)
    pat = re.compile(rf"\b{re.escape(name)}\b")
    return sum(1 for line in code if pat.search(line))


def measure(crate_dir: pathlib.Path, name: str, def_file: str) -> dict[str, int]:
    """四列计数：内联测试 / 被 bin 用 / 被其他模块用 / tests/ 提及。"""
    own = crate_dir / "src" / def_file
    inline = mentions("#[test]", own) if own.exists() else -1
    bins = 0
    bin_dir = crate_dir / "src" / "bin"
    if bin_dir.exists():
        bins = sum(1 for p in bin_dir.rglob("*.rs") if mentions(name, p) > 0)
    others = 0
    tests = 0
    for p in (crate_dir / "src").rglob("*.rs"):
        if p == own or bin_dir in p.parents:
            continue
        if mentions(name, p) > 0:
            others += 1
    tests_dir = crate_dir / "tests"
    if tests_dir.exists():
        tests = sum(mentions(name, p) for p in sorted(tests_dir.glob("*.rs")))
    return {"inline": inline, "bins": bins, "others": others, "tests": tests}


def parse_table(text: str) -> list[dict[str, str]]:
    """抓 §2 那张表的数据行；抓不到就报错（表头漂移 = 本门失明）。"""
    rows: list[dict[str, str]] = []
    in_section = False
    header_seen = False
    for line in text.splitlines():
        if line.startswith("## "):
            in_section = line.startswith("## 2.") or " §2" in line
            continue
        if not in_section or not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 8:
            continue
        if all(h in line for h in HEADER):
            header_seen = True
            continue
        if not header_seen:
            continue
        if set(cells[0]) <= set("-: "):
            continue
        m = ROW.match(line)
        if not m:
            continue
        rows.append(
            {
                "name": m.group(1),
                "ref": cells[1],
                "inline": cells[2],
                "bins": cells[3],
                "others": cells[4],
                "tests": cells[5],
                "reading": cells[6],
                "verdict": cells[7],
            }
        )
    if not header_seen:
        raise ValueError(f"{DOC.name} 里找不到 §2 那张表的表头（口径漂移，门失明）")
    if not rows:
        raise ValueError("§2 表没有数据行")
    return rows


def declared(cell: str) -> int | None:
    m = NUM.search(cell.replace("**", ""))
    return int(m.group(1)) if m else None


def verdict_of(cell: str) -> str | None:
    plain = cell.replace("**", "")
    for v in VERDICTS:
        if plain.startswith(v):
            return v
    return None


def check(rows: list[dict[str, str]], crate_dir: pathlib.Path) -> list[str]:
    problems: list[str] = []
    for row in rows:
        name = row["name"]
        ref = FILE_REF.search(row["ref"])
        if not ref:
            problems.append(f"{name}：定义处没写成 `文件.rs:行`（本门按文件路径复算，指不到就没法量）")
            continue
        got = measure(crate_dir, name, ref.group(1))
        if got["inline"] < 0:
            problems.append(f"{name}：{(crate_dir / 'src' / ref.group(1)).as_posix()} 不存在，表还引用它")
            continue
        for key, label in (
            ("inline", "内联 #test"),
            ("bins", "被 bin 用"),
            ("others", "被其他模块用"),
            ("tests", "tests/ 提及"),
        ):
            want = declared(row[key])
            if want is None:
                problems.append(f"{name}：{label} 那格不是数字（{row[key]!r}），无法对账")
            elif want != got[key]:
                problems.append(
                    f"{name}：{label} 表里写 {want}，按 §2 复算尺量到 {got[key]} —— "
                    "要么代码动了表没动，要么口径变了；改表前先重跑 `python scripts/model_status.py --show`"
                )
        verdict = verdict_of(row["verdict"])
        if verdict is None:
            problems.append(
                f"{name}：判定「{row['verdict']}」不在 §1 词表 {list(VERDICTS)} 里（档位漂移）"
            )
        # 判定与计数的一致性：已跑通必须有 tests/ 级断言，或把读数出处指到具体文件/§/PR。
        if verdict == "已跑通" and got["tests"] == 0:
            pointer = bool(
                re.search(r"`[^`]*\.(rs|md|json)`|§|PR\s*#\d+|A\d{2,}", row["reading"])
            )
            if not pointer:
                problems.append(
                    f"{name}：判「已跑通」但 `tests/` 提及 0，且「读数在哪」没指到文件/§/PR —— "
                    "内联单点测试撑不起这个档位（见 §1）"
                )
        # 命名迷惑性：名字像整机（Net/Model/UNet）却判成组件 ⇒ 提醒改名或在文档里说明。
        # 已经在表里写明"名字像整机 / 已否证改名"的，不再重复催（催了也不改的警会被人当噪音）。
        if verdict == "组件" and re.search(r"(Net|Model|UNet)$", name):
            disclosed = any(
                key in row["verdict"] + row["reading"] for key in ("名字像整机", "否证", "§4")
            )
            if not disclosed:
                problems.append(
                    f"警：{name} 判「组件」但名字像整机 —— 按 §1 要么补到已跑通，要么改名/在表里写明它是组件"
                )
    return problems


def show(rows: list[dict[str, str]], crate_dir: pathlib.Path) -> str:
    out = []
    for row in rows:
        ref = FILE_REF.search(row["ref"])
        if not ref:
            out.append(f"{row['name']}: 定义处无法解析")
            continue
        got = measure(crate_dir, row["name"], ref.group(1))
        out.append(
            f"| `{row['name']}` | `{ref.group(1)}:{ref.group(2) or '?'}` | {got['inline']} | "
            f"{got['bins']} | {got['others']} | **{got['tests']}** | {row['reading']} | {row['verdict']} |"
        )
    return "\n".join(out)



def main(argv: list[str]) -> int:
    # 输出里有 `⇒` 这类 GBK 装不下的字符：中文 Windows 上 mutation 门用默认编码起子进程会直接
    # UnicodeEncodeError（表现为"门自己崩了"），所以这里显式把 stdout 定成 UTF-8。
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--crate", default="trit-parts", help="对账哪个 crate 的模型表（默认 trit-parts）")
    parser.add_argument("--show", action="store_true", help="打印复算出来的表行")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    crate_dir = ROOT / "crates" / args.crate
    if not crate_dir.exists():
        print(f"FAIL 找不到 crate：{crate_dir}")
        return 1
    try:
        rows = parse_table(DOC.read_text(encoding="utf-8"))
    except (OSError, ValueError) as err:
        print(f"FAIL 表读不出来：{err}")
        return 1
    if args.show:
        print(show(rows, crate_dir))
        return 0
    problems = check(rows, crate_dir)
    if args.json:
        print(json.dumps({"rows": len(rows), "problems": problems}, ensure_ascii=False))
    else:
        print(f"完成度表对账：{len(rows)} 行 × 4 列计数 + 判定词")
        for p in problems:
            print(("  " if p.startswith("警") else "  拦：") + p)
    hard = [p for p in problems if not p.startswith("警")]
    print(("FAIL 拦 %d 条" % len(hard)) if hard else "OK 表里的数字与判定都和代码对得上")
    return 1 if hard else 0


def self_test() -> int:
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="model-status-self"))
    src = tmp / "src"
    (src / "bin").mkdir(parents=True, exist_ok=True)
    (tmp / "tests").mkdir(parents=True, exist_ok=True)
    (src / "thing.rs").write_text(
        "pub struct Thing;\n#[cfg(test)]\nmod tests {\n  #[test]\n  fn a() {}\n  #[test]\n  fn b() {}\n}\n",
        encoding="utf-8",
    )
    (src / "other.rs").write_text("use crate::thing::Thing;\nfn f(_: Thing) {}\n", encoding="utf-8")
    (src / "bin" / "t_thing.rs").write_text("fn main() { let _ = trit_parts::thing::Thing; }\n", encoding="utf-8")
    (tmp / "tests" / "e2e.rs").write_text("// Thing\nfn _t(_: trit_parts::thing::Thing) {}\nfn _u(_: trit_parts::thing::Thing) {}\n", encoding="utf-8")
    got = measure(tmp, "Thing", "thing.rs")
    ok = True

    def expect(label: str, bad: list[str], want: str | None) -> None:
        nonlocal ok
        good = (not bad) if want is None else any(want in b for b in bad)
        print(("ok   " if good else "FAIL ") + label)
        ok = ok and good

    if got != {"inline": 2, "bins": 1, "others": 1, "tests": 2}:
        print(f"FAIL 复算尺本身：{got}")
        ok = False
    else:
        print("ok   复算尺：内联 2 / bin 1 / 其他 1 / tests 2")

    good_row = {
        "name": "Thing",
        "ref": "`thing.rs:1`",
        "inline": "2",
        "bins": "1",
        "others": "1",
        "tests": "2",
        "reading": "本文件 §2.1",
        "verdict": "**已跑通**",
    }
    expect("正例：数字与判定都对得上", check([dict(good_row)], tmp), None)

    stale = dict(good_row, inline="7")
    expect("反证 1：表里的内联数没跟着代码动", check([stale], tmp), "表里写 7")

    # 反证 2 要用"实测真的 0 提及"的夹具：只把表里的数字改成 0 是打不到这条判据的
    # （判据看的是量出来的 tests，不是表里写的）。
    lone = tmp / "lone"
    (lone / "src").mkdir(parents=True, exist_ok=True)
    (lone / "src" / "gadget.rs").write_text(
        "pub struct Gadget;\n#[cfg(test)]\nmod t {\n  #[test]\n  fn a() {}\n}\n", encoding="utf-8"
    )
    gadget = {
        "name": "Gadget",
        "ref": "`gadget.rs:1`",
        "inline": "1",
        "bins": "0",
        "others": "0",
        "tests": "0",
        "reading": "—",
        "verdict": "**已跑通**",
    }
    expect("反证 2：实测 tests 提及 0 又没指到读数 ⇒ 撑不起已跑通", check([gadget], lone), "读数在哪")
    gadget_ok = dict(gadget, reading="`docs/PARTS-RESULTS.md`")
    expect("反证 2b：tests 为 0 但读数指到文件 ⇒ 只放行这一种", check([gadget_ok], lone), None)

    drift = dict(good_row, verdict="**基本能用**")
    expect("反证 3：判定词漂移", check([drift], tmp), "词表")

    misleading = dict(good_row, name="ThingNet", verdict="**组件**")
    expect("反证 4：名字像整机却判组件要点名", check([misleading], tmp), "像整机")
    # ThingNet 这个符号在夹具里谁都没用 ⇒ 计数要按实测填，否则先撞在计数判红上。
    disclosed = {
        **misleading,
        "bins": "0",
        "others": "0",
        "tests": "0",
        "verdict": "**组件**（名字像整机，§4 已否证改名）",
    }
    expect("反证 4b：表里已经写明的不再重复催", check([disclosed], tmp), None)

    missing = dict(good_row, ref="`gone.rs:1`")
    expect("反证 5：定义处文件已不存在", check([missing], tmp), "不存在")

    try:
        parse_table("# 没有这张表\n")
        print("FAIL 反证 6：表头缺失应报错")
        ok = False
    except ValueError:
        print("ok   反证 6：读不到表头就判失败（失明不等于对上了）")

    print("自测结论：" + ("绿" if ok else "红"))
    return 0 if ok else 1

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
