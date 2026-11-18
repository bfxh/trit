#!/usr/bin/env python3
"""AI 厂商标识门：URL / 邮箱 / trailer / remote 不得指向 AI 服务厂商。

为什么（用户口径，2026-11-19）：所有仓库禁止 AI 厂商账号或身份进入——版权与归属必须是用户本人。
判据只针对**归属与来源**，不针对「一般性提到 AI」（论文标题、文档叙述不算违规）：

1. URL 指向 AI 厂商组织：`github.com/claude`、`huggingface.co/openai` 等；
2. 邮箱域名是 AI 厂商：`*@anthropic.com`、`*@openai.com` 等；
3. commit trailer（`Co-authored-by` / `Generated-by` / `Assisted-by`）指向 AI 厂商；
4. git remote 地址指向 AI 厂商。

扫描面：工作树全部文本文件（跳过 `.git` / `target` / `node_modules` 等）+ `git log` 的
作者、提交者与提交信息 + `git remote -v`。

用法：
  python scripts/ai_vendor_scan.py              # 判定（命中即退出 1）
  python scripts/ai_vendor_scan.py --self-test   # 正例 + 反例
  python scripts/ai_vendor_scan.py --json
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = pathlib.Path(__file__).resolve().parent / "ai_vendor_baseline.json"
SELF = pathlib.Path(__file__).resolve()
SKIP_DIRS = {".git", "target", "node_modules", ".venv", "dist", "build", ".next", "__pycache__"}
TEXT_SUFFIXES = {
    ".rs",
    ".py",
    ".toml",
    ".lock",
    ".json",
    ".md",
    ".yml",
    ".yaml",
    ".sh",
    ".txt",
    ".c",
    ".h",
    ".wgsl",
    ".tsv",
    ".csv",
    ".jsonl",
}

VENDORS = (
    "anthropic",
    "openai",
    "claude",
    "copilot",
    "deepseek",
    "gemini",
    "chatgpt",
)

# 1) URL 指向厂商组织（host 或 host/path 第一段）
URL_RE = re.compile(
    r"(?i)https?://(?:[\w.-]+\.)?(?:github|gitlab|huggingface|pypi|npmjs|crates)\.(?:com|co|io|org)"
    r"/(?:" + "|".join(VENDORS) + r")\b"
)
# 2) 邮箱域名是厂商
EMAIL_RE = re.compile(
    r"(?i)[\w.+-]+@[\w.-]*\b(?:" + "|".join(VENDORS) + r")\.[a-z]{2,}\b"
)
# 3) trailer 指向厂商
TRAILER_RE = re.compile(
    r"(?im)^(?:co-authored-by|generated-by|assisted-by):[^\n]*\b(?:"
    + "|".join(VENDORS)
    + r"|gpt-[0-9])\b"
)
# 4) remote 地址指向厂商
REMOTE_RE = re.compile(
    r"(?i)(?:github|gitlab)\.(?:com|io)[:/](?:" + "|".join(VENDORS) + r")\b"
)


def scanned_files(root: pathlib.Path) -> list[pathlib.Path]:
    out: list[pathlib.Path] = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if path.resolve() == SELF:
            continue  # 本脚本必须包含厂商模式串与反例样例
        if any(part in SKIP_DIRS for part in path.relative_to(root).parts):
            continue
        if path.suffix.lower() in TEXT_SUFFIXES or path.name in {"Cargo.lock", "Cargo.toml"}:
            out.append(path)
    return out


def scan_text(text: str) -> list[str]:
    """返回 text 里命中的规则名（空 = 干净）。"""
    hits = []
    if URL_RE.search(text):
        hits.append("vendor-url")
    if EMAIL_RE.search(text):
        hits.append("vendor-email")
    if TRAILER_RE.search(text):
        hits.append("vendor-trailer")
    if REMOTE_RE.search(text):
        hits.append("vendor-remote")
    return hits


def scan_tree(root: pathlib.Path) -> list[dict]:
    problems: list[dict] = []
    for path in scanned_files(root):
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        hits = scan_text(text)
        if hits:
            problems.append({"where": str(path.relative_to(root)), "kinds": hits})
    return problems


def load_baseline() -> dict[str, list[str]]:
    if not BASELINE.exists():
        return {}
    try:
        data = json.loads(BASELINE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    raw = data.get("allowed_commits", {})
    return {str(k): list(v) for k, v in raw.items()}


def scan_git(root: pathlib.Path, baseline: dict[str, list[str]]) -> list[dict]:
    problems: list[dict] = []
    try:
        log = subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "log",
                "--all",
                "--pretty=format:@@@%h|%an <%ae>|%cn <%ce>|%s%n%b",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="ignore",
        ).stdout
    except OSError:
        return problems
    for chunk in log.split("@@@"):
        chunk = chunk.strip()
        if not chunk:
            continue
        hits = scan_text(chunk)
        if not hits:
            continue
        sha = chunk.split("|", 1)[0].strip()
        allowed = next((v for k, v in baseline.items() if sha.startswith(k)), None)
        if allowed and set(hits).issubset(set(allowed)):
            continue
        problems.append(
            {"where": "git-log", "line": chunk.splitlines()[0][:160], "kinds": hits}
        )
    try:
        remotes = subprocess.run(
            ["git", "-C", str(root), "remote", "-v"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="ignore",
        ).stdout
    except OSError:
        remotes = ""
    for line in remotes.splitlines():
        if REMOTE_RE.search(line):
            problems.append({"where": "git-remote", "line": line.strip(), "kinds": ["vendor-remote"]})
    return problems


def self_test() -> int:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "bad_url.md").write_text("see https://github.com/claude/x\n", encoding="utf-8")
        (root / "bad_email.txt").write_text("mail claude@anthropic.com\n", encoding="utf-8")
        (root / "bad_trailer.md").write_text("Co-authored-by: Claude <x@y>\n", encoding="utf-8")
        (root / "ok.md").write_text(
            "读一篇论文 *The Era of 1-bit LLMs*；外部语料地址不入库。\n"
            "https://github.com/bfxh/trit\n",
            encoding="utf-8",
        )
        found = {p["where"]: p["kinds"] for p in scan_tree(root)}
        expect = {
            "bad_url.md": "vendor-url",
            "bad_email.txt": "vendor-email",
            "bad_trailer.md": "vendor-trailer",
        }
        ok = True
        for name, kind in expect.items():
            if name not in found or kind not in found[name]:
                print(f"FAIL 正例没抓住：{name} 应命中 {kind}", file=sys.stderr)
                ok = False
        if "ok.md" in found:
            print(f"FAIL 反例被误伤：ok.md -> {found['ok.md']}", file=sys.stderr)
            ok = False
        print("自测结论：" + ("绿" if ok else "红"))
        return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    baseline = load_baseline()
    problems = scan_tree(ROOT) + scan_git(ROOT, baseline)
    if args.json:
        print(json.dumps(problems, ensure_ascii=False, indent=1))
    else:
        for p in problems[:40]:
            detail = p.get("line", "")
            print(f"  拦：{p['where']} {p['kinds']} {detail}")
        print(f"AI 厂商标识门：{'FAIL 拦 ' + str(len(problems)) + ' 条' if problems else 'OK 没有厂商 URL / 邮箱 / trailer / remote'}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
