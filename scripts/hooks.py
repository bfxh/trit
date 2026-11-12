#!/usr/bin/env python3
"""本地钩子：把多会话事故挡在“提交 / 切分支”这一刻。

用法：
  python scripts/hooks.py install [--force]   # 装 pre-push / pre-commit / post-checkout + 合并驱动
  python scripts/hooks.py uninstall
  python scripts/hooks.py status
  python scripts/hooks.py --self-test

四件事：
  - pre-push：拒绝直推 main（ALLOW_MAIN_PUSH=1 绕过）+ 高置信度密钥扫描；
  - pre-commit：共享工作树（默认 D:/KF/trit-wspace-wt）停在非 main 分支时拒绝提交
    （TRIT_ALLOW_SHARED_COMMIT=1 绕过）；main 上的提交一律拒绝（TRIT_ALLOW_MAIN_COMMIT=1 绕过）；
  - post-checkout：共享工作树被切到非 main 分支（含 detached）时自动切回 main，
    切不回就大声报（TRIT_ALLOW_SHARED_SWITCH=1 绕过）；
  - merge.json-union 驱动：三本棘轮台账（测试面 / 模块预算 / 文档卫生）并发修改时按固定策略
    合并且不产生冲突标记，避免“每次 main 前进就冲突、rebase、重跑一小时 mutation”的死循环。

为什么必须是本地钩子（2026-11-12 事故）：多个会话共用 D:/KF/trit-wspace-wt，一个会话在里面
反复切分支，另一个会话的读数和未提交改动全乱；main 被直接提交再 reset，分支爆炸。CI 只能拦
“推上去之后”，事故发生在提交和切分支的那一刻。钩子带标记，`uninstall` 只删自己装的。

安装会把本脚本与合并驱动复制到 hooks 目录作**稳定副本**（`trit-hooks.py` / `trit-json-union.py`），
钩子调副本而不是当前分支的 `scripts/hooks.py`：老分支缺新子命令时按分支路径调用会打印帮助并
rc=2，把所有老分支的提交误伤掉（2026-11-12 实测）。升级 hooks.py 后重跑一次 `install` 刷新副本。
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
MARKER = "# managed-by: repo-governance (hooks.py)"

HOOKS = ("pre-push", "pre-commit", "post-checkout")
MAIN_BRANCHES = ("main", "master")
DEFAULT_SHARED = "D:/KF/trit-wspace-wt"
DRIVER_NAME = "json-union"
DRIVER_SCRIPT = "scripts/merge_json_union.py"
STABLE_HOOK_SCRIPT = "trit-hooks.py"
STABLE_DRIVER_SCRIPT = "trit-json-union.py"
STABLE_UPDATE_SCRIPT = "trit-update-guard.py"
# 自动切回时带上的标记：嵌套的 post-checkout 看到它就立即退出，避免钩子自我递归
RECOVERY_ENV = "TRIT_SHARED_RECOVERY"

PRE_PUSH_BODY = """#!/bin/sh
{marker}
# 1) 拒绝直推主干（确需时 ALLOW_MAIN_PUSH=1 绕过）
# 2) 负更新守卫：非快进丢提交 / 棘轮回退（ALLOW_NON_FF_PUSH=1 / ALLOW_RATCHET_PUSH=1 绕过）
# 3) 跑仓库自带的高置信度密钥扫描（有 scripts/secret_scan.py 才跑）
set -u
ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
HOOKS=$(git rev-parse --git-path hooks 2>/dev/null) || HOOKS="$ROOT/.git/hooks"
case "$HOOKS" in
  /*|[A-Za-z]:*) ;;
  *) HOOKS="$ROOT/$HOOKS" ;;
esac
if command -v python >/dev/null 2>&1; then PY=python; else PY=python3; fi
PUSH_INPUT=$(cat)
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
done <<EOF
$PUSH_INPUT
EOF
[ "$blocked" = "1" ] && exit 1
if [ -f "$HOOKS/trit-update-guard.py" ]; then
  printf '%s' "$PUSH_INPUT" | "$PY" "$HOOKS/trit-update-guard.py" prepush || exit 1
fi
if [ -f "$ROOT/scripts/secret_scan.py" ]; then
  if ! "$PY" "$ROOT/scripts/secret_scan.py" --root "$ROOT" >/dev/null 2>&1; then
    echo "pre-push: 密钥扫描未通过（跑 python scripts/secret_scan.py 看明细）" >&2
    exit 1
  fi
fi
exit 0
"""

PRE_COMMIT_BODY = """#!/bin/sh
{marker}
# 共享工作树非 main / 任意 main 上的提交都在这里被拒
# （TRIT_ALLOW_SHARED_COMMIT=1 / TRIT_ALLOW_MAIN_COMMIT=1 点对点绕过）
# 逻辑跑 hooks 目录里的稳定副本，不依赖当前分支的 scripts/hooks.py（老分支会缺新子命令）。
set -u
HOOKS=$(git rev-parse --git-path hooks 2>/dev/null) || exit 0
case "$HOOKS" in
  /*|[A-Za-z]:*) ;;
  *) HOOKS="$(git rev-parse --show-toplevel)/$HOOKS" ;;
esac
[ -f "$HOOKS/trit-hooks.py" ] || exit 0
if command -v python >/dev/null 2>&1; then PY=python; else PY=python3; fi
"$PY" "$HOOKS/trit-hooks.py" hook-pre-commit
exit $?
"""

POST_CHECKOUT_BODY = """#!/bin/sh
{marker}
# 共享工作树被切到非 main 分支 ⇒ 自动切回 main（TRIT_ALLOW_SHARED_SWITCH=1 绕过）
# 逻辑跑 hooks 目录里的稳定副本，不依赖当前分支的 scripts/hooks.py。
set -u
[ "${{3:-0}}" = "1" ] || exit 0
HOOKS=$(git rev-parse --git-path hooks 2>/dev/null) || exit 0
case "$HOOKS" in
  /*|[A-Za-z]:*) ;;
  *) HOOKS="$(git rev-parse --show-toplevel)/$HOOKS" ;;
esac
[ -f "$HOOKS/trit-hooks.py" ] || exit 0
if command -v python >/dev/null 2>&1; then PY=python; else PY=python3; fi
"$PY" "$HOOKS/trit-hooks.py" hook-post-checkout
exit 0
"""

BODIES = {
    "pre-push": PRE_PUSH_BODY,
    "pre-commit": PRE_COMMIT_BODY,
    "post-checkout": POST_CHECKOUT_BODY,
}


def git(root: pathlib.Path, *args: str, env: dict | None = None) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            ["git", "-C", str(root), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=None if env is None else {**os.environ, **env},
        )
    except OSError as exc:
        return 127, str(exc)
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def hooks_dir(root: pathlib.Path) -> pathlib.Path:
    # worktree 的 .git 是文件，钩子实际在 common dir；问 git 而不是拼路径
    rc, out = git(root, "rev-parse", "--git-path", "hooks")
    if rc == 0 and out.strip():
        path = pathlib.Path(out.strip())
        return path if path.is_absolute() else (root / path)
    return root / ".git" / "hooks"


def hook_path(root: pathlib.Path, name: str) -> pathlib.Path:
    return hooks_dir(root) / name


def is_ours(path: pathlib.Path) -> bool:
    try:
        return MARKER in path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False


def norm(path: str) -> str:
    return os.path.normcase(os.path.abspath(os.path.normpath(path)))


def shared_worktrees(env: dict) -> set[str]:
    raw = env.get("TRIT_SHARED_WORKTREES", DEFAULT_SHARED)
    return {norm(part.strip()) for part in raw.split(os.pathsep) if part.strip()}


def toplevel(start: pathlib.Path) -> pathlib.Path:
    rc, out = git(start, "rev-parse", "--show-toplevel")
    if rc == 0 and out.strip():
        return pathlib.Path(out.strip())
    return start


def current_branch(root: pathlib.Path) -> str | None:
    rc, out = git(root, "symbolic-ref", "--short", "-q", "HEAD")
    if rc != 0 or not out.strip():
        return None
    return out.strip()


def in_shared(top: pathlib.Path, branch: str | None, env: dict) -> bool:
    if branch is not None and branch in MAIN_BRANCHES:
        return False
    return norm(str(top)) in shared_worktrees(env)


def hook_pre_commit(env: dict | None = None) -> int:
    env = env if env is not None else os.environ
    top = toplevel(pathlib.Path.cwd())
    branch = current_branch(top)
    if in_shared(top, branch, env):
        if env.get("TRIT_ALLOW_SHARED_COMMIT") == "1":
            print(
                f"pre-commit: 共享工作树 {top} 停在 {branch or '(detached)'}，"
                "已按 TRIT_ALLOW_SHARED_COMMIT=1 放行（风险自担）",
                file=sys.stderr,
            )
            return 0
        print(
            f"pre-commit: 拒绝在共享工作树 {top} 的 {branch or '(detached)'} 上提交。\n"
            "  共享工作树只供读取；请在自己的专属工作树做"
            "（python scripts/gitflow.py worktree add <分支>）。\n"
            "  确需在此提交：设 TRIT_ALLOW_SHARED_COMMIT=1。",
            file=sys.stderr,
        )
        return 1
    if branch is not None and branch in MAIN_BRANCHES:
        if env.get("TRIT_ALLOW_MAIN_COMMIT") == "1":
            print(
                f"pre-commit: 已按 TRIT_ALLOW_MAIN_COMMIT=1 放行对 {branch} 的直接提交",
                file=sys.stderr,
            )
            return 0
        print(
            f"pre-commit: 拒绝在 {branch} 上直接提交。\n"
            "  走 PR：git switch -c <分支> 后提交；确需：设 TRIT_ALLOW_MAIN_COMMIT=1。",
            file=sys.stderr,
        )
        return 1
    return 0


def hook_post_checkout(env: dict | None = None) -> int:
    env = env if env is not None else os.environ
    if env.get(RECOVERY_ENV) == "1":
        return 0
    top = toplevel(pathlib.Path.cwd())
    branch = current_branch(top)
    if not in_shared(top, branch, env):
        return 0
    if env.get("TRIT_ALLOW_SHARED_SWITCH") == "1":
        print(
            f"post-checkout: 共享工作树 {top} 被切到 {branch or '(detached)'}；"
            "TRIT_ALLOW_SHARED_SWITCH=1 已放行（完成后请切回 main）",
            file=sys.stderr,
        )
        return 0
    rc, out = 1, ""
    for target in MAIN_BRANCHES:
        rc, out = git(top, "switch", target, env={RECOVERY_ENV: "1"})
        if rc == 0:
            break
    if rc != 0:
        print(
            f"post-checkout: 共享工作树 {top} 被切到 {branch or '(detached)'}，"
            f"自动切回 main 失败——请人工处理：{out.strip()}",
            file=sys.stderr,
        )
        return 1
    print(
        f"post-checkout: 共享工作树 {top} 禁止停在 {branch or '(detached)'}，"
        "已自动切回 main（要保留请设 TRIT_ALLOW_SHARED_SWITCH=1）",
        file=sys.stderr,
    )
    return 0


def driver_command(hooks: pathlib.Path) -> str:
    exe = (sys.executable or "python").replace("\\", "/")
    script = str(hooks / STABLE_DRIVER_SCRIPT).replace("\\", "/")
    return f'"{exe}" "{script}" %O %A %B'


def install_merge_driver(root: pathlib.Path) -> int:
    command = driver_command(hooks_dir(root))
    pairs = (
        (f"merge.{DRIVER_NAME}.name", "deterministic union for ratchet ledgers"),
        (f"merge.{DRIVER_NAME}.driver", command),
    )
    for key, value in pairs:
        rc, out = git(root, "config", key, value)
        if rc != 0:
            print(f"拒绝：写 git config {key} 失败：{out.strip()}")
            return 2
    print(f"已配置合并驱动 merge.{DRIVER_NAME} = {command}")
    return 0


def install_stable_copies(root: pathlib.Path) -> int:
    hooks = hooks_dir(root)
    hooks.mkdir(parents=True, exist_ok=True)
    here = pathlib.Path(__file__).resolve()
    sources = (
        (here, STABLE_HOOK_SCRIPT),
        (here.parent / "merge_json_union.py", STABLE_DRIVER_SCRIPT),
        (here.parent / "branch_update.py", STABLE_UPDATE_SCRIPT),
    )
    for source, name in sources:
        try:
            shutil.copyfile(source, hooks / name)
        except OSError as exc:
            print(f"拒绝：复制钩子稳定副本 {name} 失败：{exc}")
            return 2
        (hooks / name).chmod(0o755)
    print(f"已更新钩子稳定副本：{STABLE_HOOK_SCRIPT}、{STABLE_DRIVER_SCRIPT}、{STABLE_UPDATE_SCRIPT}")
    return 0


def uninstall_merge_driver(root: pathlib.Path) -> None:
    for key in (f"merge.{DRIVER_NAME}.name", f"merge.{DRIVER_NAME}.driver"):
        git(root, "config", "--unset-all", key)


def install_one(root: pathlib.Path, name: str, force: bool) -> int:
    path = hook_path(root, name)
    if path.exists() and not is_ours(path) and not force:
        print(f"拒绝：{path} 已存在且不是本工具装的（要覆盖加 --force）")
        return 2
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(BODIES[name].format(marker=MARKER), encoding="utf-8", newline="\n")
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    print(f"已安装 {path}")
    return 0


def install(root: pathlib.Path, force: bool) -> int:
    if not (root / ".git").exists():
        print(f"拒绝：{root} 不是 git 仓库")
        return 2
    rc = install_stable_copies(root)
    if rc != 0:
        return rc
    for name in HOOKS:
        rc = install_one(root, name, force)
        if rc != 0:
            return rc
    return install_merge_driver(root)


def uninstall_one(root: pathlib.Path, name: str) -> int:
    path = hook_path(root, name)
    if not path.exists():
        return 3
    if not is_ours(path):
        print(f"拒绝：{path} 不是本工具装的，不动它")
        return 2
    path.unlink()
    print(f"已卸载 {path}")
    return 0


def uninstall(root: pathlib.Path) -> int:
    removed = 0
    for name in HOOKS:
        rc = uninstall_one(root, name)
        if rc == 2:
            return 2
        if rc == 0:
            removed += 1
    uninstall_merge_driver(root)
    hooks = hooks_dir(root)
    for name in (STABLE_HOOK_SCRIPT, STABLE_DRIVER_SCRIPT, STABLE_UPDATE_SCRIPT):
        stable = hooks / name
        if stable.exists():
            stable.unlink()
            print(f"已移除钩子稳定副本 {stable}")
    if not removed:
        print("没有安装钩子")
    return 0


def status(root: pathlib.Path) -> int:
    ok = True
    for name in HOOKS:
        path = hook_path(root, name)
        if not path.exists():
            print(f"{name}：未安装")
            ok = False
        elif is_ours(path):
            print(f"{name}：已安装（本工具）")
        else:
            print(f"{name}：已安装（**不是本工具装的**）")
            ok = False
    hooks = hooks_dir(root)
    for name in (STABLE_HOOK_SCRIPT, STABLE_DRIVER_SCRIPT, STABLE_UPDATE_SCRIPT):
        if (hooks / name).is_file():
            print(f"钩子稳定副本 {name}：已安装")
        else:
            print(f"钩子稳定副本 {name}：未安装")
            ok = False
    rc, value = git(root, "config", "--get", f"merge.{DRIVER_NAME}.driver")
    if rc == 0 and value.strip():
        print(f"合并驱动 merge.{DRIVER_NAME}：{value.strip()}")
    else:
        print(f"合并驱动 merge.{DRIVER_NAME}：未配置")
        ok = False
    return 0 if ok else 1


def self_test() -> int:
    failures: list[str] = []
    total: list[int] = [0]

    def expect(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp).resolve()
        base_env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("TRIT_ALLOW_", "TRIT_SHARED_"))
        }

        def call(
            cmd: list[str],
            cwd: pathlib.Path | None = None,
            env: dict | None = None,
            stdin_text: str | None = None,
        ) -> tuple[int, str]:
            merged = dict(base_env)
            if env:
                merged.update(env)
            proc = subprocess.run(
                cmd,
                cwd=cwd,
                env=merged,
                input=None if stdin_text is None else stdin_text.encode("utf-8"),
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            return proc.returncode, proc.stdout.decode("utf-8", "replace")

        def run_git(*args: str, env: dict | None = None) -> tuple[int, str]:
            return call(["git", "-C", str(root), *args], env=env)

        def head_branch() -> str:
            return run_git("symbolic-ref", "--short", "HEAD")[1].strip()

        run_git("init", "-q")
        run_git("config", "user.email", "hook@example.invalid")
        run_git("config", "user.name", "hooks selftest")
        run_git("config", "commit.gpgsign", "false")
        (root / "seed.txt").write_text("x\n", encoding="utf-8")
        run_git("add", "seed.txt")
        expect("初始提交（钩子未装）", run_git("commit", "-q", "-m", "base")[0], 0)
        default_branch = head_branch()
        expect("初始未安装", status(root), 1)

        scripts = root / "scripts"
        scripts.mkdir()
        here = pathlib.Path(__file__).resolve()
        shutil.copyfile(here, scripts / "hooks.py")
        shutil.copyfile(here.parent / "merge_json_union.py", scripts / "merge_json_union.py")

        expect("安装成功", install(root, force=False), 0)
        for name in HOOKS:
            expect(f"钩子文件存在 {name}", hook_path(root, name).is_file(), True)
            expect(f"带标记 {name}", is_ours(hook_path(root, name)), True)
        hooks_path = hooks_dir(root)
        for name in (STABLE_HOOK_SCRIPT, STABLE_DRIVER_SCRIPT, STABLE_UPDATE_SCRIPT):
            expect(f"稳定副本存在 {name}", (hooks_path / name).is_file(), True)
        rc, driver = run_git("config", "--get", f"merge.{DRIVER_NAME}.driver")
        expect("合并驱动已配置", rc, 0)
        expect("驱动命令指向稳定副本", STABLE_DRIVER_SCRIPT in driver, True)
        expect("重复安装幂等", install(root, force=False), 0)
        expect("安装后 status 为绿", status(root), 0)

        shared = {"TRIT_SHARED_WORKTREES": str(root)}
        rc, out = run_git("commit", "--allow-empty", "-m", "on main", env=shared)
        expect("main 上提交被拦", rc != 0, True)
        expect("拦 main 的原因可见", "直接提交" in out, True)
        rc, _ = run_git(
            "commit", "--allow-empty", "-m", "on main",
            env={**shared, "TRIT_ALLOW_MAIN_COMMIT": "1"},
        )
        expect("TRIT_ALLOW_MAIN_COMMIT 放行", rc, 0)

        rc, _ = run_git(
            "switch", "-c", "feat/probe",
            env={**shared, "TRIT_ALLOW_SHARED_SWITCH": "1"},
        )
        expect("shared 覆盖下可切分支", rc, 0)
        expect("停在 feat/probe", head_branch(), "feat/probe")
        rc, out = run_git("commit", "--allow-empty", "-m", "shared", env=shared)
        expect("共享树非 main 提交被拦", rc != 0, True)
        expect("拦共享树的原因可见", "共享工作树" in out, True)
        rc, _ = run_git(
            "commit", "--allow-empty", "-m", "shared",
            env={**shared, "TRIT_ALLOW_SHARED_COMMIT": "1"},
        )
        expect("TRIT_ALLOW_SHARED_COMMIT 放行", rc, 0)

        (scripts / "hooks.py").unlink()
        rc, out = run_git("commit", "--allow-empty", "-m", "stale", env=shared)
        expect("分支缺 hooks.py 仍被拦", rc != 0, True)
        expect("稳定副本执法原因可见", "共享工作树" in out, True)

        rc, _ = run_git("switch", default_branch)
        expect("切回主干", rc, 0)
        rc, out = run_git("switch", "-c", "feat/auto", env=shared)
        expect("自动切回后命令不报错", rc, 0)
        expect("共享树被自动切回主干", head_branch(), default_branch)
        expect("自动切回的提示可见", "自动切回 main" in out, True)

        rc, _ = run_git("switch", "--detach", env=shared)
        expect("detached 也被自动切回", head_branch(), default_branch)

        rc, _ = run_git(
            "switch", "feat/auto",
            env={**shared, "TRIT_ALLOW_SHARED_SWITCH": "1"},
        )
        expect("override 下停在 feat/auto", head_branch(), "feat/auto")

        sh = shutil.which("sh")
        if sh:
            post = hook_path(root, "post-checkout")
            expect("非分支检出（$3=0）不动",
                   call([sh, str(post), "old", "new", "0"], cwd=root, env=shared)[0], 0)
            expect("仍在 feat/auto", head_branch(), "feat/auto")
            expect("$3=1 直接调钩子也切回",
                   call([sh, str(post), "old", "new", "1"], cwd=root, env=shared)[0], 0)
            expect("手动调用后回主干", head_branch(), default_branch)

            run_git("switch", "feat/auto",
                    env={**shared, "TRIT_ALLOW_SHARED_SWITCH": "1"})
            other = {"TRIT_SHARED_WORKTREES": str(root / "not-this-repo")}
            expect("非共享树不动分支",
                   call([sh, str(post), "old", "new", "1"], cwd=root, env=other)[0], 0)
            expect("仍在 feat/auto（非共享树）", head_branch(), "feat/auto")
            expect("重入保护：RECOVERY=1 立即返回",
                   call([sh, str(post), "old", "new", "1"], cwd=root,
                        env={**shared, RECOVERY_ENV: "1"})[0], 0)
            expect("重入保护下仍在 feat/auto", head_branch(), "feat/auto")
            run_git("switch", default_branch)

            pre = hook_path(root, "pre-push")
            head_sha = run_git("rev-parse", "HEAD")[1].strip()
            zeros = "0" * 40
            expect("推主干被拦",
                   call([sh, str(pre)], cwd=root,
                        stdin_text=f"refs/heads/x {head_sha} refs/heads/main {zeros}")[0], 1)
            expect("推特性分支放行",
                   call([sh, str(pre)], cwd=root,
                        stdin_text="refs/heads/x abc refs/heads/feat/x 0000\n")[0], 0)
            expect("ALLOW_MAIN_PUSH=1 可绕过",
                   call([sh, str(pre)], cwd=root,
                        stdin_text="refs/heads/x 0000000000000000000000000000000000000000 refs/heads/main 0000000000000000000000000000000000000000",
                        env={"ALLOW_MAIN_PUSH": "1"})[0], 0)
            (scripts / "secret_scan.py").write_text("import sys\nsys.exit(1)\n", encoding="utf-8")
            expect("有密钥扫描且失败时拦",
                   call([sh, str(pre)], cwd=root,
                        stdin_text="refs/heads/x abc refs/heads/feat/x 0000\n")[0], 1)
            (scripts / "secret_scan.py").write_text("import sys\nsys.exit(0)\n", encoding="utf-8")
            expect("扫描通过时放行",
                   call([sh, str(pre)], cwd=root,
                        stdin_text="refs/heads/x abc refs/heads/feat/x 0000\n")[0], 0)

            run_git("switch", "-q", "-c", "side",
                    env={**shared, "TRIT_ALLOW_SHARED_SWITCH": "1"})
            (root / "side.txt").write_text("s\n", encoding="utf-8")
            run_git("add", "side.txt")
            run_git("commit", "-q", "-m", "side",
                    env={**shared, "TRIT_ALLOW_SHARED_COMMIT": "1"})
            side_sha = run_git("rev-parse", "HEAD")[1].strip()
            run_git("switch", "-q", default_branch)
            expect("负更新守卫：快进放行",
                   call([sh, str(pre)], cwd=root, env=shared,
                        stdin_text=f"refs/heads/x {side_sha} refs/heads/x {head_sha}\n")[0], 0)
            drop_line = f"refs/heads/x {head_sha} refs/heads/x {side_sha}\n"
            expect("负更新守卫：丢提交拦截",
                   call([sh, str(pre)], cwd=root, env=shared, stdin_text=drop_line)[0], 1)
            expect("ALLOW_NON_FF_PUSH 放行",
                   call([sh, str(pre)], cwd=root,
                        env={**shared, "ALLOW_NON_FF_PUSH": "1"},
                        stdin_text=drop_line)[0], 0)

        foreign = hooks_dir(root) / "pre-push"
        foreign.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        expect("外来钩子不被覆盖", install(root, force=False), 2)
        expect("外来钩子不算我们的", is_ours(foreign), False)
        expect("卸载不动外来钩子", uninstall(root), 2)
        install(root, force=True)
        expect("--force 覆盖后卸载成功", uninstall(root), 0)
        for name in HOOKS:
            expect(f"卸载后钩子没了 {name}", hook_path(root, name).exists(), False)
        for name in (STABLE_HOOK_SCRIPT, STABLE_DRIVER_SCRIPT, STABLE_UPDATE_SCRIPT):
            expect(f"卸载后稳定副本没了 {name}", (hooks_dir(root) / name).exists(), False)
        rc, _ = run_git("config", "--get", f"merge.{DRIVER_NAME}.driver")
        expect("卸载移除合并驱动", rc != 0, True)

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
    if argv and argv[0] == "hook-pre-commit":
        return hook_pre_commit()
    if argv and argv[0] == "hook-post-checkout":
        return hook_post_checkout()
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
