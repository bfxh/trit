#!/usr/bin/env python3
"""一键更新第一步：--check 预检 + 快进预检 + 门读数，零写入。

用法：
  python scripts/self_update.py --check [--offline] [--base main] [--repo-root <路径>]
  python scripts/self_update.py --self-test

口径（docs/AUTO-UPDATE.md §2/§4 的第一步）：
  - 预检：非 detached HEAD；工作区干净（脏 ⇒ rc1，点名脏条目）；
  - fetch origin <base>（--offline 跳过）；fetch 失败 ⇒ rc1（看不见不等于没有）；
  - 分类：已最新 / 可快进 / 本地领先 / 分叉（分叉 ⇒ rc1，打印旧 SHA、新 SHA、分叉点）；
  - 门读数：只读门逐条跑（cargo fmt --check + 存在的 python 门），红 ⇒ rc1；
  - 零写入自证：前后 `git status --porcelain` 逐字节一致，不一致 ⇒ rc1 大声报；
  - --apply 未实现 ⇒ rc2（不替用户做落地决定）。

exit：0 预检与门全绿；1 预检/分叉/门红/零写入被破坏；2 用法/未实现。
"""

from __future__ import annotations

import pathlib
import subprocess
import sys


def say(text: str) -> None:
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    try:
        print(text)
    except UnicodeEncodeError:
        print(text.encode(encoding, "replace").decode(encoding, "replace"))


