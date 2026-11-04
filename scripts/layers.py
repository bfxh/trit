#!/usr/bin/env python3
"""分层与归位门：crate 必须登记在层里，依赖只能向下（规范正本 docs/LAYERS.md）。

用法：
  python scripts/layers.py            # 全仓判定；违规判红，退出码 1
  python scripts/layers.py --self-test # 自带正例 + 四类反证

为什么要有这个（口径来自用户 2026-11-05）：算子/算法/学习要**集中在主库**，
其他项目只做生产链；现在来回搬代码、适配层重复、"算法完全一样"没人拦。
靠人记规范必然漂，所以把「谁在哪一层、允许依赖谁」变成一张表 + 一个能判红的门。

表在 `scripts/layers.json`：`layer` 是归位，`allows` 是**逐条点名**的跨层例外（必须带原因）。
新增 crate 不登记 = 判红；出现回边（下层依赖上层）且没登记例外 = 判红。
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "Cargo.toml"
TABLE = pathlib.Path(__file__).resolve().parent / "layers.json"

# 层序：数字小 = 更靠底座，只允许「大 → 小」的依赖方向。
LAYER_ORDER = {"base": 0, "ops": 1, "algo": 2, "learn": 3, "prod": 4}


def workspace_members() -> list[str]:
    """从根 Cargo.toml 的 members 里取 crate 名（只认 crates/<name>）。"""
    text = MANIFEST.read_text(encoding="utf-8")
    block = re.search(r"members\s*=\s*\[(.*?)\]", text, re.S)
    if not block:
        raise SystemExit("根 Cargo.toml 里没有 members 列表 —— 没表不等于没成员，判红")
    return [
        line.strip().strip('",')[len("crates/"):]
        for line in block.group(1).splitlines()
        if line.strip().strip('",').startswith("crates/")
    ]


def internal_deps(crate: str) -> set[str]:
    """某 crate 的 Cargo.toml 里指向仓内其他 crate 的依赖名（dependencies 与 dev-dependencies 都算）。"""
    path = ROOT / "crates" / crate / "Cargo.toml"
    if not path.is_file():
        raise SystemExit(f"找不到 {path} —— 成员登记了但目录不在，判红")
    names = set()
    for match in re.finditer(r"^([A-Za-z0-9_-]+)(?:\.[a-z]+)?\s*=\s", path.read_text(encoding="utf-8"), re.M):
        names.add(match.group(1))
    members = set(workspace_members())
    return {name for name in names & members if name != crate}


def check(table: dict, members: list[str], deps_of=None) -> list[str]:
    """返回违规清单（空 = 绿）。表与成员两边对不上也算违规。"""
    layers = table["crates"]
    if deps_of is None:
        deps_of = internal_deps
    problems: list[str] = []
    for crate in members:
        if crate not in layers:
            problems.append(f"未登记：{crate} 不在 layers.json（新增 crate 必须先归位）")
    for crate in layers:
        if crate not in members:
            problems.append(f"陈旧登记：layers.json 里的 {crate} 已不是 workspace 成员")
        elif layers[crate] not in LAYER_ORDER:
            problems.append(f"未知层：{crate} = {layers[crate]}")
    exceptions = {(item["from"], item["to"]): item["reason"] for item in table.get("allows", [])}
    for crate in members:
        if crate not in layers or layers[crate] not in LAYER_ORDER:
            continue
        for dep in sorted(deps_of(crate)):
            if dep not in layers or layers[dep] not in LAYER_ORDER:
                continue
            up = LAYER_ORDER[layers[dep]] > LAYER_ORDER[layers[crate]]
            if up and (crate, dep) not in exceptions:
                problems.append(
                    f"回边：{crate}({layers[crate]}) 依赖 {dep}({layers[dep]}) —— "
                    "下层不得依赖上层；要例外就写进 allows 并给原因"
                )
    # 例外表只能存在性检查（指向已消失的边说明该删，避免例外堆积）
    for (src, dst) in exceptions:
        if src in members and dst not in deps_of(src):
            problems.append(f"失效例外：allows 里 {src} → {dst} 这条依赖已经不在了")
    return problems


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    members = workspace_members()
    table = json.loads(TABLE.read_text(encoding="utf-8"))
    problems = check(table, members)
    counted = {
        layer: sum(1 for c in members if table["crates"].get(c) == layer)
        for layer in LAYER_ORDER
    }
    print(f"分层门：{len(members)} 个 crate ⇒ " + " ".join(f"{k}={v}" for k, v in counted.items()))
    for problem in problems:
        print(f"  拦：{problem}")
    if problems:
        print(f"FAIL 拦 {len(problems)} 条 ⇒ 有东西没归位或在倒着依赖")
        return 1
    print("OK 每个 crate 都归位，依赖全部向下（例外逐条点名）")
    return 0


def self_test() -> int:
    """正例绿 + 六类反证红：未登记 / 回边 / 例外放行 / 失效例外 / 陈旧登记 / 未知层名。"""
    failures = 0

    def expect(label: str, got: list[str], want_hit: str | None) -> None:
        nonlocal failures
        hit = (not got) if want_hit is None else any(want_hit in line for line in got)
        if hit:
            print(f"  ok   {label}")
        else:
            failures += 1
            print(f"  FAIL {label}: {got}")

    def deps(**edges: list[str]):
        table = {name: set(targets) for name, targets in edges.items()}
        return lambda crate: table.get(crate, set())

    base_case = {"crates": {"a": "base", "b": "ops", "c": "prod"}, "allows": []}
    expect("正例：a←b←c 全向下", check(base_case, ["a", "b", "c"], deps()), None)
    expect("反证 1：未登记的新 crate", check(base_case, ["a", "b", "c", "d"], deps()), "未登记")
    expect(
        "反证 2：下层依赖上层（x=ops → y=prod）",
        check({"crates": {"x": "ops", "y": "prod"}, "allows": []}, ["x", "y"], deps(x=["y"])),
        "回边",
    )
    expect(
        "反证 3：逐条例外登记后放行",
        check(
            {"crates": {"x": "ops", "y": "prod"},
             "allows": [{"from": "x", "to": "y", "reason": "基准要读产物"}]},
            ["x", "y"],
            deps(x=["y"]),
        ),
        None,
    )
    expect(
        "反证 4：失效例外（登记的边已不在）",
        check(
            {"crates": {"x": "base", "y": "prod"},
             "allows": [{"from": "x", "to": "y", "reason": "旧的"}]},
            ["x", "y"],
            deps(),
        ),
        "失效例外",
    )
    expect(
        "反证 5：陈旧登记（表里有、成员里没有）",
        check({"crates": {"x": "ops", "gone": "algo"}, "allows": []}, ["x"], deps()),
        "陈旧登记",
    )
    expect(
        "反证 6：未知层名",
        check({"crates": {"x": "middle"}, "allows": []}, ["x"], deps()),
        "未知层",
    )

    print("自测结论：%s" % ("绿" if failures == 0 else f"红 {failures} 条"))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
