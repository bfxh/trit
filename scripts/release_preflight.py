#!/usr/bin/env python3
"""发布前 preflight：只允许从规范主仓的 clean main/release 分支发布。

它不构建、不写代码、不改 git；只读 git 状态并输出可入 release 清单的来源信息：

  python scripts/release_preflight.py \
      --out target/release-manifest.json \
      --artifact target/release/trit-build

默认拒绝：
  - 当前目录不是 git 顶层；
  - origin 不是 bfxh/trit；
  - 分支不匹配 main / release/*；
  - 工作区不 clean（含 untracked）；
  - 当前目录不是 `git worktree list` 的第一个（canonical）worktree。

`--allow-noncanonical` 只应给 CI/发布专用 worktree 使用，且会写进清单。
`--self-test` 用临时 git 仓跑正反例，不需要网络。
"""
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

PROJECT = "trit"
EXPECTED_REPO = "bfxh/trit"
PREFLIGHT_VERSION = 1
DEFAULT_BRANCHES = ["main", "release/*"]


def run(args: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=cwd,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        timeout=30,
    )


def git(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run(["git", *args], cwd=root)


def normalize_remote(url: str) -> str | None:
    """把 GitHub 的 ssh/https/ssh:// 形式归一成 owner/repo；本地路径返回 None。"""
    text = url.strip()
    if not text:
        return None
    text = re.sub(r"^git@([^:]+):", r"https://\1/", text)
    text = re.sub(r"^ssh://git@", "https://", text)
    text = text.rstrip("/")
    if text.endswith(".git"):
        text = text[:-4]
    match = re.match(r"^https?://github\.com/([^/]+/[^/]+)$", text)
    return match.group(1) if match else None


def tool_version(name: str) -> str | None:
    if shutil.which(name) is None:
        return None
    out = run([name, "--version"])
    return out.stdout.strip().splitlines()[0] if out.returncode == 0 and out.stdout else None


def target_dir(root: Path) -> str:
    env = os.environ.get("CARGO_TARGET_DIR")
    if env:
        return str(Path(env).expanduser().resolve())
    config = root / ".cargo" / "config.toml"
    if config.is_file():
        match = re.search(r'^\s*target-dir\s*=\s*"([^"]+)"', config.read_text(encoding="utf-8"), re.M)
        if match:
            return str(Path(match.group(1)).expanduser().resolve())
    return str(root / "target")


def collect(root: Path, allowed_branches: list[str], expected_repo: str, allow_noncanonical: bool):
    errors: list[str] = []
    root = root.resolve()
    top = git(root, "rev-parse", "--show-toplevel")
    if top.returncode != 0:
        return None, [f"不是 git 仓：{top.stderr.strip()}"]
    top_path = Path(top.stdout.strip()).resolve()
    if top_path != root:
        errors.append(f"root 不是 git 顶层：{root}（顶层是 {top_path}）")

    branch = git(root, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
    if branch == "HEAD":
        errors.append("detached HEAD 不能发布")
    elif not any(fnmatch.fnmatchcase(branch, pattern) for pattern in allowed_branches):
        errors.append(f"分支 {branch!r} 不在允许列表 {allowed_branches}")

    remote = git(root, "remote", "get-url", "origin")
    origin = remote.stdout.strip() if remote.returncode == 0 else ""
    if normalize_remote(origin) != expected_repo:
        errors.append(f"origin 不是 {expected_repo}（实际 {origin or '缺失'}）")

    status = git(root, "status", "--porcelain")
    dirty_lines = [line for line in status.stdout.splitlines() if line.strip()]
    if dirty_lines:
        errors.append(f"工作区不 clean（{len(dirty_lines)} 个条目，含 untracked）")

    head = git(root, "rev-parse", "HEAD").stdout.strip()
    tree = git(root, "rev-parse", "HEAD^{tree}").stdout.strip()
    describe = git(root, "describe", "--tags", "--always", "--dirty").stdout.strip()
    commit_at = git(root, "show", "-s", "--format=%cI", "HEAD").stdout.strip()

    worktrees = git(root, "worktree", "list", "--porcelain")
    first = next(
        (Path(line[len("worktree "):]).resolve() for line in worktrees.stdout.splitlines() if line.startswith("worktree ")),
        None,
    )
    canonical = first == root
    if not canonical and not allow_noncanonical:
        errors.append(f"当前不是 canonical worktree（canonical={first}）")

    manifest = {
        "project": PROJECT,
        "repo": expected_repo,
        "origin": origin,
        "root": str(root),
        "canonical_worktree": canonical,
        "branch": branch,
        "head": head,
        "tree": tree,
        "describe": describe,
        "commit_at": commit_at,
        "dirty": False,
        "target_dir": target_dir(root),
        "toolchain": {
            "rustc": tool_version("rustc"),
            "cargo": tool_version("cargo"),
            "python": sys.version.split()[0],
        },
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "preflight_version": PREFLIGHT_VERSION,
    }
    if manifest["toolchain"]["rustc"] is None:
        errors.append("找不到 rustc（发布机必须能构建）")
    if manifest["toolchain"]["cargo"] is None:
        errors.append("找不到 cargo（发布机必须能构建）")
    return manifest, errors


def add_artifacts(manifest: dict, artifacts: list[str]) -> list[str]:
    errors: list[str] = []
    rows = []
    for raw in artifacts:
        path = Path(raw).expanduser().resolve()
        if not path.is_file():
            errors.append(f"artifact 不存在：{path}")
            continue
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1 << 20), b""):
                digest.update(chunk)
        rows.append({"path": str(path), "sha256": digest.hexdigest(), "bytes": path.stat().st_size})
    manifest["artifacts"] = rows
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description="trit 发布前 preflight")
    parser.add_argument("--root", default=".", help="仓库根（默认当前目录）")
    parser.add_argument("--expected-repo", default=EXPECTED_REPO)
    parser.add_argument("--branch", action="append", dest="branches", help="允许分支，可重复（默认 main / release/*）")
    parser.add_argument("--artifact", action="append", default=[], help="产物路径，可重复")
    parser.add_argument("--out", help="通过时写 release manifest JSON")
    parser.add_argument("--json", action="store_true", help="机器可读输出")
    parser.add_argument("--allow-noncanonical", action="store_true", help="允许非第一个 worktree（CI 专用）")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    branches = args.branches or DEFAULT_BRANCHES
    manifest, errors = collect(Path(args.root), branches, args.expected_repo, args.allow_noncanonical)
    if manifest is not None:
        errors += add_artifacts(manifest, args.artifact)

    if errors:
        payload = {"ok": False, "errors": errors}
        print(json.dumps(payload, ensure_ascii=False) if args.json else "\n".join(f"FAIL {e}" for e in errors))
        return 1

    text = json.dumps(manifest, ensure_ascii=False, indent=1)
    if args.out:
        out = Path(args.out).expanduser()
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text + "\n", encoding="utf-8")
    print(text if args.json else f"OK {manifest['branch']} {manifest['head'][:12]} ({manifest['root']})")
    return 0