def git(root: pathlib.Path, *args: str) -> tuple[int, str]:
    proc = subprocess.run(
        ["git", "-C", str(root), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def gates_for(root: pathlib.Path) -> list[tuple[str, list[str]]]:
    """只读门清单：按存在性挑，空集要明说（真空集 ≠ 绿）。"""
    gates: list[tuple[str, list[str]]] = []
    if (root / "Cargo.toml").is_file():
        gates.append(("cargo fmt --check", ["cargo", "fmt", "--all", "--", "--check"]))
    for script in ("module_budget.py", "test_surface.py", "docs_hygiene.py",
                   "doc_pointers.py", "hazard_gate.py", "layers.py"):
        if (root / "scripts" / script).is_file():
            gates.append((script, [sys.executable, str(root / "scripts" / script)]))
    return gates


def check(root: pathlib.Path, offline: bool, base: str) -> tuple[int, list[str]]:
    rc_st, status = git(root, "status", "--porcelain")
    if rc_st != 0:
        return 1, [f"FAIL 读不了 git status（{status.strip()[:200]}）"]
    rc_sym, ref = git(root, "symbolic-ref", "-q", "HEAD")
    if rc_sym != 0:
        return 1, ["FAIL detached HEAD：更新只认分支（先 switch 回分支）"]
    branch = ref.strip().removeprefix("refs/heads/")
    if status.strip():
        dirty = status.strip().splitlines()
        return 1, [f"FAIL 脏工作区（{len(dirty)} 条）⇒ 拒绝更新，零改动："] + \
                  ["  " + line for line in dirty[:5]]
    if not offline:
        rc_f, fout = git(root, "fetch", "origin", base)
        if rc_f != 0:
            return 1, [f"FAIL fetch origin {base} 失败（看不见不等于没有）：{fout.strip()[-200:]}"]
    rc_o, old = git(root, "rev-parse", "HEAD")
    rc_n, new = git(root, "rev-parse", f"origin/{base}")
    if rc_o != 0 or rc_n != 0:
        return 1, [f"FAIL 读不到 HEAD 或 origin/{base}（--offline 且从没 fetch 过？）"]
    old, new = old.strip(), new.strip()
    lines = [f"旧 SHA {old[:12]} → 新 SHA {new[:12]}（分支 {branch}）"]
    if old == new:
        lines.append("已是最新（无需更新）")
    elif git(root, "merge-base", "--is-ancestor", old, new)[0] == 0:
        lines.append("可快进（--apply 才落地，本轮未实现）")
    elif git(root, "merge-base", "--is-ancestor", new, old)[0] == 0:
        lines.append("本地领先 origin（无需更新；推送走你自己的 PR 流程）")
    else:
        _, mb = git(root, "merge-base", old, new)
        return 1, lines + [f"FAIL 分叉（两边互不为祖先）⇒ 拒绝：分叉点 {mb.strip()[:12]}，"
                           "先自行 merge/rebase 对齐"]
    gates = gates_for(root)
    if not gates:
        lines.append("门：无（没有 Cargo.toml / 门脚本）——空集，不是绿")
    red: list[str] = []
    for name, cmd in gates:
        proc = subprocess.run(cmd, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        out = proc.stdout.decode("utf-8", "replace").strip()
        tail = out.splitlines()[-1] if out else ""
        verdict = "绿" if proc.returncode == 0 else "红"
        lines.append(f"门 {name}: {verdict}（rc={proc.returncode}）{tail[:120]}")
        if proc.returncode != 0:
            red.append(name)
    _, after = git(root, "status", "--porcelain")
    if after != status:
        return 1, lines + ["FAIL 零写入被破坏：status 前后不一致（本命令不许写任何文件）"]
    lines.append("零写入自证：status 前后逐字节一致")
    if red:
        return 1, lines + [f"FAIL 门红：{', '.join(red)}"]
    return 0, lines


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()

    def value(flag: str, default: str = "") -> str:
        return argv[argv.index(flag) + 1] if flag in argv and argv.index(flag) + 1 < len(argv) else default

    if "--apply" in argv:
        say("--apply 未实现（本轮只交付 --check：预检 + 门读数，零写入）；见 docs/AUTO-UPDATE.md")
        return 2
    if "--check" not in argv:
        say("用法：self_update.py --check [--offline] [--base main] [--repo-root <路径>]（--apply 未实现）")
        return 2
    root = pathlib.Path(value("--repo-root")) if "--repo-root" in argv else pathlib.Path.cwd()
    if not (root / ".git").exists():
        rc, out = git(root, "rev-parse", "--show-toplevel")
        if rc != 0:
            say(f"FAIL 不在 git 仓库里：{root}")
            return 2
        root = pathlib.Path(out.strip())
    rc, lines = check(root.resolve(), "--offline" in argv, value("--base", "main"))
    for line in lines:
        say(line)
    return rc


def self_test() -> int:
    import io
    import tempfile

    total, failures = 0, []

    def check_at(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            say(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    def run_main(args: list[str]) -> tuple[int, str]:
        buf = io.StringIO()
        saved = sys.stdout
        sys.stdout = buf
        try:
            rc = main(args)
        finally:
            sys.stdout = saved
        return rc, buf.getvalue()

    def sh(root: pathlib.Path, *args: str) -> None:
        proc = subprocess.run(["git", "-C", str(root), *args],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)}: {proc.stdout.decode('utf-8', 'replace')}")

    def mk_repo(path: pathlib.Path) -> None:
        path.mkdir(parents=True, exist_ok=True)
        sh(path, "init", "-q")
        sh(path, "checkout", "-q", "-b", "main")
        sh(path, "config", "user.email", "t@t")
        sh(path, "config", "user.name", "t")
        (path / "README.md").write_text("base\n", encoding="utf-8")
        sh(path, "add", "README.md")
        sh(path, "commit", "-q", "-m", "base")

    def with_remote(path: pathlib.Path, remote: pathlib.Path) -> None:
        remote.mkdir(parents=True, exist_ok=True)
        sh(remote, "init", "-q", "--bare")
        sh(path, "remote", "add", "origin", str(remote))
        sh(path, "push", "-q", "-u", "origin", "main")

    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        ws, remote = tmp / "ws", tmp / "remote.git"
        mk_repo(ws)
        with_remote(ws, remote)
        before = git(ws, "status", "--porcelain")[1]
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws)])
        check_at("干净且最新 ⇒ 绿", rc, 0)
        check_at("check() 直连：干净最新 ⇒ rc0", check(ws, True, "main")[0], 0)
        check_at("最新明说", "已是最新" in out, True)
        check_at("无门空集明说（不是绿）", "门：无" in out, True)
        check_at("零写入自证在输出里", "零写入自证" in out, True)
        check_at("干净树 status 前后一致", git(ws, "status", "--porcelain")[1], before)

        (ws / "dirty.txt").write_text("x", encoding="utf-8")
        dirty_before = git(ws, "status", "--porcelain")[1]
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws)])
        check_at("脏树 ⇒ 红", rc, 1)
        check_at("脏条目点名", ("脏工作区" in out, "dirty.txt" in out), (True, True))
        check_at("脏树零改动（逐字节）", git(ws, "status", "--porcelain")[1], dirty_before)
        (ws / "dirty.txt").unlink()

        clone2 = tmp / "clone2"
        sh(tmp, "clone", "-q", str(remote), str(clone2))
        sh(clone2, "checkout", "-q", "-B", "main", "origin/main")
        sh(clone2, "config", "user.email", "t@t")
        sh(clone2, "config", "user.name", "t")
        (clone2 / "next.txt").write_text("next\n", encoding="utf-8")
        sh(clone2, "add", "next.txt")
        sh(clone2, "commit", "-q", "-m", "next")
        sh(clone2, "push", "-q", "origin", "main")
        rc, out = run_main(["--check", "--repo-root", str(ws)])
        check_at("落后可快进 ⇒ 绿并明说", (rc, "可快进" in out), (0, True))

        (ws / "local.txt").write_text("local\n", encoding="utf-8")
        sh(ws, "add", "local.txt")
        sh(ws, "commit", "-q", "-m", "local")
        rc, out = run_main(["--check", "--repo-root", str(ws)])
        check_at("分叉 ⇒ 红", rc, 1)
        check_at("分叉打印旧/新/分叉点",
                 ("分叉" in out, "旧 SHA" in out, "分叉点" in out), (True, True, True))

        sh(ws, "checkout", "-q", "--detach")
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws)])
        check_at("detached ⇒ 红", (rc, "detached" in out), (1, True))
        sh(ws, "checkout", "-q", "main")

        rc, out = run_main(["--apply", "--repo-root", str(ws)])
        check_at("--apply 未实现 ⇒ rc2", (rc, "未实现" in out), (2, True))
        rc, _ = run_main(["--repo-root", str(ws)])
        check_at("不带 --check 给用法 ⇒ rc2", rc, 2)

        ws2 = tmp / "ws2"
        mk_repo(ws2)
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws2)])
        check_at("offline 且无 origin ⇒ 红（不是没有更新）", (rc, "读不到" in out), (1, True))
        ws3 = tmp / "ws3"
        mk_repo(ws3)
        sh(ws3, "remote", "add", "origin", str(tmp / "不存在.git"))
        rc, out = run_main(["--check", "--repo-root", str(ws3)])
        check_at("fetch 失败 ⇒ 红", (rc, "fetch" in out), (1, True))

        ws4, remote4 = tmp / "ws4", tmp / "remote4.git"
        mk_repo(ws4)
        with_remote(ws4, remote4)
        (ws4 / "Cargo.toml").write_text(
            chr(10).join(["[package]", 'name = "fu"', 'version = "0.1.0"',
                          'edition = "2021"', ""]), encoding="utf-8")
        (ws4 / "src").mkdir()
        (ws4 / "src" / "lib.rs").write_text("pub fn f( ) {  }\n", encoding="utf-8")
        sh(ws4, "add", "Cargo.toml", "src/lib.rs")
        sh(ws4, "commit", "-q", "-m", "rust")
        sh(ws4, "push", "-q", "origin", "main")
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws4)])
        check_at("fmt 红 ⇒ 门红判红", (rc, "cargo fmt --check: 红" in out), (1, True))
        (ws4 / "src" / "lib.rs").write_text("pub fn f() {}\n", encoding="utf-8")
        sh(ws4, "add", "src/lib.rs")
        sh(ws4, "commit", "-q", "-m", "fmt")
        sh(ws4, "push", "-q", "origin", "main")
        st4 = git(ws4, "status", "--porcelain")[1]
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws4)])
        check_at("fmt 绿 ⇒ 绿", (rc, "cargo fmt --check: 绿" in out), (0, True))
        check_at("门路径也零写入", git(ws4, "status", "--porcelain")[1], st4)

        (ws4 / "scripts").mkdir()
        (ws4 / "scripts" / "docs_hygiene.py").write_text(
            "import sys\nprint('stub ok')\nsys.exit(0)\n", encoding="utf-8")
        sh(ws4, "add", "scripts/docs_hygiene.py")
        sh(ws4, "commit", "-q", "-m", "stub gate")
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws4)])
        check_at("存在的 python 门真跑（绿）", (rc, "docs_hygiene.py: 绿" in out), (0, True))
        (ws4 / "scripts" / "docs_hygiene.py").write_text(
            "import sys\nprint('stub red')\nsys.exit(1)\n", encoding="utf-8")
        sh(ws4, "add", "scripts/docs_hygiene.py")
        sh(ws4, "commit", "-q", "-m", "stub red")
        rc, out = run_main(["--check", "--offline", "--repo-root", str(ws4)])
        check_at("python 门红 ⇒ 判红点名", (rc, "门红：docs_hygiene.py" in out), (1, True))

    say(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))