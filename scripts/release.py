#!/usr/bin/env python3
"""发版一致性：版本号 ↔ 最新 tag ↔ CHANGELOG，缺一个就打 tag。

用法：
  python scripts/release.py check             # 只看，不一致就非零
  python scripts/release.py tag 0.13.0 [--apply] [--push]
  python scripts/release.py --self-test

口径：
  - 版本的唯一来源是根 `Cargo.toml` 的 `[workspace.package] version`；
  - 最新 tag 取 `git tag --sort=-v:refname` 的第一项（按版本序，不靠 describe）；
  - CHANGELOG 必须提到该版本号（或 `[Unreleased]` 非空，表示还没切版）；
  - `tag` 默认 dry-run；`--apply` 才建 annotated tag；`--push` 才推远端。
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SEMVER = re.compile(r"^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$")
WORKSPACE_VERSION = re.compile(r"(?m)^\[workspace\.package\][\s\S]*?^version\s*=\s*\"([^\"]+)\"")


def git(args: list[str], root: pathlib.Path) -> str:
    result = subprocess.run(
        ["git", *args], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, encoding="utf-8", errors="replace",
    )
    return result.stdout.strip() if result.returncode == 0 else ""


def workspace_version(root: pathlib.Path) -> str:
    manifest = root / "Cargo.toml"
    if not manifest.is_file():
        return ""
    match = WORKSPACE_VERSION.search(manifest.read_text(encoding="utf-8"))
    return match.group(1) if match else ""


def latest_tag(root: pathlib.Path) -> str:
    """按**版本序**取最高 tag。

    不用 `git describe`：多个 tag 指向同一提交时它挑哪个不确定（自测里 v1.0.0 与 v1.2.3 同提交，describe 返回了 1.0.0）。
    """
    tags = git(["tag", "--sort=-v:refname"], root).splitlines()
    return tags[0].lstrip("v") if tags else ""


def changelog_mentions(root: pathlib.Path, version: str) -> tuple[bool, bool]:
    """(有该版本的段落, [Unreleased] 非空)。"""
    path = root / "CHANGELOG.md"
    if not path.is_file():
        return False, False
    text = path.read_text(encoding="utf-8")
    has_version = re.search(rf"(?m)^##\s*\[?{re.escape(version)}\]?\s*$", text) is not None
    unreleased = False
    match = re.search(r"(?ms)^##\s*\[Unreleased\].*?(?=^##\s|\Z)", text)
    if match:
        body = match.group(0).split("\n", 1)[-1]
        unreleased = any(line.strip() and not line.startswith("#") for line in body.splitlines())
    return has_version, unreleased


def check(root: pathlib.Path) -> list[str]:
    problems: list[str] = []
    version = workspace_version(root)
    if not version:
        return ["读不到 [workspace.package] version"]
    if not SEMVER.match(version):
        problems.append(f"版本号不是 semver：{version}")
    tag = latest_tag(root)
    if tag and tag != version:
        problems.append(f"最新 tag({tag}) 与工作区版本({version}) 不一致")
    if not tag:
        problems.append("仓库还没有任何 tag")
    has_version, unreleased = changelog_mentions(root, version)
    if not has_version and not unreleased:
        problems.append(f"CHANGELOG 既没有 [{version}] 段落，[Unreleased] 也是空的")
    return problems


def make_tag(root: pathlib.Path, version: str, apply: bool, push: bool) -> int:
    if not SEMVER.match(version):
        print(f"拒绝：版本号不是 semver：{version}")
        return 2
    current = workspace_version(root)
    if current and current != version:
        print(f"拒绝：Cargo.toml 里是 {current}，要打的 tag 是 {version}（先改版本号）")
        return 2
    if git(["rev-parse", "--verify", f"refs/tags/v{version}"], root):
        print(f"拒绝：tag v{version} 已存在")
        return 2
    has_version, unreleased = changelog_mentions(root, version)
    if not has_version and not unreleased:
        print(f"拒绝：CHANGELOG 没有 [{version}]，[Unreleased] 也是空的")
        return 2
    commands = [["git", "tag", "-a", f"v{version}", "-m", f"v{version}"]]
    if push:
        commands.append(["git", "push", "origin", f"v{version}"])
    if not apply:
        print("dry-run：将执行")
        for command in commands:
            print("  " + " ".join(command))
        return 0
    for command in commands:
        result = subprocess.run(
            command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, encoding="utf-8", errors="replace",
        )
        print(f"$ {' '.join(command[:3])} … rc={result.returncode}")
        if result.returncode != 0:
            print(result.stdout[-800:])
            return 1
    print(f"已创建 tag v{version}" + (" 并推送" if push else "（未推送；要推加 --push）"))
    return 0


def self_test() -> int:
    failures: list[str] = []
    total: list[int] = [0]

    def expect(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "Cargo.toml").write_text(
            '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "1.2.3"\n', encoding="utf-8"
        )
        (root / "CHANGELOG.md").write_text("# Changelog\n\n## [Unreleased]\n\n- 一条真实改动\n", encoding="utf-8")
        expect("解析工作区版本", workspace_version(root), "1.2.3")
        subprocess.run(["git", "init", "-q"], cwd=root)
        subprocess.run(["git", "config", "user.email", "t@example.com"], cwd=root)
        subprocess.run(["git", "config", "user.name", "t"], cwd=root)
        problems = check(root)
        expect("没 tag 会被指出", any("还没有任何 tag" in p for p in problems), True)
        expect("[Unreleased] 非空则不要求版本段落", any("CHANGELOG" in p for p in problems), False)

        subprocess.run(["git", "add", "-A"], cwd=root)
        subprocess.run(["git", "commit", "-qm", "init"], cwd=root)
        subprocess.run(["git", "tag", "-a", "v1.0.0", "-m", "v1.0.0"], cwd=root)
        problems = check(root)
        expect("tag 落后于版本会被指出", any("不一致" in p for p in problems), True)

        expect("dry-run 不建 tag", make_tag(root, "1.2.3", apply=False, push=False), 0)
        expect("dry-run 后没有新 tag", git(["tag", "-l", "v1.2.3"], root), "")
        expect("版本不符时拒绝", make_tag(root, "9.9.9", apply=True, push=False), 2)
        (root / "CHANGELOG.md").write_text("# Changelog\n\n## [1.2.3]\n\n- 发版\n", encoding="utf-8")
        expect("--apply 建 tag", make_tag(root, "1.2.3", apply=True, push=False), 0)
        expect("tag 真的存在", git(["tag", "-l", "v1.2.3"], root), "v1.2.3")
        expect("重复打 tag 被拒", make_tag(root, "1.2.3", apply=True, push=False), 2)
        expect("打完后 check 通过", check(root), [])

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
    command = argv[0] if argv else "check"
    if command == "check":
        problems = check(root)
        for problem in problems:
            print(f"- {problem}")
        print("发版一致性：OK" if not problems else f"发版一致性：{len(problems)} 项不一致")
        return 0 if not problems else 1
    if command == "tag":
        rest = [a for a in argv[1:] if not a.startswith("--")]
        if not rest:
            print("用法：release.py tag <版本> [--apply] [--push]")
            return 2
        return make_tag(root, rest[0], "--apply" in argv, "--push" in argv)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
