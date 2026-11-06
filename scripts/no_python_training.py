#!/usr/bin/env python3
"""no-python-training：训练必须 Rust 三进制，Python 只能做对齐基线/开发门脚本。

死命令（D:\\KF\\AGENTS.md / 仓库 AGENTS.md）：
1. 模型、训练、推理、工具一律 Rust 三进制；
2. 不许用 Python 训练、外部模型或二进制现成件顶替；Python 只作对齐基线，不交付；
3. 模型/权重/训练产物归本仓（大件 D:\\KF\\trit-data），不许放 .yanagent/。

本门只查可机器判定的四类信号：
A. Python 训练入口：文件名以 train/finetune/fit/optim 开头。
B. Python 训练框架：import torch/tensorflow/jax/keras/pytorch_lightning。
C. crates/ 下非注释代码引用 .yanagent 路径（模型/训练产物落错地方）。
D. 仓库根 AGENTS.md 缺失，或不含“死命令/训练/Python”三个关键词。

允许 scripts/ 下的门脚本、构建脚本、数据生成脚本；export_*.py 只作对齐基线。
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
import tempfile

TRAIN_NAME = re.compile(r"^(?:train|finetune|fit|optim)", re.IGNORECASE)
TRAIN_FRAMEWORKS = re.compile(
    r"^\s*(?:import|from)\s+(torch|tensorflow|jax|keras|pytorch_lightning)\b",
    re.MULTILINE,
)
YANAGENT = re.compile(r"\.yanagent(?:[\\/]|$)")
RULE_NEEDLES = ("死命令", "训练", "Python")
SKIP_DIRS = {".git", "target", "node_modules", ".venv", "__pycache__", ".pytest_cache"}
TEXT_LIMIT = 1_000_000


def git_files(root: pathlib.Path, staged: bool) -> list[str] | None:
    cmd = ["git", "-C", str(root)]
    if staged:
        cmd += ["diff", "--cached", "--name-only", "--diff-filter=ACM", "-z"]
    else:
        cmd += ["ls-files", "-z"]
    try:
        out = subprocess.run(cmd, check=True, capture_output=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    return [p for p in out.decode("utf-8", "replace").split("\0") if p]


def walk_files(root: pathlib.Path) -> list[str]:
    found: list[str] = []
    for p in root.rglob("*"):
        if not p.is_file():
            continue
        if any(part in SKIP_DIRS for part in p.parts):
            continue
        found.append(p.relative_to(root).as_posix())
    return found


def read_text(p: pathlib.Path) -> str | None:
    try:
        if p.stat().st_size > TEXT_LIMIT:
            return None
        data = p.read_bytes()
    except OSError:
        return None
    if b"\0" in data[:4096]:
        return None
    return data.decode("utf-8", "replace")


def code_part(line: str, suffix: str) -> str:
    """去掉整行注释/行尾注释，只留下会被编译器或解释器执行的文本。"""
    if suffix == ".rs":
        stripped = line.lstrip()
        if stripped.startswith("//"):
            return ""
        # 字符串里的 // 可能误伤；这里宁可漏报，不误报
        return line.split("//", 1)[0]
    if suffix == ".toml":
        if line.lstrip().startswith("#"):
            return ""
        return line.split("#", 1)[0]
    return line


def check(
    root: pathlib.Path, staged: bool = False, names: list[str] | None = None
) -> tuple[list[str], int]:
    if names is not None:
        files = names
    else:
        listed = git_files(root, staged)
        files = listed if listed is not None else walk_files(root)
    findings: list[str] = []
    checked = 0
    for rel in files:
        p = root / rel
        if not p.is_file():
            continue
        checked += 1
        name = pathlib.PurePosixPath(rel).name
        suffix = p.suffix.lower()
        if suffix == ".py" and TRAIN_NAME.match(name) and name != "no_python_training.py":
            findings.append(
                f"{rel}: Python 训练入口（文件名以 train/finetune/fit/optim 开头）"
            )
        if suffix == ".py":
            text = read_text(p)
            if text is not None:
                for m in TRAIN_FRAMEWORKS.finditer(text):
                    line = text.count("\n", 0, m.start()) + 1
                    findings.append(
                        f"{rel}:{line}: Python 训练框架 import {m.group(1)}；训练必须 Rust 三进制"
                    )
        if suffix in (".rs", ".toml") and rel.startswith("crates/"):
            text = read_text(p)
            if text is not None:
                for lineno, line in enumerate(text.splitlines(), 1):
                    part = code_part(line, suffix)
                    if part and YANAGENT.search(part):
                        findings.append(
                            f"{rel}:{lineno}: 代码引用 .yanagent 路径；"
                            "模型/训练产物必须归本仓或 D:\\KF\\trit-data"
                        )
    rules = root / "AGENTS.md"
    text = read_text(rules) if rules.is_file() else None
    if text is None:
        findings.append("AGENTS.md: 仓库根缺少规则文件（死命令必须随仓库走）")
    else:
        missing = [w for w in RULE_NEEDLES if w not in text]
        if missing:
            findings.append(f"AGENTS.md: 缺少死命令关键词 {missing}")
    return findings, checked


def self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="no-python-training-") as tmp:
        root = pathlib.Path(tmp)
        (root / "crates" / "x" / "src").mkdir(parents=True)
        (root / "train_head.py").write_text("print('x')\n", encoding="utf-8")
        (root / "good.py").write_text("import numpy as np\n", encoding="utf-8")
        (root / "bad.py").write_text("import torch\n", encoding="utf-8")
        (root / "crates" / "x" / "src" / "lib.rs").write_text(
            "// .yanagent 只在注释里，允许\nlet _ = 1;\n", encoding="utf-8"
        )
        (root / "crates" / "x" / "src" / "bad.rs").write_text(
            'let p = ".yanagent/models";\n', encoding="utf-8"
        )
        (root / "AGENTS.md").write_text(
            "# 死命令\n训练必须 Python 之外的 Rust\n", encoding="utf-8"
        )
        findings, _ = check(root, names=walk_files(root))
        reasons = "\n".join(findings)
        ok = (
            len(findings) == 3
            and "train_head.py" in reasons
            and "bad.py" in reasons
            and "bad.rs" in reasons
            and "good.py" not in reasons
            and "lib.rs" not in reasons
        )
        (root / "AGENTS.md").unlink()
        findings2, _ = check(root, names=walk_files(root))
        ok = ok and any("AGENTS.md" in f for f in findings2)
        if ok:
            print("no-python-training: self-test OK（四类规则各有一个会红的反证）")
            return 0
        print("no-python-training: self-test FAIL")
        print(reasons)
        print("\n".join(findings2))
        return 1


def main() -> int:
    ap = argparse.ArgumentParser(description="训练必须 Rust 三进制；Python 只作对齐基线")
    ap.add_argument("--root", default=".")
    ap.add_argument("--staged", action="store_true", help="只检查已暂存文件（pre-commit）")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    root = pathlib.Path(args.root).resolve()
    if not root.is_dir():
        print(f"no-python-training: root 不存在: {root}", file=sys.stderr)
        return 2
    findings, checked = check(root, staged=args.staged)
    if args.json:
        print(
            json.dumps(
                {"checked": checked, "findings": findings}, ensure_ascii=False, indent=2
            )
        )
    else:
        print(f"no-python-training: 检查 {checked} 个文件")
        for f in findings:
            print(f"  FAIL {f}")
        if not findings:
            print("no-python-training: OK（训练/模型路径全部走 Rust 三进制）")
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
