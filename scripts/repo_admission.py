#!/usr/bin/env python3
"""入库准入闸门（**默认强制**）：把「这东西是什么、凭什么进版本库」变成提交前的硬判定。

为什么要有它（2026-11-21，用户口径）：
  多智能体会话会因为**额度耗尽**被中断，工作目录随后被清理。Git 侧已经有 push 规程与
  归档脚本（docs/COLLAB.md §10），但**没有任何东西在管「什么东西被塞进了仓库」**：
  编译产物、权重、归档包、日志、批量抓下来的数据一旦入库，仓库会持续膨胀 → `clone` 与
  CI 拉取变慢 → CI 分钟额度更早烧完 → 门被跳过（`--no-verify`）→ 缺陷与脏产物一起进 main。
  这条链和「工作丢失」是同一个根：**没有强制、没有记录、没有解释**。

判定分三档（退出码 1 = 拦下提交）：
  BLOCK   无论如何都不许入库：编译产物、模型权重、归档/备份、日志转储、密钥文件、>2 MiB、
          高置信度密钥内容、提交信息里指向 AI 厂商的 trailer
  REASON  必须写明「入库理由:」才能过：二进制资产、>0.5 MiB 的文件、单次新增 >20 个文件
  FREE    源码 / 文档 / 配置 / 小型文本夹具

开关（用户可自己关，但关了也会被告知）：
  1. `.agent/admission.json` → {"enabled": true, "max_mb": 2, "reason_mb": 0.5, "allow": [...]}
  2. 环境变量 `TRIT_ADMISSION=off`
  3. `git config trit.admission off`
  **关闭时仍然全量判定**，只是不拦；命中项会连同「可能是哪一类问题、会连锁到哪」一起打印，
  并追加到 `.agent/admission.log`（不入库）。

用法：
  python scripts/repo_admission.py                  # 查已 staged 的文件（pre-commit 用）
  python scripts/repo_admission.py --all            # 体检：列出全部被跟踪文件里的违规（不退出 1）
  python scripts/repo_admission.py --all --strict   # 体检但违规即退出 1
  python scripts/repo_admission.py --explain        # 只打印策略表
  python scripts/repo_admission.py --self-test      # 正例 + 逐条反证

豁免：`.agent/admission.json` 的 `allow` 里每条 `{"path": "glob", "reason": "..."}`，
**reason 必填**——写不出理由就说明它不该入库。
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / ".agent" / "admission.json"
LOG = ROOT / ".agent" / "admission.log"
REASON_RE = re.compile(r"(?i)^\s*(?:入库理由|why-in-repo)\s*[:：]\s*(.*)$")
ALLOW_MARKER = "secret-scan: allow"

DEFAULT_CONFIG = {
    "enabled": True,
    "max_mb": 2.0,
    "reason_mb": 0.5,
    "bulk_files": 20,
    "allow": [],
}

# 1) 目录级黑名单：构建/缓存/运行时状态，无论叫什么名字都不入库
BLOCK_DIRS = {
    "target", "node_modules", "__pycache__", ".trit", ".venv", "venv", "env",
    "dist", "runs", "vendor", ".pytest_cache", ".mypy_cache", ".ruff_cache",
    ".mutation-check.lock", ".next", ".turbo", ".cache",
}
BLOCK_PATH_PREFIXES = ("playground/pkg", ".agent/memory")

# 2) 扩展名级黑名单
COMPILED = {
    ".rlib", ".rmeta", ".so", ".dylib", ".dll", ".pyd", ".exe", ".o", ".obj",
    ".a", ".lib", ".d", ".pdb", ".ilk", ".exp", ".pyc", ".pyo", ".class",
    ".wasm", ".gcda", ".gcno", ".profraw", ".profdata",
}
WEIGHTS = {
    ".safetensors", ".gguf", ".onnx", ".pt", ".pth", ".ckpt", ".tflite",
    ".mlmodel", ".npy", ".npz", ".parquet", ".arrow", ".feather", ".h5",
}
ARCHIVES = {
    ".bundle", ".pack", ".idx", ".zip", ".tar", ".tgz", ".gz", ".bz2", ".xz",
    ".7z", ".rar", ".orig", ".rej", ".patch", ".diff", ".bak",
}
LOGS = {".log", ".dmp", ".core", ".sqlite", ".sqlite3", ".db", ".trace", ".etl"}

# 3) 文件名级：密钥与本地环境
SECRET_NAMES = {
    "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".env", ".netrc",
    ".npmrc", ".pypirc", "credentials", "credentials.json", ".htpasswd",
}
SECRET_SUFFIXES = {".pem", ".key", ".p12", ".pfx", ".keystore", ".jks", ".kdbx", ".ppk"}

# 4) 允许的二进制"文档资产"（仍受体积门槛约束）
ASSETS = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".pdf", ".woff", ".woff2", ".ttf"}


def load_config() -> dict:
    cfg = dict(DEFAULT_CONFIG)
    if CONFIG.is_file():
        try:
            data = json.loads(CONFIG.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            print(f"[admission] {CONFIG} 不是合法 JSON（{exc}），按默认策略执行", file=sys.stderr)
        else:
            for key in DEFAULT_CONFIG:
                if key in data:
                    cfg[key] = data[key]
    return cfg


def switch_reason() -> str | None:
    """返回关闭原因；None = 开着。"""
    env = os.environ.get("TRIT_ADMISSION", "").strip().lower()
    if env in {"off", "0", "false", "no"}:
        return f"环境变量 TRIT_ADMISSION={env}"
    try:
        got = subprocess.run(
            ["git", "config", "--get", "trit.admission"], capture_output=True, text=True
        ).stdout.strip().lower()
    except OSError:
        got = ""
    if got in {"off", "0", "false", "no"}:
        return "git config trit.admission=off"
    if not load_config().get("enabled", True):
        return f"{CONFIG} 里 enabled=false"
    return None


def allowed(cfg: dict, rel: str) -> str | None:
    for item in cfg.get("allow", []):
        glob = item.get("path", "")
        reason = (item.get("reason") or "").strip()
        if not glob or not reason:
            print(f"[admission] allow 里有一条缺 path 或 reason，已忽略：{item}", file=sys.stderr)
            continue
        if fnmatch.fnmatch(rel, glob) or fnmatch.fnmatch("/" + rel, glob):
            return reason
    return None


def looks_binary(head: bytes) -> bool:
    return b"\x00" in head[:8192]


def secret_hits(text: str) -> list[str]:
    if ALLOW_MARKER in text:
        return []
    return [name for pattern, name in secret_scan_patterns() if pattern.search(text)]


_PATTERNS_CACHE: list | None = None


def secret_scan_patterns():
    """复用 scripts/secret_scan.py 的高置信度模式（运行期拼接，本文件不自锁）。"""
    global _PATTERNS_CACHE
    if _PATTERNS_CACHE is None:
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "_secret_scan", Path(__file__).resolve().parent / "secret_scan.py"
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        _PATTERNS_CACHE = module.PATTERNS
    return _PATTERNS_CACHE


def classify(rel: str, size: int, head: bytes, cfg: dict) -> tuple[str, str]:
    """返回 (档位, 规则名)。档位 ∈ BLOCK / REASON / FREE。"""
    parts = Path(rel).parts
    if any(part in BLOCK_DIRS for part in parts[:-1]):
        return "BLOCK", "构建产物目录"
    if any(rel == pre or rel.startswith(pre + "/") for pre in BLOCK_PATH_PREFIXES):
        return "BLOCK", "本地产物目录"

    name = parts[-1]
    suffix = Path(rel).suffix.lower()
    if name in SECRET_NAMES or suffix in SECRET_SUFFIXES:
        return "BLOCK", "密钥/凭据文件"
    if suffix in COMPILED:
        return "BLOCK", "编译产物"
    if suffix in WEIGHTS:
        return "BLOCK", "模型权重/数据列存"
    if suffix in ARCHIVES:
        return "BLOCK", "归档/备份/补丁"
    if suffix in LOGS:
        return "BLOCK", "日志/转储/本地数据库"
    if size > cfg["max_mb"] * 1024 * 1024:
        return "BLOCK", f"超过 {cfg['max_mb']} MiB"

    if head and not looks_binary(head):
        hits = secret_hits(head.decode("utf-8", "ignore"))
        if hits:
            return "BLOCK", "高置信度密钥（" + "、".join(hits) + "）"

    if suffix in ASSETS:
        return ("REASON", "二进制资产") if size > cfg["reason_mb"] * 1024 * 1024 else ("FREE", "-")
    if head and looks_binary(head):
        return "REASON", "二进制文件（需说明是什么）"
    if size > cfg["reason_mb"] * 1024 * 1024:
        return "REASON", f"超过 {cfg['reason_mb']} MiB（需说明为什么必须入库）"
    return "FREE", "-"


def reason_line(text: str) -> bool:
    for line in text.splitlines():
        match = REASON_RE.match(line)
        if match and len(match.group(1).strip()) >= 10:
            return True
    return False


def vendor_trailer(text: str) -> bool:
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "_ai_vendor_scan", Path(__file__).resolve().parent / "ai_vendor_scan.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return bool(module.TRAILER_RE.search(text))


def git(*args: str) -> bytes:
    return subprocess.run(["git", *args], capture_output=True, check=False).stdout


def staged_items() -> list[tuple[str, str, int, bytes]]:
    """[(rel, status, size, head)]，只取新增/修改/改名。"""
    raw = git("diff", "--cached", "--name-status", "--diff-filter=ACMR", "-z").decode("utf-8", "replace")
    fields = [f for f in raw.split("\0") if f]
    out = []
    i = 0
    while i + 1 < len(fields):
        status = fields[i]
        rel = fields[i + 1]
        i += 2
        if not rel:
            break
        blob = git("cat-file", "-s", f":{rel}")
        try:
            size = int(blob.strip() or 0)
        except ValueError:
            size = 0
        head = git("cat-file", "blob", f":{rel}")[:8192] if size <= 4 * 1024 * 1024 else b""
        out.append((rel, status[:1], size, head))
    return out


def tracked_items() -> list[tuple[str, str, int, bytes]]:
    raw = git("ls-files", "-z").decode("utf-8", "replace")
    out = []
    for rel in [f for f in raw.split("\0") if f]:
        path = ROOT / rel
        try:
            size = path.stat().st_size
            head = path.read_bytes()[:8192] if size <= 4 * 1024 * 1024 else b""
        except OSError:
            continue
        out.append((rel, "M", size, head))
    return out


def commit_message() -> str:
    for candidate in (ROOT / ".git" / "COMMIT_EDITMSG",):
        if candidate.is_file():
            return candidate.read_text(encoding="utf-8", errors="replace")
    path = subprocess.run(
        ["git", "rev-parse", "--git-path", "COMMIT_EDITMSG"], capture_output=True, text=True
    ).stdout.strip()
    if path:
        full = (ROOT / path) if not Path(path).is_absolute() else Path(path)
        if full.is_file():
            return full.read_text(encoding="utf-8", errors="replace")
    return ""


def advise(rule: str) -> str:
    table = {
        "构建产物目录": "构建产物不入库：改 .gitignore，或把它留在归档根目录（docs/COLLAB.md §10）",
        "本地产物目录": "本地产物不入库：智能体记忆与运行时状态属本机状态",
        "密钥/凭据文件": "凭据不入库：删掉 staged 项（git rm --cached），泄露后必须轮换",
        "编译产物": "编译产物不入库：target/ 已忽略，说明它为什么跑到源码树里来了",
        "模型权重/数据列存": "权重不入库：用发布包/外部存储，仓库只存指针与校验和",
        "归档/备份/补丁": "归档与备份不入库：放归档根目录（docs/COLLAB.md §10），恢复脚本见同名脚本",
        "日志/转储/本地数据库": "日志与本地数据库不入库：它们是运行期状态，不是源码",
        "高置信度密钥（": "疑似密钥：确认后删除并轮换；合成夹具加 `secret-scan: allow` 标注",
        "二进制资产": "二进制资产需要「入库理由:」——写清它是什么、为什么不能生成",
        "二进制文件（需说明是什么）": "二进制文件需要「入库理由:」——写清它是什么、为什么不能由源码生成",
    }
    for key, text in table.items():
        if rule.startswith(key):
            return text
    return "超过体积门槛：拆小、外部化，或写「入库理由:」说明为什么必须整体入库"


def report(items, cfg, *, strict, disabled: str | None) -> int:
    verdicts = []
    for rel, status, size, head in items:
        why = allowed(cfg, rel)
        if why:
            continue
        tier, rule = classify(rel, size, head, cfg)
        if tier != "FREE":
            verdicts.append({"path": rel, "tier": tier, "rule": rule, "size": size})

    blocks = [v for v in verdicts if v["tier"] == "BLOCK"]
    reasons = [v for v in verdicts if v["tier"] == "REASON"]
    added = [rel for rel, status, _, _ in items if status == "A"]
    msg = commit_message()
    need_reason = bool(reasons) or (len(added) > cfg["bulk_files"] and not msg)
    has_reason = reason_line(msg)
    trailer = vendor_trailer(msg) if msg else False

    if disabled:
        print(f"[admission] 已关闭（{disabled}）——仍然检查，只是不拦。")
    for v in blocks:
        prefix = "拦截" if not disabled else "风险"
        print(f"[admission] {prefix} {v['path']}（{v['size'] / 1024:.0f} KiB，{v['rule']}）", file=sys.stderr)
        print(f"            → {advise(v['rule'])}", file=sys.stderr)
    for v in reasons:
        print(f"[admission] 需理由 {v['path']}（{v['size'] / 1024:.0f} KiB，{v['rule']}）", file=sys.stderr)
    if need_reason and not has_reason:
        print("[admission] 本次入库存疑项，提交信息里缺一行「入库理由: <≥10 字：这是什么、凭什么入库>」", file=sys.stderr)
    if len(added) > cfg["bulk_files"]:
        print(f"[admission] 本次新增 {len(added)} 个文件（阈值 {cfg['bulk_files']}）：批量导入也要写「入库理由:」", file=sys.stderr)
    if trailer:
        print("[admission] 提交信息里有指向 AI 厂商的 trailer（Co-authored-by/Generated-by）：归属必须是用户本人", file=sys.stderr)

    risks = len(blocks) + len(reasons) + int(bool(trailer))
    if disabled:
        if risks:
            print("[admission] 可能的原因与连锁后果：")
            print("  - 编译产物/权重/归档入库 → 仓库膨胀 → clone 与 CI 拉取变慢 → CI 额度更早烧完 → 门被跳过 → 脏产物进 main")
            print("  - 密钥入库 → push protection 拦截或真泄露 → 必须轮换 + 重写历史 → 所有 worktree 失效 → 未推送工作丢失")
            print("  - 本机状态/日志入库 → 别人的 worktree 被覆盖 → 复现环境不一致")
            print(f"[admission] 重新开启：{CONFIG} 里 \"enabled\": true，或去掉 TRIT_ADMISSION=off")
            log_event(risks, blocks[:10], disabled)
        return 0

    hard_fail = bool(blocks) or bool(trailer) or (need_reason and not has_reason)
    if hard_fail:
        print("[admission] 结论：拒收。想放行只能改 .agent/admission.json 的 allow（必须写 reason）", file=sys.stderr)
        return 1
    if verdicts:
        print(f"[admission] OK（{len(verdicts)} 项已带理由放行）")
    else:
        print("[admission] OK")
    return 0


def log_event(count: int, blocks: list[dict], disabled: str) -> None:
    try:
        LOG.parent.mkdir(parents=True, exist_ok=True)
        entry = {
            "at": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "disabled_by": disabled,
            "risks": count,
            "blocked": [b["path"] for b in blocks],
        }
        with LOG.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(entry, ensure_ascii=False) + "\n")
    except OSError:
        pass


def explain() -> int:
    print("入库准入（强制）——能接受什么、不能接受什么\n")
    print("不能入库（BLOCK）")
    print("  编译产物    " + " ".join(sorted(COMPILED)))
    print("  权重/列存   " + " ".join(sorted(WEIGHTS)))
    print("  归档/备份   " + " ".join(sorted(ARCHIVES)))
    print("  日志/数据库 " + " ".join(sorted(LOGS)))
    print("  目录        " + " ".join(sorted(BLOCK_DIRS)))
    print("  密钥文件    " + " ".join(sorted(SECRET_NAMES | SECRET_SUFFIXES)))
    print("  其他        > max_mb 的任何文件；高置信度密钥内容；AI 厂商 trailer")
    print("\n需要写「入库理由:」才能入库（REASON）")
    print("  二进制资产（" + " ".join(sorted(ASSETS)) + "）超过 reason_mb")
    print("  任何超过 reason_mb 的文件；单次新增超过 bulk_files 个文件")
    print("\n可以直接入库（FREE）：源码、文档、配置、小型文本夹具")
    return 0


def self_test() -> int:
    cfg = dict(DEFAULT_CONFIG)
    ok = True

    def check(label, got, want):
        nonlocal ok
        if got != want:
            ok = False
            print(f"  FAIL {label}: 期望 {want}，实际 {got}")

    check("编译产物", classify("target/debug/x.rlib", 10, b"", cfg)[0], "BLOCK")
    check("目标目录里的源码也算产物", classify("crates/trit-ops/target/a.rs", 10, b"x", cfg)[0], "BLOCK")
    check("权重", classify("models/w.safetensors", 10, b"", cfg)[0], "BLOCK")
    check("归档", classify("backup/all.bundle", 10, b"", cfg)[0], "BLOCK")
    check("补丁备份", classify("src/a.rs.orig", 10, b"x", cfg)[0], "BLOCK")
    check("环境文件", classify("deploy/.env", 10, b"A=1", cfg)[0], "BLOCK")
    check("私钥", classify("keys/server.pem", 10, b"", cfg)[0], "BLOCK")
    check("超过 max_mb", classify("docs/big.md", int(3 * 1024 * 1024), b"text", cfg)[0], "BLOCK")
    key = "github" + "_pat_" + "A" * 24
    check("密钥内容", classify("src/conf.rs", 10, key.encode(), cfg)[0], "BLOCK")
    check("带豁免标记的密钥内容", classify("src/conf.rs", 10, (ALLOW_MARKER + key).encode(), cfg)[0], "FREE")
    check("大图需理由", classify("docs/pic.png", int(1024 * 1024), b"\x00\x01", cfg)[0], "REASON")
    check("小图放行", classify("docs/icon.png", 1024, b"\x00\x01", cfg)[0], "FREE")
    check("大文本需理由", classify("docs/notes.md", int(900 * 1024), b"text", cfg)[0], "REASON")
    check("普通源码放行", classify("crates/x/src/lib.rs", 2048, b"fn main() {}", cfg)[0], "FREE")

    check("理由行：合格", reason_line("fix: a\n\n入库理由: 这是 47 KB 的黄金语料，无法由源码生成"), True)
    check("理由行：太短", reason_line("入库理由: 需要"), False)
    check("理由行：缺失", reason_line("fix: a\n\n随便写点"), False)
    check("理由行：英文写法", reason_line("Why-in-repo: canonical fixture for the parser"), True)

    vendor = "co-authored-by: someone <x@" + "anthropic" + ".com>"
    check("厂商 trailer", vendor_trailer(vendor), True)
    check("干净 trailer", vendor_trailer("Co-authored-by: bfxh <bfxh@users.noreply.github.com>"), False)

    check("allow 生效", allowed({"allow": [{"path": "docs/*.pdf", "reason": "上游规范原文"}]}, "docs/rfc.pdf"),
          "上游规范原文")
    check("allow 缺 reason 不生效", allowed({"allow": [{"path": "docs/*.pdf", "reason": "  "}]}, "docs/rfc.pdf"),
          None)

    print("self-test:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="入库准入闸门（默认强制）")
    parser.add_argument("--all", action="store_true", help="体检全部被跟踪文件（默认不因违规退出 1）")
    parser.add_argument("--strict", action="store_true", help="与 --all 连用：违规即退出 1")
    parser.add_argument("--explain", action="store_true", help="打印策略表")
    parser.add_argument("--self-test", action="store_true", help="正例 + 逐条反证")
    args = parser.parse_args(argv)

    if args.explain:
        return explain()
    if args.self_test:
        return self_test()

    cfg = load_config()
    disabled = switch_reason()

    if args.all:
        items = tracked_items()
        if disabled:
            return report(items, cfg, strict=False, disabled=disabled)
        before = 0
        for rel, _, size, head in items:
            if allowed(cfg, rel):
                continue
            tier, rule = classify(rel, size, head, cfg)
            if tier != "FREE":
                print(f"[admission] 存量 {tier} {rel}（{size / 1024:.0f} KiB，{rule}）")
                before += 1
        print(f"[admission] 体检完成：{len(items)} 个被跟踪文件，{before} 项存疑"
              f"（存量不拦；新入库由 pre-commit 强制）")
        return 1 if (args.strict and before) else 0

    return report(staged_items(), cfg, strict=True, disabled=disabled)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
