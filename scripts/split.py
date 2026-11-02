#!/usr/bin/env python3
"""库拆分：识别候选边界 → 出迁移计划 → 生成独立库骨架 → 列出平台侧步骤。

用法：
  python scripts/split.py candidates [--top 10] [--since 180.days] [--jaccard 0.3]
  python scripts/split.py plan <crate>... 
  python scripts/split.py scaffold <crate>... --to <目录>
  python scripts/split.py --self-test

为什么不是"看目录像什么就拆什么"：
  - 边界用两条证据交叉：**依赖图**（谁依赖谁）与**共变聚类**（同一批提交里一起改的 crate，
    Jaccard ≥ 阈值的聚成一簇）——后者是模块聚类文献里评估"是不是一个模块"的常用信号；
  - 拆之前先算**闭包**（要一起搬走谁）、**阻断项**（继承字段/路径依赖/越界引用）、**下游**（谁会被影响）；
  - 最后把平台侧动作列成可执行清单（建仓、保护规则、topic、下游 pin 更新、弃用公告）。
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def load_deps():
    spec = importlib.util.spec_from_file_location("split_deps", pathlib.Path(__file__).resolve().parent / "deps.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules["split_deps"] = module
    spec.loader.exec_module(module)
    return module


deps = load_deps()


def git(args: list[str], root: pathlib.Path) -> str:
    result = subprocess.run(
        ["git", *args], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, encoding="utf-8", errors="replace",
    )
    return result.stdout if result.returncode == 0 else ""


def commit_crate_sets(root: pathlib.Path, since: str) -> list[set[str]]:
    """把 git 历史切成"每次提交碰到哪些 crate"。"""
    text = git(["log", f"--since={since}", "--name-only", "--pretty=format:__C__"], root)
    commits: list[set[str]] = []
    current: set[str] = set()
    for line in text.splitlines():
        if line.startswith("__C__"):
            if current:
                commits.append(current)
            current = set()
            continue
        parts = line.split("/")
        if len(parts) > 2 and parts[0] == "crates":
            current.add(parts[1])
    if current:
        commits.append(current)
    return [c for c in commits if 2 <= len(c) <= 12]  # 超大提交（批量格式化）不算耦合证据


def co_change_pairs(commits: list[set[str]]) -> tuple[dict[tuple[str, str], int], dict[str, int]]:
    pairs: dict[tuple[str, str], int] = {}
    support: dict[str, int] = {}
    for crates in commits:
        ordered = sorted(crates)
        for name in ordered:
            support[name] = support.get(name, 0) + 1
        for i, left in enumerate(ordered):
            for right in ordered[i + 1 :]:
                pairs[(left, right)] = pairs.get((left, right), 0) + 1
    return pairs, support


def cluster_crates(
    pairs: dict[tuple[str, str], int], support: dict[str, int], jaccard: float, min_co: int
) -> list[dict]:
    """Jaccard ≥ 阈值且共现 ≥ min_co 的连边聚成簇（并查集）。"""
    parent: dict[str, str] = {}

    def find(x: str) -> str:
        parent.setdefault(x, x)
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a: str, b: str) -> None:
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[rb] = ra

    edges: list[dict] = []
    for (a, b), count in pairs.items():
        union_size = support.get(a, 0) + support.get(b, 0) - count
        if union_size <= 0 or count < min_co:
            continue
        score = count / union_size
        if score < jaccard:
            continue
        union(a, b)
        edges.append({"a": a, "b": b, "count": count, "jaccard": round(score, 3)})

    groups: dict[str, list[str]] = {}
    for name in parent:
        groups.setdefault(find(name), []).append(name)
    result = []
    for members in groups.values():
        if len(members) < 2:
            continue
        inner = [e for e in edges if e["a"] in members and e["b"] in members]
        strength = sum(e["jaccard"] for e in inner) / max(1, len(inner))
        result.append(
            {
                "crates": sorted(members),
                "mean_jaccard": round(strength, 3),
                "co_changes": sum(e["count"] for e in inner),
                "support": {m: support.get(m, 0) for m in sorted(members)},
            }
        )
    return sorted(result, key=lambda g: (-len(g["crates"]), -g["mean_jaccard"]))


def crate_metrics(plans: list[dict]) -> dict[str, dict]:
    fan_in: dict[str, int] = {p["crate"]: 0 for p in plans}
    for plan in plans:
        for dep in plan["internal_deps"]:
            fan_in[dep] = fan_in.get(dep, 0) + 1
    return {
        p["crate"]: {
            "fan_in": fan_in.get(p["crate"], 0),
            "fan_out": len(p["internal_deps"]),
            "publish": p["publish"],
            "blockers": len(p["blockers"]),
        }
        for p in plans
    }


def downstream_of(plans: list[dict], target: str) -> list[str]:
    return sorted(p["crate"] for p in plans if target in p["internal_deps"])


def closure_of(plans: dict[str, dict], crates: list[str]) -> list[str]:
    seen: set[str] = set()
    stack = list(crates)
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        plan = plans.get(current)
        if not plan:
            continue
        stack.extend(plan["internal_deps"])
        for blocker in plan["blockers"]:
            if blocker["kind"] == "内部 dev":
                stack.append(blocker["detail"].split("（", 1)[0])
    return sorted(seen)


def platform_steps(owner: str, name: str) -> list[str]:
    full = f"{owner}/{name}"
    return [
        f"gh repo create {full} --public --description '<一句话定位>'",
        f"gh repo edit {full} --add-topic rust --add-topic ternary",
        f"在 {full} 里加 .github/workflows/ci.yml（scaffold 已生成）",
        f"gh api --method PUT repos/{full}/branches/main/protection "
        "--input <(生成 required_status_checks 的 JSON)  # 或先用 ci_integrity.py 的清单",
        "把下游仓的依赖从 path 改成 version 或 git pin（改完跑 cargo metadata --offline）",
        f"在旧仓的 README/CHANGELOG 写明：该模块已迁到 {full}",
    ]


def scaffold(root: pathlib.Path, crates: list[str], to: pathlib.Path, repo: str) -> int:
    plans, by_name = deps.build(root)
    plan_map = {p["crate"]: p for p in plans}
    missing = [c for c in crates if c not in plan_map]
    if missing:
        print(f"没有这些 crate：{', '.join(missing)}")
        return 2
    closure = closure_of(plan_map, crates)
    first = crates[0]
    code = deps.detach(root, first, to, force=False)
    if code != 0:
        return code
    # detach 只搬了 first 的闭包；把同组其它 crate 也补进去
    import shutil

    for name in closure:
        if name == first or name not in plan_map:
            continue
        src = root / plan_map[name]["path"]
        dst = to / "crates" / name
        if dst.exists():
            continue
        shutil.copytree(src, dst, ignore=shutil.ignore_patterns("target", ".trit", "vendor"))

    name = to.name
    (to / "README.md").write_text(
        f"# {name}\n\n"
        f"从 `{repo}` 拆出的独立库，包含：{', '.join('`' + c + '`' for c in closure)}。\n\n"
        "```bash\ncargo test\ncargo clippy --all-targets -- -D warnings\n```\n\n"
        "## 边界\n\n"
        "这里只放这一组 crate；跨组的依赖请发布到 crates.io 或固定 git pin，不要回引旧仓路径。\n",
        encoding="utf-8",
        newline="\n",
    )
    (to / ".gitattributes").write_text("* text=auto eol=lf\n", encoding="utf-8", newline="\n")
    (to / ".gitignore").write_text("/target\n__pycache__/\n.trit/\n", encoding="utf-8", newline="\n")
    (to / "CODEOWNERS").write_text(f"* @{repo.split('/')[0]}\n", encoding="utf-8", newline="\n")
    (to / ".github" / "workflows").mkdir(parents=True, exist_ok=True)
    (to / ".github" / "workflows" / "ci.yml").write_text(
        "name: CI\n\non:\n  push:\n  pull_request:\n\n"
        "permissions:\n  contents: read\n\n"
        "jobs:\n  check:\n    runs-on: ubuntu-latest\n    timeout-minutes: 30\n    steps:\n"
        "      - uses: actions/checkout@v5\n"
        "      - uses: dtolnay/rust-toolchain@stable\n"
        "      - run: cargo fmt --all -- --check\n"
        "      - run: cargo clippy --workspace --all-targets -- -D warnings\n"
        "      - run: cargo test --workspace\n",
        encoding="utf-8",
        newline="\n",
    )
    owner = repo.split("/")[0]
    checklist = ["# 拆分清单", "", "## 平台侧", ""] + [f"- [ ] {step}" for step in platform_steps(owner, name)]
    checklist += ["", "## 下游影响", ""]
    for crate in crates:
        for consumer in downstream_of(plans, crate):
            checklist.append(f"- [ ] `{consumer}` 依赖 `{crate}`：把 path 依赖改成 version / git pin")
    (to / "SPLIT-CHECKLIST.md").write_text("\n".join(checklist) + "\n", encoding="utf-8", newline="\n")
    print(f"骨架已生成：{to}（{len(closure)} 个 crate；清单见 SPLIT-CHECKLIST.md）")
    return 0


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    commits = [{"a", "b"}, {"a", "b"}, {"a", "b"}, {"b", "c"}, {"a", "d"}]
    pairs, support = co_change_pairs(commits)
    check("共变计数", pairs[("a", "b")], 3)
    check("support 计数", support["a"], 4)
    clusters = cluster_crates(pairs, support, jaccard=0.5, min_co=2)
    check("聚出一簇", [g["crates"] for g in clusters], [["a", "b"]])
    # 共现次数高、但各自提交基数也大 ⇒ Jaccard 很低，不该聚
    check("高共现低相似度不聚", cluster_crates({("x", "y"): 3}, {"x": 40, "y": 40}, jaccard=0.5, min_co=2), [])
    loose = cluster_crates(pairs, support, jaccard=0.1, min_co=1)
    check("阈值放宽后会并进来", any("c" in g["crates"] for g in loose), True)

    plans = [
        {"crate": "core", "internal_deps": [], "blockers": [], "publish": True, "path": "crates/core"},
        {"crate": "mid", "internal_deps": ["core"], "blockers": [], "publish": True, "path": "crates/mid"},
        {"crate": "top", "internal_deps": ["mid"], "blockers": [], "publish": True, "path": "crates/top"},
    ]
    metrics = crate_metrics(plans)
    check("fan_in 计算", metrics["core"]["fan_in"], 1)
    check("fan_out 计算", metrics["mid"]["fan_out"], 1)
    check("下游识别", downstream_of(plans, "core"), ["mid"])
    plan_map = {p["crate"]: p for p in plans}
    check("闭包含传递依赖", closure_of(plan_map, ["top"]), ["core", "mid", "top"])
    steps = platform_steps("bfxh", "demo")
    check("平台清单有建仓", any("gh repo create" in s for s in steps), True)
    check("平台清单有保护规则", any("protection" in s for s in steps), True)

    if failures:
        print(f"self-test: {total[0] - len(failures)}/{total[0]} passed")
        return 1
    print(f"self-test: {total[0]}/{total[0]} passed")
    return 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root = ROOT
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1]).resolve()
    plans, _ = deps.build(root)
    plan_map = {p["crate"]: p for p in plans}
    command = argv[0] if argv else "candidates"
    value_flags = {"--root", "--to", "--repo", "--since", "--jaccard", "--top"}
    rest: list[str] = []
    index = 1
    while index < len(argv):
        arg = argv[index]
        if arg in value_flags:
            index += 2
            continue
        if not arg.startswith("--"):
            rest.append(arg)
        index += 1

    if command == "candidates":
        since = argv[argv.index("--since") + 1] if "--since" in argv else "180.days"
        jaccard = float(argv[argv.index("--jaccard") + 1]) if "--jaccard" in argv else 0.3
        top = int(argv[argv.index("--top") + 1]) if "--top" in argv else 10
        commits = commit_crate_sets(root, since)
        pairs, support = co_change_pairs(commits)
        clusters = cluster_crates(pairs, support, jaccard, min_co=3)
        metrics = crate_metrics(plans)
        print(f"共变聚类（{len(commits)} 次多 crate 提交，Jaccard ≥ {jaccard}，共现 ≥ 3）")
        if not clusters:
            print("  没有达到阈值的簇——说明各 crate 的提交是分散的（拆分的收益可能不大）")
        for group in clusters[:top]:
            members = group["crates"]
            fanned = sum(metrics[m]["fan_in"] for m in members if m in metrics)
            print(
                f"  [{len(members)} 个] {', '.join(members)}"
                f"  mean_jaccard={group['mean_jaccard']}  co_changes={group['co_changes']}"
                f"  外部被依赖={fanned}"
            )
        print("\n单体 crate 指标（fan_in 高=公共底座；fan_out 高=编排层；blockers 多=拆起来麻烦）")
        for name, metric in sorted(metrics.items(), key=lambda kv: (-kv[1]["fan_in"], kv[0]))[:top]:
            print(f"  {name:<28} fan_in={metric['fan_in']:<3} fan_out={metric['fan_out']:<3} "
                  f"blockers={metric['blockers']:<2} publish={metric['publish']}")
        return 0

    if command == "plan":
        if not rest:
            print("用法：split.py plan <crate>...")
            return 2
        unknown = [c for c in rest if c not in plan_map]
        if unknown:
            print(f"没有这些 crate：{', '.join(unknown)}")
            return 2
        closure = closure_of(plan_map, rest)
        print(f"要搬走的闭包（{len(closure)}）：{', '.join(closure)}")
        blockers = 0
        for name in rest:
            for blocker in plan_map[name]["blockers"]:
                print(f"  阻断 {name}: {blocker['kind']} — {blocker['detail']}")
                blockers += 1
        consumers = sorted({c for name in rest for c in downstream_of(plans, name)} - set(closure))
        print(f"下游会被影响（{len(consumers)}）：{', '.join(consumers) if consumers else '（无）'}")
        print(f"平台侧：")
        for step in platform_steps("bfxh", "TODO-name"):
            print(f"  - {step}")
        print(f"（本组阻断项 {blockers} 条；先修阻断再 scaffold）")
        return 0

    if command == "scaffold":
        if not rest or "--to" not in argv:
            print("用法：split.py scaffold <crate>... --to <目录>")
            return 2
        to = pathlib.Path(argv[argv.index("--to") + 1]).resolve()
        repo = argv[argv.index("--repo") + 1] if "--repo" in argv else "bfxh/trit"
        return scaffold(root, rest, to, repo)

    print(f"未知命令：{command}（candidates / plan / scaffold）")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
