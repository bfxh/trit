#!/usr/bin/env python3
"""改动牵连面：改了哪些文件 → 哪些 crate 会被牵连 + 谁已经认领。

用法：
  python scripts/impact.py [--base <rev>] [--files <路径>...]
      [--claims <CLAIMS.md>] [--repo <o/r>] [--json] [--strict]
  python scripts/impact.py --self-test

口径：
  - 文件 → 归属 crate（最深前缀），再取**反向依赖闭包**（谁依赖它）；
  - 归不到 crate 的非文档文件（Cargo.toml / rust-toolchain / .github / scripts）⇒ full=true；
  - 闭包 ≥ 全仓 2/3 ⇒ full=true（与 ci_scope 同口径：窄化没收益）；
  - 文档面单独归类（docs_hygiene / doc_pointers）；
  - 给 `--claims` 时列出"会被牵连、但没有任何活动声明覆盖"的 crate —— 防"我只认领了 core，
    没看见下游 12 个 crate 的测试也会跟着变"。
  - `--strict`：存在未认领的牵连 crate 就 rc=1（默认只报告，rc=0）。

exit：0 报告完成（--strict 下有未认领牵连则为 1）；2 用法/取证失败。
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
CLOSURE_RATIO = 2.0 / 3.0
HIGH_RATIO = 0.5


def say(text: str) -> None:
    stream = sys.stdout
    encoding = getattr(stream, "encoding", None) or "utf-8"
    try:
        print(text)
    except UnicodeEncodeError:
        print(text.encode(encoding, "replace").decode(encoding, "replace"))


def git(root: pathlib.Path, *args: str) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            ["git", "-C", str(root), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
    except OSError as exc:
        return 127, str(exc)
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def is_docs(path: str) -> bool:
    p = path.replace("\\", "/")
    return (
        p.startswith("docs/")
        or p.endswith(".md")
        or p.startswith("README")
        or p.startswith("LICENSE")
        or p.startswith("CHANGELOG")
    )


def is_script(path: str) -> bool:
    p = path.replace("\\", "/")
    return p.startswith("scripts/") or "/scripts/" in p


def owner_of(path: str, pkg_dirs: dict[str, str]) -> str | None:
    """最深前缀归属：`crates/trit-core/src/x.rs` → `trit-core`。"""
    best, best_len = None, -1
    for name, directory in pkg_dirs.items():
        prefix = directory.replace("\\", "/").rstrip("/") + "/"
        if path.startswith(prefix) and len(prefix) > best_len:
            best, best_len = name, len(prefix)
    return best


def reverse_closure(seeds: set[str], *maps: dict[str, set[str]]) -> set[str]:
    """依赖种子的人（反向依赖的传递闭包），含种子自身。

    多张图分开传：正常/构建依赖是“运行时会牵连”，dev 依赖只牵连测试面。
    """
    seen = set(seeds)
    queue = list(seeds)
    while queue:
        current = queue.pop()
        for mapping in maps:
            for dependent in mapping.get(current, ()):
                if dependent not in seen:
                    seen.add(dependent)
                    queue.append(dependent)
    return seen


def normalize(path: str) -> str:
    p = path.replace("\\", "/")
    while p.startswith("./"):
        p = p[2:]
    return p.lstrip("/")


def compute(
    files: list[str],
    pkg_dirs: dict[str, str],
    dependents: dict[str, set[str]],
    claims: list[dict] | None = None,
    repo: str = "",
    dev_dependents: dict[str, set[str]] | None = None,
) -> dict:
    normalized = [normalize(path) for path in files if path.strip()]
    seeds: set[str] = set()
    others: list[str] = []
    docs: list[str] = []
    scripts: list[str] = []
    for path in normalized:
        owner = owner_of(path, pkg_dirs)
        if owner:
            seeds.add(owner)
        elif is_docs(path):
            docs.append(path)
        else:
            others.append(path)
        if is_script(path) and not is_docs(path):
            scripts.append(path)

    reasons: list[str] = []
    full = bool(others)
    if others:
        reasons.append("归不到 crate 的文件：" + ", ".join(sorted(others)[:5]))

    dev_dependents = dev_dependents or {}
    runtime = reverse_closure(seeds, dependents) if seeds else set()
    closure = reverse_closure(seeds, dependents, dev_dependents) if seeds else set()
    dev_only = sorted(closure - runtime)
    total = len(pkg_dirs)
    ratio = (len(runtime) / total) if total else 0.0

    if not seeds and not others:
        kind = "docs" if docs else ("scripts" if scripts else "none")
    elif seeds and (docs or scripts):
        kind = "mixed"
    elif seeds:
        kind = "crates"
    else:
        kind = "scripts" if scripts and not others else "other"

    if not full and total and ratio >= CLOSURE_RATIO:
        full = True
        reasons.append(f"反向依赖闭包 {len(closure)}/{total}（>={int(CLOSURE_RATIO * 100)}%）")

    risk = "low"
    if full:
        risk = "high"
    elif ratio >= HIGH_RATIO:
        risk = "high"
    elif len(runtime) > len(seeds) or dev_only:
        risk = "medium"

    gates: list[str] = []
    if full:
        gates.append("cargo test --workspace")
    elif seeds:
        gates.append("cargo test -p " + " ".join(sorted(closure)))
    if docs:
        gates.extend(["python scripts/docs_hygiene.py", "python scripts/doc_pointers.py"])
    for path in sorted(set(scripts)):
        if path.endswith(".py"):
            gates.append(f"python {path} --self-test")
    gates = list(dict.fromkeys(gates))

    claimed: set[str] = set()
    for claim in claims or []:
        claim_repo = (claim.get("repo") or "").rstrip("/")
        if repo and claim_repo.split("/")[-1] != repo.rstrip("/").split("/")[-1]:
            continue
        paths = claim.get("paths") or []
        if not paths:
            claimed |= closure  # 整仓声明覆盖所有牵连
            continue
        for path in paths:
            owner = owner_of(normalize(str(path)), pkg_dirs)
            if owner:
                claimed.add(owner)
    unclaimed = sorted(closure - claimed) if claims is not None else []

    return {
        "files": normalized,
        "seeds": sorted(seeds),
        "closure": sorted(closure),
        "runtime": sorted(runtime),
        "dev_only": dev_only,
        "ratio": round(ratio, 3),
        "kind": kind,
        "full": full,
        "risk": risk,
        "reasons": reasons,
        "gates": gates,
        "claimed": sorted(claimed),
        "unclaimed": unclaimed,
    }


def load_graph(root: pathlib.Path) -> tuple[dict[str, str], dict[str, set[str]]]:
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stdout.decode("utf-8", "replace")[-400:])
    meta = json.loads(proc.stdout.decode("utf-8", "replace"))
    pkg_dirs: dict[str, str] = {}
    dependents: dict[str, set[str]] = {}
    dev_dependents: dict[str, set[str]] = {}
    for package in meta.get("packages", []):
        name = package["name"]
        directory = pathlib.Path(package["manifest_path"]).parent
        try:
            rel = directory.relative_to(root).as_posix()
        except ValueError:
            rel = directory.as_posix()
        pkg_dirs[name] = rel
    for package in meta.get("packages", []):
        for dep in package.get("dependencies", []):
            kind = dep.get("kind") or "normal"
            target = dev_dependents if kind == "dev" else dependents
            target.setdefault(dep["name"], set()).add(package["name"])
    return pkg_dirs, dependents, dev_dependents


def changed_files(root: pathlib.Path, base: str) -> list[str]:
    rc, out = git(root, "diff", "--name-only", f"{base}...HEAD")
    if rc != 0:
        raise RuntimeError(out.strip()[:300])
    return [line.strip() for line in out.splitlines() if line.strip()]


def parse_claims(path: pathlib.Path, repo: str) -> list[dict]:
    import time

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from agent_claims import live, parse_claims as parse  # noqa: PLC0415

    if not path.is_file():
        raise RuntimeError(f"声明文件不存在：{path}")
    claims, _broken = parse(path.read_text(encoding="utf-8"))
    return [claim for claim in claims if live(claim, int(time.time()))]


def report(result: dict) -> None:
    say(f"牵连面：{len(result['files'])} 个文件 → {result['kind']}｜风险 {result['risk']}")
    if result["reasons"]:
        for reason in result["reasons"]:
            say(f"  - {reason}")
    if result["seeds"]:
        say(f"  种子 crate：{', '.join(result['seeds'])}")
    if result["closure"]:
        shown = result["closure"][:12]
        more = "" if len(result["closure"]) <= 12 else f" … 共 {len(result['closure'])} 个"
        say(f"  反向依赖闭包：{', '.join(shown)}{more}")
    if result["gates"]:
        say("  建议跑：")
        for gate in result["gates"]:
            say(f"    {gate}")
    if result.get("dev_only"):
        say(f"  仅测试面牵连（dev 依赖）：{', '.join(result['dev_only'])}")
    if result["unclaimed"]:
        say(f"  未被认领的牵连 crate（{len(result['unclaimed'])}）："
            f"{', '.join(result['unclaimed'])}")
    elif result["claimed"]:
        say("  牵连 crate 全部有活动声明覆盖")


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}: 期望 {expected!r}，实际 {actual!r}")

    pkg_dirs = {
        "trit-core": "crates/trit-core",
        "trit-a": "crates/trit-a",
        "app": "crates/app",
    }
    dependents = {"trit-core": {"trit-a", "app"}, "trit-a": {"app"}}

    core = compute(["crates/trit-core/src/lib.rs"], pkg_dirs, dependents)
    check("core 闭包含下游", core["closure"], ["app", "trit-a", "trit-core"])
    check("core 判高风险", core["risk"], "high")
    check("core 给全量测试门", "cargo test --workspace" in core["gates"], True)

    leaf = compute(["crates/app/src/lib.rs"], pkg_dirs, dependents)
    check("叶子 crate 低风险", leaf["risk"], "low")
    check("叶子闭包只有自己", leaf["closure"], ["app"])

    manifest = compute(["Cargo.toml"], pkg_dirs, dependents)
    check("清单文件判全量", manifest["full"], True)
    check("全量给 workspace 门", "cargo test --workspace" in manifest["gates"], True)

    docs = compute(["docs/X.md", "README.md"], pkg_dirs, dependents)
    check("纯文档不判全量", docs["full"], False)
    check("纯文档归类", docs["kind"], "docs")
    check("文档给卫生门", "python scripts/docs_hygiene.py" in docs["gates"], True)

    scripts = compute(["scripts/hooks.py"], pkg_dirs, dependents)
    check("脚本给自测门", "python scripts/hooks.py --self-test" in scripts["gates"], True)

    mixed = compute(
        ["crates/trit-core/src/lib.rs", "docs/X.md"], pkg_dirs, dependents
    )
    check("混合改动归类", mixed["kind"], "mixed")

    wide_dirs = {"x": "crates/x", "y": "crates/y", "z": "crates/z", "w": "crates/w"}
    wide = compute(["crates/x/src/lib.rs"], wide_dirs, {"x": {"y"}})
    check("闭包 2/4 判高风险（>=50%）", wide["risk"], "high")
    check("闭包未到 2/3 不判全量", wide["full"], False)

    claims = [
        {"repo": "bfxh/trit", "paths": ["crates/trit-a/src/lib.rs"]},
        {"repo": "bfxh/other", "paths": ["crates/trit-core/src/lib.rs"]},
    ]
    covered = compute(
        ["crates/trit-core/src/lib.rs"], pkg_dirs, dependents, claims, repo="bfxh/trit"
    )
    check("只认领下游仍表白 core/app", covered["unclaimed"], ["app", "trit-core"])

    all_claimed = compute(
        ["crates/trit-core/src/lib.rs"], pkg_dirs, dependents,
        [{"repo": "bfxh/trit", "paths": []}], repo="bfxh/trit",
    )
    check("整仓声明覆盖所有牵连", all_claimed["unclaimed"], [])

    unknown = compute(["crates/trit-core/src/lib.rs"], pkg_dirs, dependents)
    check("默认不计算未认领", unknown["unclaimed"], [])

    dev_dirs = dict(pkg_dirs)
    dev_dirs["trit-dev"] = "crates/trit-dev"
    dev = compute(
        ["crates/trit-core/src/lib.rs"], dev_dirs, dependents,
        dev_dependents={"trit-core": {"trit-dev"}},
    )
    check("dev 依赖不进运行时闭包", dev["runtime"], ["app", "trit-a", "trit-core"])
    check("测试闭包含 dev 下游", dev["closure"], ["app", "trit-a", "trit-core", "trit-dev"])
    check("dev_only 点名", dev["dev_only"], ["trit-dev"])

    none = compute([], pkg_dirs, dependents)
    check("空改动不慌", none["kind"], "none")

    if failures:
        print(f"self-test: {total[0] - len(failures)}/{total[0]} passed")
        return 1
    print(f"self-test: {total[0]}/{total[0]} passed")
    return 0


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root = ROOT
    if "--repo-root" in argv:
        root = pathlib.Path(argv[argv.index("--repo-root") + 1]).resolve()
    repo = ""
    if "--repo" in argv:
        repo = argv[argv.index("--repo") + 1]
    files: list[str] = []
    if "--files" in argv:
        index = argv.index("--files") + 1
        while index < len(argv) and not argv[index].startswith("--"):
            files.append(argv[index])
            index += 1
    try:
        if not files:
            base = "origin/main"
            if "--base" in argv:
                base = argv[argv.index("--base") + 1]
            files = changed_files(root, base)
        pkg_dirs, dependents, dev_dependents = load_graph(root)
        claims = None
        if "--claims" in argv:
            claims = parse_claims(pathlib.Path(argv[argv.index("--claims") + 1]), repo)
    except (RuntimeError, json.JSONDecodeError, OSError) as exc:
        say(f"FAIL impact: 取证失败（{exc}）——看不见不等于没有牵连")
        return 2
    result = compute(files, pkg_dirs, dependents, claims, repo, dev_dependents)
    if "--json" in argv:
        say(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        report(result)
    if "--strict" in argv and result["unclaimed"]:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
