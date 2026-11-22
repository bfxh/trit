#!/usr/bin/env python3
"""发布图门：可发布 crate 不允许依赖 `publish = false` 的 crate。

背景（真实踩过两次）：`cargo package --workspace` 会**连 `publish = false` 的 crate 一起打包**，
而指向未发布 crate 的路径依赖没有版本，打包直接失败。发布姿态是内部层不进 crates.io，
所以 CI 的 package 步骤只打包可发布 crate（scripts/package_publishable.py 会排除全部
不可发布 crate）。在这个姿态下：

- 可发布 → 实验层：发布链断了，**红**；
- 实验层 → 实验层：不参与打包，**绿**；
- 实验层 → 可发布：没问题，**绿**。

推论：可发布 crate 必须自包含；内部 crate 之间可以互相依赖，但一进入可发布面就必须
先补发布链。另外顺手检查可发布 crate 的 crates.io 必填字段。

用法：
    python scripts/publish_graph.py            # 检查并打印发布顺序
    python scripts/publish_graph.py --self-test  # 正例 + 逐项反证
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REQUIRED_FIELDS = ("description", "license", "repository")


def metadata(manifest_path: Path) -> dict:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest_path),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        raise SystemExit(f"cargo metadata 失败：\n{result.stderr[-2000:]}")
    return json.loads(result.stdout)


def is_publishable(package: dict) -> bool:
    publish = package.get("publish")
    # null = 默认可发布；[] = publish = false；非空数组 = 白名单。
    return publish is None or (isinstance(publish, list) and len(publish) > 0)


def check(manifest_path: Path) -> list[str]:
    data = metadata(manifest_path)
    by_name = {package["name"]: package for package in data["packages"]}
    failures: list[str] = []

    for package in data["packages"]:
        publishable = is_publishable(package)
        if publishable:
            for field in REQUIRED_FIELDS:
                if not package.get(field):
                    failures.append(f"MISSING {field}: {package['name']}")
        for dependency in package["dependencies"]:
            if dependency.get("path") is None:
                continue
            # dev-dependencies 不进发布链：`cargo package` 打包时剥离 path dev-deps
            # （2026-10-24 实证：trit-gpu dev-dep 指向 trit-texture，package rc=0）。
            if dependency.get("kind") == "dev":
                continue
            target = by_name.get(dependency["name"])
            if target is None:
                failures.append(
                    f"UNRESOLVED: {package['name']} -> {dependency['name']} 有路径但不在 workspace 里"
                )
                continue
            if publishable and not is_publishable(target):
                failures.append(
                    f"PUBLISHABLE->UNPUBLISHABLE: {package['name']} 依赖了未发布的 {target['name']}"
                    f"（{dependency['kind']}），会让 cargo package 失败"
                )
    return failures


def publish_order(manifest_path: Path) -> list[str]:
    data = metadata(manifest_path)
    packages = {package["name"]: package for package in data["packages"]}
    publishable = {
        name for name, package in packages.items() if is_publishable(package)
    }
    deps: dict[str, set[str]] = {
        name: {
            dependency["name"]
            for dependency in packages[name]["dependencies"]
            if dependency.get("path") is not None and dependency["name"] in publishable
        }
        for name in publishable
    }
    order: list[str] = []
    remaining = dict(deps)
    while remaining:
        ready = sorted(name for name, need in remaining.items() if not need)
        if not ready:
            raise SystemExit(f"发布图有环：{sorted(remaining)}")
        for name in ready:
            order.append(name)
            del remaining[name]
        for need in remaining.values():
            need.difference_update(ready)
    return order


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def _fixture(
    root: Path,
    consumer_publish: str,
    dependency_publish: str,
    fields: bool,
    dep_section: str = "dependencies",
) -> None:
    # a 是可发布方；fields=False 时故意只给它 description，用来测缺字段判红。
    extra = (
        'description = "x"\nlicense = "MIT"\nrepository = "https://example.com"'
        if fields
        else 'description = "x"'
    )
    _write(
        root / "Cargo.toml",
        '[workspace]\nmembers = ["a", "b"]\nresolver = "3"\n',
    )
    _write(
        root / "a" / "Cargo.toml",
        f'[package]\nname = "a"\nversion = "0.1.0"\nedition = "2024"\n{extra}\n'
        f'{consumer_publish}\n[{dep_section}]\nb = {{ path = "../b" }}\n',
    )
    _write(root / "a" / "src" / "lib.rs", "pub fn a() {}\n")
    _write(
        root / "b" / "Cargo.toml",
        f'[package]\nname = "b"\nversion = "0.1.0"\nedition = "2024"\ndescription = "b"\n'
        f'license = "MIT"\nrepository = "https://example.com"\n{dependency_publish}\n',
    )
    _write(root / "b" / "src" / "lib.rs", "pub fn b() {}\n")


def self_test() -> int:
    script = str(Path(__file__).resolve())
    failures = 0
    seen = 0

    def run_case(
        name: str,
        consumer: str,
        dependency: str,
        fields: bool,
        expect_ok: bool,
        needle: str,
        dep_section: str = "dependencies",
    ) -> None:
        nonlocal failures, seen
        seen += 1
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _fixture(root, consumer, dependency, fields, dep_section)
            result = subprocess.run(
                [sys.executable, script, "--manifest-path", str(root / "Cargo.toml")],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            good = (result.returncode == 0) == expect_ok and (expect_ok or needle in result.stdout)
            print(("PASS " if good else "FAIL ") + name)
            if not good:
                print(result.stdout[-800:])
                failures += 1

    run_case("green: 可发布 -> 可发布", "", "", True, True, "")
    run_case("green: 实验层 -> 可发布", "publish = false", "", True, True, "")
    run_case("red: 可发布 -> 实验层", "", "publish = false", True, False, "PUBLISHABLE->UNPUBLISHABLE")
    run_case(
        "green: 实验层 -> 实验层（不参与 package 链）",
        "publish = false",
        "publish = false",
        True,
        True,
        "",
    )
    run_case("red: 缺 license", "", "", False, False, "MISSING license")
    # dev-dependencies 不进发布链（cargo package 剥离 path dev-deps，2026-10-24 实证）：
    # 指向实验层必须绿；同形状的正式依赖仍然红（上面的用例已覆盖）。
    run_case(
        "green: dev-dependency -> 实验层",
        "",
        "publish = false",
        True,
        True,
        "",
        "dev-dependencies",
    )

    print(f"self-test: {seen - failures}/{seen} passed")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    manifest = ROOT / "Cargo.toml"
    mode = "check"
    rest = list(argv)
    while rest:
        arg = rest.pop(0)
        if arg == "--self-test":
            mode = "self-test"
        elif arg == "--manifest-path":
            manifest = Path(rest.pop(0))
        else:
            print(f"未知参数：{arg}")
            return 2
    if mode == "self-test":
        return self_test()

    failures = check(manifest)
    order = publish_order(manifest)
    data = metadata(manifest)
    total = len(data["packages"])
    print(f"workspace: {total} 个 crate，其中可发布 {len(order)} 个")
    print("发布顺序（按依赖）：")
    for name in order:
        print(f"  {name}")
    for failure in failures:
        print("FAIL " + failure)
    if failures:
        return 1
    print("OK 没有任何 crate 依赖未发布 crate，可发布 crate 的必填字段齐全")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
