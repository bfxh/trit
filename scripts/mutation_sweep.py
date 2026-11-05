#!/usr/bin/env python3
"""全表锚点 sweep：把 `preflight` 从「只扫被 `--pkg` 选中的那批」变成「每次必扫整张表」。

为什么单独一个脚本，而不改 `mutation_check.py`：门本体正被另一路改动（撤过期豁免），
同一文件两个写者必撞。这里只做一件事——`from mutation_check import load_mutants, preflight`
把现成的判据拿过来跑全表，一行都不改门本体；判据若要变，变的是门，不是这里。

盲区的形状（不是设想，是同仓真发生过的事）：`mutation_check.main()` 只对 `select(pkgs)`
选中的条目调用 `preflight`，所以**没被选中的目标里，锚点烂掉了没人报**。
搬代码的那一轮（拆文件、把 `old` 锚点留在旧路径）就是这个盲区最容易中招的时候：
条目还在、文件还在、锚点数是 0 —— 而那个包当天根本没进 CI 的 `--pkg` 列表。

纪律（每条都可测）：
- 「没表」绝不等于「没变异」：登记表目录缺失由 loader 直接判红，本脚本不吞这个异常；
- 跨目标**重名**判红：两条同名条目会让「哪条被抓了」的账对不上（#220 拆表时算过 0 重名）；
- 扫了多少条必须**打印出来**：打印 0 条 = 门在空转，与红同罪；
- usage 错误打 `FAIL ` 前缀并返回 2（管道会吞 rc，前缀让日志里看得见没跑成）。
"""

import contextlib
import io
import json
import pathlib
import sys
import tempfile
import time

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from mutation_check import ROOT, load_mutants, preflight  # noqa: E402  (判据复用，不复制)

MUTANT_DIR = SCRIPT_DIR / "mutants"


def duplicate_names(entries: list[dict]) -> list[str]:
    """跨目标重名的条目名（同名会让「谁抓住了哪条变异」的账对不上）。"""
    seen: set[str] = set()
    twice: list[str] = []
    for entry in entries:
        if entry["name"] in seen:
            twice.append(entry["name"])
        else:
            seen.add(entry["name"])
    return sorted(set(twice))


def sweep(directory: pathlib.Path = MUTANT_DIR, root: pathlib.Path = ROOT) -> tuple[int, list[str]]:
    """扫整张表：返回（条目数，问题清单）。问题清单为空 = 全表锚点都还落在产品代码上。"""
    entries = load_mutants(directory)
    problems = preflight(entries, root=root)
    for name in duplicate_names(entries):
        problems.append(f"重名条目：{name}")
    return len(entries), problems


def main(argv: list[str], directory: pathlib.Path = MUTANT_DIR, root: pathlib.Path = ROOT) -> int:
    """CLI：默认扫全表；`--self-test` 只验本脚本自身的判据，不读真登记表。

    `directory`/`root` 是给自测留的注入口：CI 吃的是**退出码**，所以 rc 的接线
    必须由走 `main()` 的用例钉住——只测纯函数会留下「判据对、码错」的空档。
    """
    for index, token in enumerate(argv):
        if token.startswith("--") and token not in ("--self-test",):
            print(f"FAIL 用法：mutation_sweep.py 不认识参数 {token}（可用：--self-test）")
            return 2
        if token == "--self-test":
            return self_test()
        if not token.startswith("-") and index == 0:
            print(f"FAIL 用法：mutation_sweep.py 不收位置参数（收到 {token}）")
            return 2

    started = time.perf_counter()
    count, problems = sweep(directory, root)
    targets = len(list(directory.glob("*.json")))
    for problem in problems:
        print(f"锚点腐烂：{problem}")
    print(
        f"全表 sweep：{count} 条 / {targets} 个目标 ｜ 问题 {len(problems)} 个"
        f" ｜ {time.perf_counter() - started:.2f}s"
    )
    if problems:
        print("⇒ 门判红：锚点漂了就把登记表的 `file`/`old` 跟着搬，删条目等于放宽门。")
        return 1
    if count == 0:
        # loader 不会让这发生（目录不存在先炸），留着是给"表被清空"这条路上一道死闸。
        print("⇒ 门判红：0 条不等于全绿。")
        return 1
    return 0


