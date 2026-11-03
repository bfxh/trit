#!/usr/bin/env python3
"""CI 作用域：改了哪些 crate，就只编译「这些 crate + 它们的反向依赖闭包」。

用法：
  python3 scripts/ci_scope.py                      # 本地看决策（无 base => 全量）
  python3 scripts/ci_scope.py --base <sha>          # 打印决策
  python3 scripts/ci_scope.py --base <sha> --json   # JSON
  python3 scripts/ci_scope.py --base <sha> --github-output   # 写 GITHUB_OUTPUT 键值
  python3 scripts/ci_scope.py --self-test

为什么收窄：Windows 全量 check 近期样本 10m05s（docs/08 的 C1），而多数 PR 只动一两个 crate。

什么时候**不收窄**（`full=true`，宁可全跑）：
  - 拿不到 base（push main / workflow_dispatch / 浅克隆）；
  - 改到不属于任何 crate 的文件（`Cargo.toml`、`Cargo.lock`、`rust-toolchain*`、`.cargo/**`、
    `.github/**`、`scripts/**` 都从这里出去：归不到 crate 就不窄化）；
  - 反向依赖闭包 >= 全仓 2/3：窄化没收益，只剩风险；
  - crate 名字不符合 `[A-Za-z0-9_.-]+`：这些名字会被拼进 shell，必须挡住。

`only_docs` 沿用 #247 的口径（`docs/**`、`*.md`、README/LICENSE/CHANGELOG），
让既有 `if:` 条件不用改。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]

# Windows runner 的 stdout 默认 cp1252，中文标签会 UnicodeEncodeError（#257 实测）；
# 门脚本的输出必须是 UTF-8，编不动的字符替换掉而不是抛异常。
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

NAME_OK = re.compile(r"^[A-Za-z0-9_.-]+$")
# 需要单独 feature 才编译得到的 crate：闭包里含它，才跑那条 step。
FEATURE_GATES = {"needs_png": ("png", "trit-texture"), "needs_gpu": ("gpu", "trit-gpu")}
# 闭包占全仓的比例阈值：超过就全量。
CLOSURE_RATIO = 2.0 / 3.0


def is_docs(path: str) -> bool:
    """文档面：与 #247 的正则同口径。"""
    p = path.replace("\\", "/")
    return (
        p.startswith("docs/")
        or p.endswith(".md")
        or p.startswith("README")
        or p.startswith("LICENSE")
        or p.startswith("CHANGELOG")
    )


def owner_of(path: str, pkg_dirs: dict[str, str]) -> str | None:
    """把仓库相对路径归到最深（最具体）的那个 crate 目录；归不到返回 None。

    `pkg_dirs` 是 `{crate 名: 该 crate 目录（仓库相对、posix、无尾斜杠）}`。
    最深匹配是必须的：`crates/trit-core` 与 `crates/trit-core/derive` 可能都是成员。
    """
    p = path.replace("\\", "/")
    best: str | None = None
    best_len = -1
    for name, directory in pkg_dirs.items():
        prefix = directory + "/"
        if p.startswith(prefix) and len(directory) > best_len:
            best, best_len = name, len(directory)
    return best


def reverse_closure(seeds: set[str], dependents: dict[str, set[str]]) -> set[str]:
    """沿「谁依赖我」做传递闭包；seeds 本身一定在内。"""
    seen = set(seeds)
    stack = list(seeds)
    while stack:
        cur = stack.pop()
        for up in dependents.get(cur, ()):
            if up not in seen:
                seen.add(up)
                stack.append(up)
    return seen


def blank_decision(reason: str) -> dict:
    """`full=true` 的安全默认：feature step 一律照跑，args 空 = 全量。"""
    out = {
        "full": True,
        "only_docs": False,
        "crates": "",
        "args": "",
        "reason": reason,
        "changed": "",
    }
    for key in FEATURE_GATES:
        out[key] = True
    return out


