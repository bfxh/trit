#!/usr/bin/env python3
"""本地钩子：推送前拦"直推主干"和"高置信度密钥"。

用法：
  python scripts/hooks.py install [--force]
  python scripts/hooks.py uninstall
  python scripts/hooks.py status
  python scripts/hooks.py --self-test

为什么还要本地钩子（CI 已经有门）：
  - CI 是**推送之后**才跑；直推主干/真密钥一旦推出去，清理成本高（要改写历史 + 轮换）；
  - 钩子只拦本次推送涉及的 ref，不走网络、不依赖平台设置；
  - 钩子带标记，`uninstall` 只删自己装的，不碰别人的 pre-push。
"""

from __future__ import annotations

import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
HOOK_NAME = "pre-push"
MARKER = "# managed-by: repo-governance (hooks.py)"

HOOK_BODY = """#!/bin/sh
{marker}
# 1) 拒绝直推主干（确需时 ALLOW_MAIN_PUSH=1 绕过）
# 2) 跑仓库自带的高置信度密钥扫描（有 scripts/secret_scan.py 才跑）
set -u
ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
blocked=0
while read -r local_ref local_sha remote_ref remote_sha; do
  case "$remote_ref" in
    refs/heads/main|refs/heads/master)
      if [ "${{ALLOW_MAIN_PUSH:-}}" != "1" ]; then
        echo "pre-push: 拒绝直接推送到 $remote_ref（请走 PR；确需推送设 ALLOW_MAIN_PUSH=1）" >&2
        blocked=1
      fi
      ;;
  esac
done
[ "$blocked" = "1" ] && exit 1
if [ -f "$ROOT/scripts/secret_scan.py" ]; then
  if command -v python >/dev/null 2>&1; then PY=python; else PY=python3; fi
  if ! "$PY" "$ROOT/scripts/secret_scan.py" --root "$ROOT" >/dev/null 2>&1; then
    echo "pre-push: 密钥扫描未通过（跑 python scripts/secret_scan.py 看明细）" >&2
    exit 1
  fi
fi
exit 0
"""


def hooks_dir(root: pathlib.Path) -> pathlib.Path:
    return root / ".git" / "hooks"


def hook_path(root: pathlib.Path) -> pathlib.Path:
    return hooks_dir(root) / HOOK_NAME


def is_ours(path: pathlib.Path) -> bool:
    try:
        return MARKER in path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False


def install(root: pathlib.Path, force: bool) -> int:
    if not (root / ".git").exists():
        print(f"拒绝：{root} 不是 git 仓库")
        return 2
    path = hook_path(root)
    if path.exists() and not is_ours(path) and not force:
        print(f"拒绝：{path} 已存在且不是本工具装的（要覆盖加 --force）")
        return 2
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(HOOK_BODY.format(marker=MARKER), encoding="utf-8", newline="\n")
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    print(f"已安装 {path}")
    return 0


def uninstall(root: pathlib.Path) -> int:
    path = hook_path(root)
    if not path.exists():
        print("没有安装钩子")
        return 0
    if not is_ours(path):
        print(f"拒绝：{path} 不是本工具装的，不动它")
        return 2
    path.unlink()
    print(f"已卸载 {path}")
    return 0


def status(root: pathlib.Path) -> int:
    path = hook_path(root)
    if not path.exists():
        print("钩子：未安装")
        return 1
    print("钩子：已安装" + ("（本工具）" if is_ours(path) else "（**不是本工具装的**）"))
    return 0 if is_ours(path) else 1


def self_test() -> int:
    failures: list[str] = []
    total: list[int] = [0]

    def expect(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        subprocess.run(["git", "init", "-q"], cwd=root, check=False)
        expect("初始未安装", status(root), 1)
        expect("安装成功", install(root, force=False), 0)
        path = hook_path(root)
        expect("钩子文件存在", path.is_file(), True)
        expect("带标记", is_ours(path), True)
        expect("重复安装幂等", install(root, force=False), 0)

        sh = shutil.which("sh")
        if sh:
            def run_hook(stdin: str, env: dict | None = None) -> int:
                merged = {**os.environ, "PATH": os.environ.get("PATH", "")}
                if env:
                    merged.update(env)
                return subprocess.run(
                    [sh, str(path)], input=stdin, cwd=root, env=merged,
                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                    encoding="utf-8", errors="replace",
                ).returncode

            expect("推主干被拦",
                   run_hook("refs/heads/x abc refs/heads/main def\n"), 1)
            expect("推特性分支放行",
                   run_hook("refs/heads/x abc refs/heads/feat/x 0000\n"), 0)
            expect("ALLOW_MAIN_PUSH=1 可绕过",
                   run_hook("refs/heads/x abc refs/heads/main def\n", {"ALLOW_MAIN_PUSH": "1"}), 0)
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "secret_scan.py").write_text(
                "import sys\nsys.exit(1)\n", encoding="utf-8"
            )
            expect("有密钥扫描且失败时拦",
                   run_hook("refs/heads/x abc refs/heads/feat/x 0000\n"), 1)
            (scripts / "secret_scan.py").write_text("import sys\nsys.exit(0)\n", encoding="utf-8")
            expect("扫描通过时放行",
                   run_hook("refs/heads/x abc refs/heads/feat/x 0000\n"), 0)

        foreign = hooks_dir(root) / "pre-push"
        foreign.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        expect("外来钩子不被覆盖", install(root, force=False), 2)
        expect("外来钩子不算我们的", is_ours(foreign), False)
        expect("卸载不动外来钩子", uninstall(root), 2)
        install(root, force=True)
        expect("--force 覆盖后卸载成功", uninstall(root), 0)
        expect("卸载后文件没了", hook_path(root).exists(), False)

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
    command = argv[0] if argv else "status"
    if command == "install":
        return install(root, force="--force" in argv)
    if command == "uninstall":
        return uninstall(root)
    if command == "status":
        return status(root)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