def self_test() -> int:
    """正反例都在这：正例证明扫得动，反例逐条证明「什么形状会被抓到」。

    不碰 cargo、不修真登记表，毫秒级。失败打 `FAIL <例名>`，与门脚本族同口径。
    """
    total = 0
    failures: list[str] = []

    def check(label: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(f"FAIL {label}: 期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        table = root / "scripts" / "mutants"
        table.mkdir(parents=True)
        source = root / "scripts"
        (source / "gate.py").write_text("x = 1\ndef helper():\n    return 2\n", encoding="utf-8")

        good = {
            "name": "gate: 好的",
            "file": "scripts/gate.py",
            "old": "    return 2",
            "new": "    return 3",
            # preflight 会按 pkg 推 runner（cargo/python/bash），少这个键直接 KeyError——
            # 夹具必须长成真的形状，不然自测是在验一个不存在的接口。
            "pkg": "scripts/gate.py",
        }
        (table / "a.json").write_text(json.dumps([good], ensure_ascii=False, indent=1), encoding="utf-8")
        count, problems = sweep(table, root=root)
        check("合法表：条目数", count, 1)
        check("合法表：零问题", problems, [])

        # 反例一：锚点漂移（改过代码没改表）——这正是不被 `--pkg` 选中时会漏掉的那类
        (source / "gate.py").write_text("x = 1\ndef helper():\n    return 42\n", encoding="utf-8")
        _, problems = sweep(table, root=root)
        check("锚点没了要点名", [p.startswith("gate: 好的") for p in problems], [True])

        # 反例二：文件不存在
        (table / "b.json").write_text(
            json.dumps(
                [{"name": "gate: 指错文件", "file": "scripts/gone.py", "old": "x", "new": "y",
                 "pkg": "scripts/gone.py"}],
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        _, problems = sweep(table, root=root)
        check("文件不存在要点名", any("gone.py" in p for p in problems), True)

        # 反例三：跨目标重名
        (source / "gate.py").write_text("x = 1\ndef helper():\n    return 2\n", encoding="utf-8")
        (table / "c.json").write_text(json.dumps([good], ensure_ascii=False), encoding="utf-8")
        _, problems = sweep(table, root=root)
        check("重名要判红", any("重名" in p for p in problems), True)

        # 反例四：锚点出现两次（"恰好 1 次"的另一侧）
        (table / "c.json").write_text("[]", encoding="utf-8")
        (source / "twice.py").write_text("v\nv\n", encoding="utf-8")
        (table / "d.json").write_text(
            json.dumps(
                [{"name": "gate: 锚点两处", "file": "scripts/twice.py", "old": "v", "new": "w",
                 "pkg": "scripts/twice.py"}],
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        _, problems = sweep(table, root=root)
        check("锚点多处要点名", any("出现 2 次" in p for p in problems), True)

        # 反例五：没表 ≠ 没有变异（loader 自己判红，本脚本不吞）
        try:
            sweep(root / "scripts" / "nope", root=root)
            check("目录缺失要抛", False, True)
        except SystemExit as exit_error:
            check("目录缺失点名是哪个目录", "nope" in str(exit_error), True)

        # 六、退出码的接线（CI 吃的是 rc，不是打印）：三条都要走 main()
        # 过程输出丢掉：自测里 main() 会打 `FAIL 用法：…`（那是它该有的行为），
        # 但一份绿的 self-test 日志里出现 FAIL 形状的行，下一个人就会误判门红了。
        with contextlib.redirect_stdout(io.StringIO()):
            rc_dirty = main([], table, root)
            rc_positional = main(["顺手传的"], table, root)
        check("走 main：表里有腐烂 ⇒ rc=1", rc_dirty, 1)
        check("走 main：位置参数 ⇒ rc=2", rc_positional, 2)

        clean_root = root / "clean"
        clean_table = clean_root / "scripts" / "mutants"
        clean_table.mkdir(parents=True)
        (clean_root / "scripts").joinpath("gate.py").write_text(
            "x = 1\ndef helper():\n    return 2\n", encoding="utf-8"
        )
        (clean_table / "a.json").write_text(json.dumps([good], ensure_ascii=False), encoding="utf-8")
        with contextlib.redirect_stdout(io.StringIO()):
            rc_clean = main([], clean_table, clean_root)
        check("走 main：全表干净 ⇒ rc=0", rc_clean, 0)

        # 七、两条死闸各自都要有人验：空表不等于全绿、未知旗标不能当成一次正常运行
        empty_root = root / "empty"
        empty_table = empty_root / "scripts" / "mutants"
        empty_table.mkdir(parents=True)
        (empty_table / "a.json").write_text("[]", encoding="utf-8")
        with contextlib.redirect_stdout(io.StringIO()):
            rc_empty = main([], empty_table, empty_root)
        check("走 main：表被清空 ⇒ rc=1（0 条不算绿）", rc_empty, 1)
        with contextlib.redirect_stdout(io.StringIO()):
            rc_bogus_flag = main(["--bogus"], table, root)
        check("走 main：未知旗标 ⇒ rc=2（不能顺手当正常运行）", rc_bogus_flag, 2)

    print(f"self-test: {total - len(failures)}/{total} passed")
    for failure in failures:
        print(failure)
    return 0 if not failures else 1


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(main(sys.argv[1:]))
