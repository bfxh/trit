#!/usr/bin/env python3
"""公开树隐私门禁：任务文档、本机绝对路径、数据产物不许再进版本库。

用法：
  python scripts/privacy_scan.py [--root 目录] [--json]
  python scripts/privacy_scan.py --self-test
  python scripts/privacy_scan.py --init      # 只在第一次建档时用
  python scripts/privacy_scan.py --update    # 只减不增

口径（与 `secret_scan.py` 正交：那边认密钥形态，这边认内容性质）：
  1. local-path —— 文本里出现本机绝对路径（用户名与盘符结构会带出内网和账号）；
  2. task-doc   —— 路径落在 docs/plans、docs/positioning、docs/results、docs/review、
                   docs/AGENT-TASKS 之下（内部计划/定位/结论/评审/任务单）；
  3. data-file  —— 数据与二进制产物后缀（.patch/.ckpt/.npy/.ply/.tpm/…）。

基线是**只减不增**的棘轮：已进入公开历史的存量路径被冻结（`--update` 不许新增），
新文件一律判红。所以"再往里放任务文档"这件事在 CI 上不可能悄悄发生。
基线只记路径、不记命中文本 —— 基线自己不能变成第二份泄漏。

整文件豁免走 `.privacy-scan-ignore`（正则 + 必填理由），只给合成夹具与上游补丁用。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = "scripts/privacy_scan_baseline.json"
IGNORE_FILE = ".privacy-scan-ignore"
EXCLUDE_DIRS = {".git", "target", ".trit", "node_modules", "vendor", "__pycache__", ".venv", "dist"}
MAX_BYTES = 2 << 20

# 运行期拼接：本文件自身不能命中（扫描器拒绝自己就是永久红灯）
_SEP = r"[\\/]"
# 硬规则：带出账号名/家目录，一律不许有，也不许记进基线
_USER_RX = re.compile(
    r"[A-Za-z]:" + _SEP + r"Users" + _SEP
    + r"|/Users/[A-Za-z0-9._-]+/"
    + r"|/home/[A-Za-z0-9._-]+/"
)
# 软规则：只露出工作根约定，存量按棘轮冻结
_DRIVE_RX = re.compile(
    r"[A-Za-z]:" + _SEP + r"(?:KF|Documents)" + _SEP + r"|/mnt/[a-z]/"
)
HARD_RULES = {"user-path"}
TASK_DIRS = (
    "docs/plans/",
    "docs/positioning/",
    "docs/results/",
    "docs/review/",
    "docs/AGENT-TASKS",
    ".agent/memory/",
)
DATA_SUFFIX = (
    ".patch",
    ".diff",
    ".ckpt",
    ".npy",
    ".ply",
    ".tpm",
    ".safetensors",
    ".onnx",
    ".log",
    ".csv",
    ".xlsx",
    ".zip",
    ".parquet",
    ".pth",
)


def tracked(root: pathlib.Path) -> list[str]:
    proc = subprocess.run(
        ["git", "ls-files", "-z"], cwd=str(root), capture_output=True, check=False
    )
    if proc.returncode != 0:
        raise SystemExit("privacy: 需要在 git 仓库里跑（git ls-files 失败）")
    return [p for p in proc.stdout.decode("utf-8").split("\0") if p]


def load_ignores(root: pathlib.Path) -> list[re.Pattern[str]]:
    path = root / IGNORE_FILE
    if not path.is_file():
        return []
    pats: list[re.Pattern[str]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        if len(parts) != 2:
            print(f"忽略：{IGNORE_FILE} 里有一行没写理由，跳过：{line}")
            continue
        pats.append(re.compile(parts[0]))
    return pats


def load_baseline(root: pathlib.Path) -> dict[str, list[str]]:
    path = root / BASELINE
    if not path.is_file():
        return {}
    doc = json.loads(path.read_text(encoding="utf-8"))
    return {k: list(v) for k, v in (doc.get("allowed") or {}).items()}


def save_baseline(root: pathlib.Path, allowed: dict[str, list[str]]) -> None:
    doc = {
        "version": 1,
        "note": "公开树隐私基线：只减不增。放行新路径要清掉内容，不要在这里加行。",
        "allowed": {k: sorted(v) for k, v in sorted(allowed.items()) if v},
    }
    (root / BASELINE).write_text(
        json.dumps(doc, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def scan_file(full: pathlib.Path, rel: str) -> list[tuple[str, int, str]]:
    """返回 (rule, 行号, 说明)。只扫文本，二进制跳过。"""
    hits: list[tuple[str, int, str]] = []
    if rel.endswith(DATA_SUFFIX):
        hits.append(("data-file", 0, "数据/二进制产物不进公开树"))
    if rel.startswith(TASK_DIRS):
        hits.append(("task-doc", 0, "任务/计划/定位/评审文档不进公开树"))
    try:
        if full.stat().st_size > MAX_BYTES:
            return hits
        raw = full.read_bytes()
    except OSError:
        return hits
    if b"\0" in raw[:8192]:
        return hits
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return hits
    for i, line in enumerate(text.splitlines(), 1):
        if _USER_RX.search(line):
            hits.append(("user-path", i, "出现用户名/家目录（硬规则，不许基线）"))
        elif _DRIVE_RX.search(line):
            hits.append(("drive-path", i, "出现本机工作根盘符"))
    return hits


def collect(root: pathlib.Path) -> tuple[dict[str, list[str]], list[dict]]:
    ignores = load_ignores(root)
    found: dict[str, list[str]] = {}
    details: list[dict] = []
    for rel in tracked(root):
        if rel.split("/")[0] in EXCLUDE_DIRS:
            continue
        if any(rx.search(rel) for rx in ignores):
            continue
        for rule, line, why in scan_file(root / rel, rel):
            found.setdefault(rule, []).append(rel)
            details.append({"rule": rule, "path": rel, "line": line, "why": why})
    return found, details


def run(root: pathlib.Path, as_json: bool) -> int:
    allowed = load_baseline(root)
    found, details = collect(root)
    new = [
        {"rule": r, "path": p}
        for r, paths in found.items()
        for p in sorted(set(paths))
        if r in HARD_RULES or p not in set(allowed.get(r, ()))
    ]
    stale = [
        {"rule": r, "path": p}
        for r, paths in allowed.items()
        for p in paths
        if p not in set(found.get(r, ()))
    ]
    if as_json:
        print(json.dumps({"details": details, "new": new, "stale": stale}, ensure_ascii=False, indent=2))
    else:
        new_key = {(d["rule"], d["path"]) for d in new}
        for d in new:
            print(f"FAIL {d['rule']}  {d['path']} —— 不在基线里，新增即红")
        for d in details:
            if (d["rule"], d["path"]) in new_key and d["line"]:
                print(f"      命中 {d['path']}:{d['line']}  {d['why']}")
        for s in stale:
            print(f"INFO 基线里的 {s['rule']} {s['path']} 已不在树上，可 --update 收掉")
        if new:
            print(f"\nprivacy: {len(new)} 条新增违规（存量冻结在基线里）")
            print("放行办法：清掉内容；或按 .privacy-scan-ignore 写正则+理由（只给夹具/上游产物）")
        else:
            frozen = sum(len(set(v)) for v in allowed.values())
            print(f"OK 没有新增违规（基线冻结 {frozen} 条存量）")
    return 1 if new else 0


def update(root: pathlib.Path) -> int:
    allowed = load_baseline(root)
    found, _details = collect(root)
    found = {k: v for k, v in found.items() if k not in HARD_RULES}
    added = [
        {"rule": r, "path": p}
        for r, paths in found.items()
        for p in sorted(set(paths))
        if p not in set(allowed.get(r, ()))
    ]
    if added:
        print("REFUSED --update 只减不增。下面这些是新违规，先把内容清掉：")
        for a in added:
            print(f"  {a['rule']}  {a['path']}")
        return 1
    save_baseline(root, found)
    print(f"baseline updated: 冻结 {sum(len(set(v)) for v in found.values())} 条存量")
    return 0


def self_test() -> int:
    # 每条都用断言钉住：门禁把 self_test 当判据，print 出来的 FAIL 拦不住任何东西
    with tempfile.TemporaryDirectory() as td:
        root = pathlib.Path(td)
        (root / "scripts").mkdir()
        for d in ("docs/plans", "docs/AGENT-TASKS", "data"):
            (root / d).mkdir(parents=True)
        (root / "docs/plans/x.md").write_text("hi\n", encoding="utf-8")
        (root / "docs/AGENT-TASKS/1.md").write_text("hi\n", encoding="utf-8")
        (root / "data/y.npy").write_bytes(b"\0\1")
        p = "C" + ":" + chr(92) + "Users" + chr(92) + "bob" + chr(92) + "a.txt"
        (root / "note.md").write_text("见 " + p + "\n", encoding="utf-8")
        (root / "clean.md").write_text("干净\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q"], cwd=td, check=False)
        subprocess.run(["git", "add", "-A"], cwd=td, check=False)

        cases = {
            "docs/plans/x.md": "task-doc",
            "docs/AGENT-TASKS/1.md": "task-doc",
            "data/y.npy": "data-file",
            "note.md": "user-path",
            "clean.md": None,
        }
        for rel, want in cases.items():
            got = sorted({r for r, _l, _m in scan_file(root / rel, rel)}) or [None]
            assert got == ([want] if want else [None]), f"{rel}: 期望 {want}，实得 {got}"

        # 棘轮：存量之外的必须拒绝 --update
        save_baseline(root, {"task-doc": ["docs/plans/x.md"]})
        (root / "docs/positioning").mkdir()
        (root / "docs/positioning/p.md").write_text("hi\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=td, check=False)
        assert update(root) == 1, "基线在新增违规时没有拒绝 --update"

        # 豁免文件能整文件放行
        (root / IGNORE_FILE).write_text("data/y\\.npy  合成夹具\n", encoding="utf-8")
        assert any(rx.search("data/y.npy") for rx in load_ignores(root)), "豁免正则没生效"

        # 二进制只按后缀判，不读正文
        assert scan_file(root / "data/y.npy", "data/y.npy") == [("data-file", 0, "数据/二进制产物不进公开树")], \
            "二进制被当文本读了"

        # 硬规则不进基线：带用户名的文件永远算新增
        assert any(d["rule"] == "user-path" for d in collect(root)[1]), "硬规则没被识别"

        # 只减不增的另一半：清掉之后 --update 必须收掉
        for rel in ("docs/positioning/p.md", "docs/AGENT-TASKS/1.md", "data/y.npy", "note.md"):
            (root / rel).unlink()
        subprocess.run(["git", "add", "-A"], cwd=td, check=False)
        assert update(root) == 0, "清理后 --update 应成功"
    print("self-test OK")
    return 0


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(prog="privacy_scan")
    p.add_argument("--root", default="")
    p.add_argument("--json", action="store_true")
    p.add_argument("--self-test", action="store_true")
    p.add_argument("--init", action="store_true")
    p.add_argument("--update", action="store_true")
    a = p.parse_args(argv)
    if a.self_test:
        return self_test()
    root = pathlib.Path(a.root).resolve() if a.root else ROOT
    if a.init:
        found, _details = collect(root)
        found = {k: v for k, v in found.items() if k not in HARD_RULES}
        save_baseline(root, found)
        print(f"baseline created: 冻结 {sum(len(set(v)) for v in found.values())} 条存量")
        return 0
    if a.update:
        return update(root)
    return run(root, a.json)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