def decide(files: list[str], pkg_dirs: dict[str, str], dependents: dict[str, set[str]]) -> dict:
    """纯决策：输入变更文件 + 依赖图，输出要不要窄化、窄化成什么。"""
    names = set(pkg_dirs)
    out = blank_decision("")

    if not files:
        out["reason"] = "没有可比对的变更（非 PR 或拿不到 base）=> 全量"
        return out

    docs = [f for f in files if is_docs(f)]
    code = [f for f in files if not is_docs(f)]
    if not code:
        out["full"] = False
        out["only_docs"] = True
        out["reason"] = f"只改文档（{len(docs)} 个文件）=> 跳过 Rust 矩阵"
        return out

    seeds: set[str] = set()
    for f in code:
        owner = owner_of(f, pkg_dirs)
        if owner is None:
            # 根级/workspace 级文件（Cargo.toml、Cargo.lock、.github、scripts、toolchain）
            # 都从这条路出去：归不到 crate 就不窄化。
            out["reason"] = f"{f} 不属于任何 crate => 全量"
            return out
        seeds.add(owner)

    closure = reverse_closure(seeds, dependents) & names
    if not all(NAME_OK.match(n) for n in closure):
        out["reason"] = "crate 名字含非法字符（会被拼进 shell）=> 全量"
        return out

    if len(names) and len(closure) / len(names) >= CLOSURE_RATIO:
        out["reason"] = (
            f"反向依赖闭包 {len(closure)}/{len(names)} 个 crate（>={int(CLOSURE_RATIO * 100)}%）=> 窄化没收益，全量"
        )
        return out

    ordered = sorted(closure)
    out.update(
        full=False,
        crates=" ".join(ordered),
        args=" ".join(f"-p {n}" for n in ordered),
        changed=" ".join(sorted(seeds)),
        reason=f"直改 {len(seeds)} 个 crate，闭包 {len(ordered)} 个 => 窄化",
    )
    for key, (_feature, gate_crate) in FEATURE_GATES.items():
        out[key] = gate_crate in closure
    return out


# —— IO：真仓库的依赖图与 diff ——


def load_graph(root: pathlib.Path) -> tuple[dict[str, str], dict[str, set[str]]]:
    """`cargo metadata --no-deps` → (crate 名 → 目录, 被依赖表)。"""
    raw = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=True,
    ).stdout
    meta = json.loads(raw)
    root_dir = pathlib.PurePosixPath(meta["workspace_root"].replace("\\", "/"))
    pkg_dirs: dict[str, str] = {}
    for pkg in meta["packages"]:
        manifest = pathlib.PurePosixPath(pkg["manifest_path"].replace("\\", "/"))
        try:
            rel = manifest.parent.relative_to(root_dir)
        except ValueError:  # crate 在 workspace 之外：不该进 -p 列表
            rel = manifest.parent
        pkg_dirs[pkg["name"]] = str(rel).replace("\\", "/").rstrip(".")

    dependents: dict[str, set[str]] = {}
    for pkg in meta["packages"]:
        for dep in pkg.get("dependencies", []):
            if dep["name"] in pkg_dirs:
                dependents.setdefault(dep["name"], set()).add(pkg["name"])
    return pkg_dirs, dependents


def changed_files(root: pathlib.Path, base: str, head: str = "HEAD") -> list[str]:
    res = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...{head}"],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if res.returncode != 0:
        return []
    return [line for line in res.stdout.splitlines() if line.strip()]


