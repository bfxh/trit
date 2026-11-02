#!/usr/bin/env python3
"""依赖脱离体检：把"这个 crate 能不能整块摘出去、缺哪几块"逐条列出来。

用法：
  python scripts/deps.py plan                 # 全工作区：每个 crate 的脱离阻断项
  python scripts/deps.py plan trit-capacity   # 只看一个 crate
  python scripts/deps.py graph                # 内部依赖图（谁依赖谁）
  python scripts/deps.py --json               # 机器可读
  python scripts/deps.py --self-test          # 正例 + 逐项反证

只读、离线：不联网、不改任何文件，只读 Cargo.toml 与源码文本。
阻断项分五类（每一类都会在 `--self-test` 里有夹具）：
  继承字段   `version.workspace = true` 之类 —— 摘出去后没有 workspace 可继承；
  内部依赖   workspace 内其它 crate 的 path 依赖（`{ workspace = true }` 指向 path）；
  内部 dev   只用于测试的内部依赖（摘出去后测试编不过）；
  越界引用   `include_str!("../…")` / `#[path = "../…"]` 指到 crate 目录外；
  build.rs   构建脚本可能读工作区文件。
"""

from __future__ import annotations

import dataclasses
import json
import pathlib
import re
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - python < 3.11
    tomllib = None  # type: ignore[assignment]


@dataclasses.dataclass
class Crate:
    name: str
    rel: str
    data: dict
    publish: bool


def load_toml(path: pathlib.Path) -> dict:
    if tomllib is None:
        raise SystemExit("需要 python ≥ 3.11（tomllib）")
    with path.open("rb") as handle:
        return tomllib.load(handle)


def workspace(root: pathlib.Path) -> tuple[dict, dict]:
    manifest = load_toml(root / "Cargo.toml")
    return manifest.get("workspace", {}), manifest


def crates(root: pathlib.Path, ws: dict) -> list[Crate]:
    out: list[Crate] = []
    for member in ws.get("members", []):
        data = load_toml(root / member / "Cargo.toml")
        package = data.get("package", {})
        name = package.get("name", pathlib.PurePosixPath(member).name)
        publish = package.get("publish", True)
        out.append(Crate(name=name, rel=member, data=data, publish=publish is not False))
    return sorted(out, key=lambda c: c.name)


INHERITED_SUFFIX = ".workspace"


def inherited_fields(crate: Crate) -> list[str]:
    fields = [k for k in crate.data.get("package", {}) if k.endswith(INHERITED_SUFFIX)]
    # `version.workspace = true` 会被 tomllib 解析成 {"version": {"workspace": True}}，
    # 所以还要看一遍值为 dict 的字段。
    nested = [
        k
        for k, v in crate.data.get("package", {}).items()
        if isinstance(v, dict) and v.get("workspace") is True
    ]
    return sorted(set(fields + nested))


DEP_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


def classify_dep(name: str, spec, ws_deps: dict) -> dict:
    """把一条依赖判成 internal / external，并带上它要求的版本。"""
    if isinstance(spec, str):
        return {"name": name, "kind": "external", "version": spec, "inherited": False}
    if isinstance(spec, dict) and spec.get("workspace") is True:
        base = ws_deps.get(name)
        if isinstance(base, dict):
            kind = "internal" if "path" in base else "external"
            return {
                "name": name,
                "kind": kind,
                "path": base.get("path"),
                "version": base.get("version"),
                "inherited": True,
            }
        return {"name": name, "kind": "unknown", "inherited": True}
    if isinstance(spec, dict) and "path" in spec:
        return {
            "name": name,
            "kind": "internal",
            "path": spec["path"],
            "version": spec.get("version"),
            "inherited": False,
        }
    return {"name": name, "kind": "external", "version": (spec or {}).get("version"), "inherited": False}


OUTSIDE_REF = re.compile(r'(include_str!|include_bytes!|#\[path\s*=\s*)\(\s*"|"\s*\)|"([^"]+)"')