def self_test() -> int:
    failures: list[str] = []
    with tempfile.TemporaryDirectory(prefix="trit-preflight-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        run(["git", "init", "-b", "main"], cwd=root)
        run(["git", "config", "user.email", "test@example.com"], cwd=root)
        run(["git", "config", "user.name", "test"], cwd=root)
        (root / "README.md").write_text("test\n", encoding="utf-8")
        run(["git", "add", "README.md"], cwd=root)
        run(["git", "commit", "-m", "init"], cwd=root)
        run(["git", "remote", "add", "origin", "git@github.com:bfxh/trit.git"], cwd=root)
        script = str(Path(__file__).resolve())

        def invoke(*extra: str) -> subprocess.CompletedProcess[str]:
            return run([sys.executable, script, "--root", str(root), "--json", *extra])

        if invoke().returncode != 0:
            failures.append("clean main 应该通过")
        (root / "dirty.txt").write_text("x\n", encoding="utf-8")
        if invoke().returncode == 0:
            failures.append("untracked 脏文件应该拒绝")
        (root / "dirty.txt").unlink()
        run(["git", "checkout", "-b", "feature/x"], cwd=root)
        if invoke().returncode == 0:
            failures.append("非发布分支应该拒绝")
        run(["git", "checkout", "main"], cwd=root)
        run(["git", "remote", "set-url", "origin", "git@github.com:other/trit.git"], cwd=root)
        if invoke().returncode == 0:
            failures.append("错误 origin 应该拒绝")
        run(["git", "remote", "set-url", "origin", "git@github.com:bfxh/trit.git"], cwd=root)

        linked = Path(tmp) / "linked"
        run(["git", "worktree", "add", "-b", "release/1", str(linked)], cwd=root)
        if run([sys.executable, script, "--root", str(linked), "--json"]).returncode == 0:
            failures.append("linked worktree 默认应该拒绝")
        if run([sys.executable, script, "--root", str(linked), "--json", "--allow-noncanonical"]).returncode != 0:
            failures.append("--allow-noncanonical 应该通过")

        sample = Path(tmp) / "artifact.bin"
        sample.write_bytes(b"trit")
        ok = invoke("--artifact", str(sample), "--out", str(Path(tmp) / "manifest.json"))
        if ok.returncode != 0:
            failures.append("artifact 应通过")
        else:
            data = json.loads(ok.stdout)
            if data["artifacts"][0]["sha256"] != hashlib.sha256(b"trit").hexdigest():
                failures.append("artifact sha256 不对")

    if failures:
        print("\n".join(f"FAIL {item}" for item in failures))
        return 1
    print("OK release_preflight self-test")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