def emit(decision: dict, mode: str, stream) -> None:
    if mode == "json":
        json.dump(decision, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
        return
    if mode == "plain":
        if decision["full"]:
            scope = "全量"
        elif decision["only_docs"]:
            scope = "跳过 Rust"
        else:
            scope = "窄化"
        stream.write(f"{scope}：{decision['reason']}\n")
        if decision["args"]:
            stream.write(f"  args: {decision['args']}\n")
        return
    for key in (
        "only_docs",
        "full",
        "needs_png",
        "needs_gpu",
        "crates",
        "changed",
        "args",
        "reason",
    ):
        value = decision[key]
        if isinstance(value, bool):
            value = "true" if value else "false"
        stream.write(f"{key}={value}\n")


def self_test() -> int:
    """夹具驱动的决策测试；负例必须真的把错误判断挑出来。"""
    pkg_dirs = {
        "trit-core": "crates/trit-core",
        "trit-ops": "crates/trit-ops",
        "trit-gpu": "crates/trit-gpu",
        "trit-texture": "crates/trit-texture",
        "trit-cli": "crates/trit-cli",
        "trit-wasm": "crates/trit-wasm",
        "trit-lsp": "crates/trit-lsp",
        "trit-re": "crates/trit-re",
    }
    # trit-ops / trit-gpu / trit-texture 依赖 trit-core；trit-cli 依赖 trit-ops（传递进来）。
    dependents = {
        "trit-core": {"trit-ops", "trit-gpu", "trit-texture"},
        "trit-ops": {"trit-cli"},
    }
    checks: list[tuple[str, bool]] = []

    def check(label: str, cond: bool) -> None:
        checks.append((label, cond))

    d = decide([], pkg_dirs, dependents)
    check("空 diff => 全量", d["full"] and not d["only_docs"])

    d = decide(["docs/08.md", "README.md"], pkg_dirs, dependents)
    check("纯文档 => 跳过", (not d["full"]) and d["only_docs"])

    d = decide(["crates/trit-core/src/lib.rs"], pkg_dirs, dependents)
    check(
        "改 trit-core => 闭包含直接+传递依赖者",
        set(d["crates"].split())
        == {"trit-core", "trit-ops", "trit-gpu", "trit-texture", "trit-cli"}
        and "-p trit-gpu" in d["args"] and d["needs_gpu"],
    )
    check("闭包不含无关 crate", "trit-wasm" not in d["crates"] and "trit-lsp" not in d["crates"])

    d = decide(["crates/trit-texture/src/ttx.rs"], pkg_dirs, dependents)
    check("改叶子 crate => 只有自己", d["crates"] == "trit-texture" and d["needs_png"])
    check("改叶子 => 不跑 gpu step", not d["needs_gpu"])

    d = decide(["Cargo.lock"], pkg_dirs, dependents)
    check("Cargo.lock => 全量", d["full"])
    d = decide([".github/workflows/ci.yml"], pkg_dirs, dependents)
    check(".github => 全量", d["full"])
    d = decide(["scripts/mutation_check.py"], pkg_dirs, dependents)
    check("scripts => 全量", d["full"])

    d = decide(["crates/trit-core/src/lib.rs", "docs/08.md"], pkg_dirs, dependents)
    check("文档 + 代码 => 仍按代码窄化", (not d["full"]) and (not d["only_docs"]) and d["crates"])

    d = decide(["xtask/src/main.rs"], pkg_dirs, dependents)
    check("归不到 crate 的路径 => 全量", d["full"])

    # 传递闭包吃掉大半仓时，窄化没有收益。
    wide = decide(
        ["crates/trit-core/src/lib.rs", "crates/trit-wasm/src/lib.rs"], pkg_dirs, dependents
    )
    check("闭包覆盖大半仓 => 全量", wide["full"])

    # 注入防线：目录名带空格时不许把它拼进 shell（4 个成员，闭包只占一半，
    # 免得“大半仓”那条规则替它兜底， mutant 就白给了）。
    bad_dirs = {
        "ok": "crates/ok",
        "bad name": "crates/bad name",
        "x": "crates/x",
        "y": "crates/y",
    }
    bad = decide(["crates/ok/src/lib.rs"], bad_dirs, {"ok": {"bad name"}})
    check("非法 crate 名 => 全量", bad["full"])
    check("合法名窄化不被误拦", decide(["crates/x/src/lib.rs"], bad_dirs, {}).get("args") == "-p x")

    # owner_of 必须挑最深的目录（嵌套成员）。
    nested = {"outer": "crates/outer", "inner": "crates/outer/inner"}
    check("嵌套成员归最深", owner_of("crates/outer/inner/src/lib.rs", nested) == "inner")
    check("外层文件归外层", owner_of("crates/outer/src/lib.rs", nested) == "outer")

    failed = [label for label, ok in checks if not ok]
    for label, ok in checks:
        print(f"  {'FAILED' if not ok else 'ok    '} {label}")
    print(f"self-test: {len(checks) - len(failed)}/{len(checks)} passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", help="比对基线（PR 的 base sha）")
    ap.add_argument("--head", default="HEAD")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--github-output", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--root", default=str(ROOT))
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()

    mode = "json" if args.json else "github" if args.github_output else "plain"
    root = pathlib.Path(args.root)
    try:
        pkg_dirs, dependents = load_graph(root)
    except (subprocess.SubprocessError, OSError, ValueError, KeyError) as exc:
        # 读不到依赖图不是「窄化失败」而是「保守全量」：full 永远是安全方向，
        # 且这里必须 exit 0 —— 让 workflow 的 fallback 分支而不是整条 job 红。
        decision = blank_decision(f"依赖图读取失败（{type(exc).__name__}）=> 全量")
        decision["closure_total"] = 0
        emit(decision, mode, sys.stdout)
        return 0
    files = changed_files(root, args.base) if args.base else []
    decision = decide(files, pkg_dirs, dependents)
    decision["closure_total"] = len(pkg_dirs)
    emit(decision, mode, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