def outside_refs(crate_root: pathlib.Path) -> list[dict]:
    """源码里指到 crate 目录**之外**的引用（摘出去会丢文件）。

    必须解析相对路径再判：`src/matmul/baseline.rs` 里的 `../matmul.wgsl` 落在
    `src/` 内，仍是 crate 自己的文件 —— 只看前缀 `..` 会把它们全误报成越界
    （2026-11-02 本仓就误报了 5 处，公开数字已撤回）。
    """
    hits: list[dict] = []
    crate_abs = crate_root.resolve()
    for path in sorted(crate_root.rglob("*.rs")):
        if "target" in path.parts:
            continue
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        for number, line in enumerate(lines, 1):
            refs = [m.group(1) for m in re.finditer(r'(?:include_str!|include_bytes!)\(\s*"([^"]+)"', line)]
            refs += [m.group(1) for m in re.finditer(r'#\[path\s*=\s*"([^"]+)"', line)]
            for ref in refs:
                target = (path.parent / ref).resolve()
                try:
                    target.relative_to(crate_abs)
                    inside = True
                except ValueError:
                    inside = False
                if not inside:
                    hits.append({"file": path.name, "line": number, "ref": ref, "kind": "越界"})
                elif not target.is_file():
                    hits.append({"file": path.name, "line": number, "ref": ref, "kind": "缺失"})
    return hits


def plan_crate(root: pathlib.Path, crate: Crate, ws_deps: dict, by_name: dict[str, Crate]) -> dict:
    blockers: list[dict] = []
    for field in inherited_fields(crate):
        blockers.append({"kind": "继承字段", "detail": field})

    internal: list[str] = []
    for section in DEP_SECTIONS:
        for name, spec in sorted(crate.data.get(section, {}).items()):
            info = classify_dep(name, spec, ws_deps)
            if info["kind"] == "internal":
                target = by_name.get(name)
                unpublished = target is not None and not target.publish
                kind = "内部 dev" if section == "dev-dependencies" else "内部依赖"
                blockers.append(
                    {
                        "kind": kind,
                        "detail": f"{name}（{section}；path={info.get('path')}；"
                        f"版本={info.get('version') or '未写'}；"
                        f"{'不可发布' if unpublished else '可发布'}）",
                    }
                )
                if section != "dev-dependencies":
                    internal.append(name)

    refs = outside_refs(root / crate.rel)
    for ref in refs:
        blockers.append(
            {"kind": "越界引用", "detail": f"{ref['file']}:{ref['line']} → {ref['ref']}（{ref['kind']}）"}
        )

    if (root / crate.rel / "build.rs").is_file():
        blockers.append({"kind": "build.rs", "detail": "存在 build.rs（可能读工作区文件）"})

    return {
        "crate": crate.name,
        "path": crate.rel,
        "publish": crate.publish,
        "internal_deps": sorted(set(internal)),
        "blockers": blockers,
    }


def build(root: pathlib.Path) -> tuple[list[dict], dict]:
    ws, _ = workspace(root)
    ws_deps = ws.get("dependencies", {})
    crate_list = crates(root, ws)
    by_name = {c.name: c for c in crate_list}
    plans = [plan_crate(root, c, ws_deps, by_name) for c in crate_list]
    return plans, {c.name: c for c in crate_list}


def print_plan(plans: list[dict]) -> None:
    clean = [p for p in plans if not p["blockers"]]
    print(f"crates={len(plans)} 无阻断={len(clean)} 有阻断={len(plans) - len(clean)}")
    for plan in plans:
        flags = ", ".join(sorted({b["kind"] for b in plan["blockers"]})) or "可直接摘出"
        print(f"\n{plan['crate']}  [{plan['path']}]  publish={plan['publish']}")
        print(f"  阻断面: {flags}")
        for blocker in plan["blockers"]:
            print(f"    - {blocker['kind']}: {blocker['detail']}")


