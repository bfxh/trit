#!/usr/bin/env python3
"""仓库级密钥扫描：在 CI 里跑，和 GitHub 的 push protection 互补。

用法：
  python scripts/secret_scan.py [--root 目录] [--max-mb 2] [--json]
  python scripts/secret_scan.py --self-test

口径：
  - 只认**高置信度**前缀（各家 token 的固定形态 + 私钥头），不做 "password=" 这类猜测；
  - 跳过二进制（前 8 KiB 有 NUL）与超大文件，避免误报与内存爆掉；
  - 模式串运行期拼接：本文件自身不能命中（否则扫描器永远拒绝自己）。
"""

from __future__ import annotations

import json
import pathlib
import re
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXCLUDE_DIRS = {".git", "target", ".trit", "node_modules", "vendor", "__pycache__", ".venv", "dist"}
# 显式豁免：本行或上一行带这个标记就跳过（合成夹具用，代码评审能看见）
ALLOW_MARKER = "secret-scan: allow"

# 运行期拼接：源码里不出现能命中自己的字面量（自锁时踩过）
PATTERNS = [
    (re.compile("sk" + r"-[A-Za-z0-9]{16,}"), "sk- 形状密钥"),
    (re.compile("gh" + r"[posu]_[A-Za-z0-9]{30,}"), "GitHub token"),
    (re.compile("github" + r"_pat_[A-Za-z0-9_]{20,}"), "GitHub fine-grained PAT"),
    (re.compile("AK" + r"IA[0-9A-Z]{16}"), "AWS access key"),
    (re.compile("xox" + r"[baprs]-[A-Za-z0-9-]{10,}"), "Slack token"),
    (re.compile("gl" + r"pat-[A-Za-z0-9_-]{20,}"), "GitLab PAT"),
    (re.compile("AI" + r"za[0-9A-Za-z_-]{35}"), "Google API key"),
    (re.compile("hf" + r"_[A-Za-z0-9]{30,}"), "HuggingFace token"),
    (re.compile("np" + r"m_[A-Za-z0-9]{36}"), "npm token"),
    (re.compile("dckr" + r"_pat_[A-Za-z0-9_-]{20,}"), "Docker PAT"),
    (re.compile(r"-----BEGIN " + r"[A-Z ]*PRIVATE KEY-----"), "私钥"),
]


def is_binary(data: bytes) -> bool:
    return b"\x00" in data[:8192]


def scan_file(path: pathlib.Path, max_bytes: int) -> list[dict]:
    try:
        if path.stat().st_size > max_bytes:
            return []
        data = path.read_bytes()
    except OSError:
        return []
    if is_binary(data):
        return []
    text = data.decode("utf-8", errors="replace")
    lines = text.splitlines()
    hits: list[dict] = []
    for index, line in enumerate(lines):
        allowed = ALLOW_MARKER in line or (index > 0 and ALLOW_MARKER in lines[index - 1])
        if allowed:
            continue
        for pattern, name in PATTERNS:
            if pattern.search(line):
                hits.append({"line": index + 1, "rule": name})
    return hits


def iter_files(root: pathlib.Path) -> list[pathlib.Path]:
    out: list[pathlib.Path] = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if any(part in EXCLUDE_DIRS for part in path.parts):
            continue
        out.append(path)
    return sorted(out)


def scan(root: pathlib.Path, max_mb: int) -> list[dict]:
    max_bytes = max_mb * 1024 * 1024
    findings: list[dict] = []
    for path in iter_files(root):
        for hit in scan_file(path, max_bytes):
            findings.append(
                {"file": path.relative_to(root).as_posix(), "line": hit["line"], "rule": hit["rule"]}
            )
    return findings


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
        (root / "clean.py").write_text("x = 1\n", encoding="utf-8")
        (root / "leak.py").write_text('KEY = "sk' + "-abcdefghijklmnopqrstu\"\n", encoding="utf-8")
        (root / "key.pem").write_text("-----BEGIN " + "RSA PRIVATE KEY-----\n", encoding="utf-8")
        (root / "bin.dat").write_bytes(b"\x00" + "sk".encode() + b"-abcdefghijklmnopqrstu")
        (root / "big.txt").write_text("sk" + "-abcdefghijklmnopqrstu" + "\n" * 10, encoding="utf-8")
        (root / "sub").mkdir()
        (root / "sub" / "nested.txt").write_text("AK" + "IA" + "A" * 16 + "\n", encoding="utf-8")

        findings = scan(root, max_mb=2)
        by_file = {}
        for finding in findings:
            by_file.setdefault(finding["file"], []).append(finding["rule"])
        check("干净文件不报", "clean.py" in by_file, False)
        check("sk- 命中", bool(by_file.get("leak.py")), True)
        check("私钥头命中", bool(by_file.get("key.pem")), True)
        check("二进制不扫", "bin.dat" in by_file, False)
        check("嵌套目录会扫", bool(by_file.get("sub/nested.txt")), True)
        check("行号是 1 起", by_file.get("leak.py") and findings[0]["line"] == 1, True)
        (root / "allowed.rs").write_text(
            'let k = "sk' + '-abcdefghijklmnopqrstu"; // secret-scan: allow 合成夹具\n', encoding="utf-8"
        )
        (root / "allowed_prev.rs").write_text(
            "// secret-scan: allow 合成夹具\nlet k = \"AK" + "IA" + "A" * 16 + "\";\n", encoding="utf-8"
        )
        allowed = scan(root, max_mb=2)
        allowed_files = {finding["file"] for finding in allowed}
        check("同行标记被跳过", "allowed.rs" in allowed_files, False)
        check("上一行标记被跳过", "allowed_prev.rs" in allowed_files, False)
        check("超大文件跳过", scan(root, max_mb=0) == [], True)
        check("排除目录不扫", all(".git" not in f["file"] for f in findings), True)

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
    max_mb = int(argv[argv.index("--max-mb") + 1]) if "--max-mb" in argv else 2
    findings = scan(root, max_mb)
    if "--json" in argv:
        print(json.dumps(findings, ensure_ascii=False, indent=1))
    else:
        for finding in findings:
            print(f"{finding['file']}:{finding['line']}: 命中 {finding['rule']}")
        print(f"扫描完成：{len(findings)} 处命中")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
