#!/usr/bin/env python3
"""_run: execute one already-approved command inside a root, and report its exit code.

    _run.py -Root D:\\proj -Command "cargo test"

Deliberately separate from agent-verify: the chain must be able to *name* the moment
execution starts, so that everything before it (preflight, policy) and everything after it
(journal end) is visible in the trace. A command run inline in the orchestrator blurs that
boundary and makes "did the guard run before or after the write" unanswerable.

Shell choice: bash when available, because that is the default execution surface now.
Falling back to cmd on Windows keeps the tool usable on a machine without Git Bash, but the
fallback is printed -- a silent shell switch would change how quoting works without saying so.
"""

import os
import shutil
import subprocess
import sys


def parse_args(argv):
    out = {"root": "", "command": ""}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[{"root": "root", "command": "command"}.get(name, name)] = val
        i += 2
    return out


def pick_shell():
    """Pick Git Bash, never WSL bash.

    `shutil.which("bash")` on Windows resolves to System32\\bash.exe -- the WSL launcher.
    That lands in a different filesystem (D:\\ is /mnt/d) with a different PATH, so a command
    that works in a normal shell fails here, and the failure looks like the *project* broke.
    Silent wrong-environment is worse than a loud missing-shell, so Git Bash is chosen by
    path and anything else is only a last resort, with the risk printed.
    """
    cands = [os.environ.get("BASH"),
             r"C:\Program Files\Git\bin\bash.exe",
             r"C:\Program Files (x86)\Git\bin\bash.exe"]
    git = shutil.which("git")
    if git:
        cands.append(os.path.join(os.path.dirname(os.path.dirname(git)), "bin", "bash.exe"))
    for c in cands:
        if c and os.path.exists(c):
            return c, None
    other = shutil.which("bash")
    if other:
        return other, f"_run: 警告 -- 只找到 {other}，它可能是 WSL bash（文件系统视图不同）"
    return None, None


def main(argv):
    a = parse_args(argv)
    root = a.get("root", "").strip()
    command = a.get("command", "")
    if not root or not os.path.isdir(root):
        print("_run: -Root 必须是已存在的目录")
        return 2
    if not command.strip():
        print("_run: -Command 不能为空（空命令不算通过）")
        return 2

    bash, warn = pick_shell()
    if warn:
        print(warn)
    if bash:
        proc = subprocess.run([bash, "-c", command], cwd=root, capture_output=True,
                              text=True, encoding="utf-8", errors="replace")
    else:
        print("_run: 没有 bash，回退到系统 shell（引号语义会变）")
        proc = subprocess.run(command, cwd=root, shell=True, capture_output=True,
                              text=True, encoding="utf-8", errors="replace")

    lines = (proc.stdout + proc.stderr).splitlines()
    for ln in lines[-40:]:
        print(ln)
    return proc.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