def print_graph(plans: list[dict]) -> None:
    for plan in plans:
        deps = ", ".join(plan["internal_deps"]) or "（无内部依赖）"
        print(f"{plan['crate']} -> {deps}")


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "Cargo.toml").write_text(
            "[workspace]\n"
            'members = ["crates/alpha", "crates/beta", "crates/gamma"]\n'
            "\n"
            "[workspace.package]\n"
            'version = "9.9.9"\n'
            "\n"
            "[workspace.dependencies]\n"
            'beta = { path = "crates/beta", version = "9.9.9" }\n'
            'serde = "1"\n',
            encoding="utf-8",
        )
        (root / "crates" / "alpha" / "src").mkdir(parents=True)
        (root / "crates" / "alpha" / "Cargo.toml").write_text(
            "[package]\n"
            'name = "alpha"\n'
            "version.workspace = true\n"
            "\n"
            "[dependencies]\n"
            "beta = { workspace = true }\n"
            'serde = { workspace = true }\n'
            "\n"
            "[dev-dependencies]\n"
            "beta = { workspace = true }\n"
            'gamma = { path = "../gamma" }\n',
            encoding="utf-8",
        )
        (root / "crates" / "alpha" / "src" / "lib.rs").write_text(
            'const OUT: &str = include_str!("../../outside.txt");\n'
            'const IN: &str = include_str!("../data/x.txt");\n'
            'const GONE: &str = include_str!("../data/absent.txt");\n',
            encoding="utf-8",
        )
        (root / "crates" / "outside.txt").write_text("x\n", encoding="utf-8")
        (root / "crates" / "alpha" / "data").mkdir(parents=True)
        (root / "crates" / "alpha" / "data" / "x.txt").write_text("x\n", encoding="utf-8")
        (root / "crates" / "alpha" / "build.rs").write_text("fn main() {}\n", encoding="utf-8")
        (root / "crates" / "beta" / "src").mkdir(parents=True)
        (root / "crates" / "beta" / "Cargo.toml").write_text(
            "[package]\n"
            'name = "beta"\n'
            'version = "9.9.9"\n'
            'publish = false\n',
            encoding="utf-8",
        )
        (root / "crates" / "beta" / "src" / "lib.rs").write_text("pub fn f() {}\n", encoding="utf-8")
        (root / "crates" / "gamma" / "src").mkdir(parents=True)
        (root / "crates" / "gamma" / "Cargo.toml").write_text(
            '[package]\nname = "gamma"\nversion = "9.9.9"\n', encoding="utf-8"
        )
        (root / "crates" / "gamma" / "src" / "lib.rs").write_text("pub fn g() {}\n", encoding="utf-8")

        plans, _ = build(root)
        by_name = {p["crate"]: p for p in plans}
        alpha = by_name["alpha"]
        kinds = [b["kind"] for b in alpha["blockers"]]
        check("继承字段被识别", "继承字段" in kinds, True)
        check("内部依赖被识别", "内部依赖" in kinds, True)
        check("内部 dev 被识别", "内部 dev" in kinds, True)
        check(
            "直接 path 的 dev 依赖也被识别为内部",
            any("gamma" in b["detail"] for b in alpha["blockers"] if b["kind"] == "内部 dev"),
            True,
        )
        check("越界 include 被识别", "越界引用" in kinds, True)
        ref_detail = " ".join(b["detail"] for b in alpha["blockers"] if b["kind"] == "越界引用")
        check("越界项点名 outside.txt", "outside.txt" in ref_detail, True)
        check("crate 内的 ../ 不算越界", "data/x.txt" in ref_detail, False)
        check("指向不存在的文件单列成缺失", "缺失" in ref_detail, True)
        check("build.rs 被识别", "build.rs" in kinds, True)
        check("内部依赖点名 beta", alpha["internal_deps"], ["beta"])
        check(
            "beta 不可发布被标出",
            any("不可发布" in b["detail"] for b in alpha["blockers"] if b["kind"] == "内部依赖"),
            True,
        )
        check("beta 自身无阻断", by_name["beta"]["blockers"], [])
        # 反证：把 include 改成 crate 内已存在的文件，越界与缺失都该消失
        (root / "crates" / "alpha" / "src" / "lib.rs").write_text(
            'const X: &str = include_str!("lib.rs");\n', encoding="utf-8"
        )
        plans2, _ = build(root)
        check(
            "crate 内 include 不算越界",
            [b for b in {p["crate"]: p for p in plans2}["alpha"]["blockers"] if b["kind"] == "越界引用"],
            [],
        )

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
        index = argv.index("--root")
        argv = argv[:index] + argv[index + 2 :]
    command = argv[0] if argv else "plan"
    rest = [a for a in argv[1:] if not a.startswith("--")]
    plans, _ = build(root)
    if command == "graph":
        print_graph(plans)
        return 0
    if command != "plan":
        print(f"未知命令: {command}")
        return 2
    if rest:
        selected = [p for p in plans if p["crate"] in set(rest)]
        if not selected:
            print(f"没有这些 crate：{', '.join(rest)}")
            return 2
        plans = selected
    if "--json" in argv:
        print(json.dumps(plans, ensure_ascii=False, indent=1))
    else:
        print_plan(plans)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
