#!/usr/bin/env python3
"""只打包可发布 crate。

`cargo package --workspace` 会把 `publish = false` 的 crate 也一起打包；只要其中
一个内部 crate 依赖另一个未发布 crate，cargo 就会去 crates.io 找依赖并失败。
发布姿态是：内部层（如 trit-ops ECC、trit-orch 编排）留在工作区，不进发布链。
因此这里按 cargo metadata 的 `publish` 字段把不可发布 crate 全部 `--exclude`，
只让可发布 crate 走 package。依赖方向由 scripts/publish_graph.py 单独把关。
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def is_publishable(package: dict) -> bool:
    publish = package.get("publish")
    return publish is None or (isinstance(publish, list) and len(publish) > 0)


def metadata() -> dict:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        raise SystemExit(f"cargo metadata 失败：\n{result.stderr[-2000:]}")
    return json.loads(result.stdout)


def excluded_names(packages: list[dict]) -> list[str]:
    return [p["name"] for p in packages if not is_publishable(p)]


def self_test() -> int:
    packages = [
        {"name": "publishable-default", "publish": None},
        {"name": "publishable-whitelist", "publish": ["crates-io"]},
        {"name": "internal-false", "publish": False},
        {"name": "internal-empty", "publish": []},
    ]
    assert is_publishable(packages[0])
    assert is_publishable(packages[1])
    assert not is_publishable(packages[2])
    assert not is_publishable(packages[3])
    assert excluded_names(packages) == ["internal-false", "internal-empty"]
    print("package-publishable self-test: 2/2 passed")
    return 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    dry_run = False
    allow_dirty = False
    for arg in argv:
        if arg == "--dry-run":
            dry_run = True
        elif arg == "--allow-dirty":
            allow_dirty = True
        else:
            print(f"未知参数：{arg}")
            return 2

    packages = metadata()["packages"]
    publishable = [p["name"] for p in packages if is_publishable(p)]
    internal = excluded_names(packages)
    print(f"package: {len(publishable)} publishable, {len(internal)} internal/excluded")
    for name in internal:
        print(f"  exclude {name}")

    cmd = ["cargo", "package", "--workspace", "--no-verify", "--locked"]
    for name in internal:
        cmd.extend(["--exclude", name])
    if allow_dirty:
        cmd.append("--allow-dirty")
    if dry_run:
        print("DRY-RUN " + " ".join(cmd))
        return 0
    result = subprocess.run(cmd, cwd=ROOT)
    return result.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
