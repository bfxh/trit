#!/usr/bin/env python3
"""分支更新判据：快进 / 干净重写 / 负更新（丢提交、删文件、棘轮回退）。

用法：
  python scripts/branch_update.py check --old <rev> --new <rev> [--json]
      [--allow-rewrite] [--allow-ratchet]
  python scripts/branch_update.py prepush          # 由 pre-push 钩子调用，stdin 是 git 的 ref 行
  python scripts/branch_update.py --self-test

判据（只认证据，查不到就判红）：
  - old 为全 0 或新提交不存在 => new-branch（OK）；
  - new 含 old（祖先）=> fast-forward（OK）；
  - 非快进：把 old 上"不在 new 里"的提交逐条按补丁等价比对（git cherry 同口径）：
      · 全部等价 => clean-rewrite（警告放行；rebase/amend 的常态）；
      · 有不等价 => negative-update（拦，列丢失提交与删除文件）。
  - 任何更新都查棘轮台账（old -> new，三本基线只许收紧）：
      · 测试面每个单位的计数不许变小；
      · 模块预算每个文件的上限不许变大或消失；
      · 文档卫生的 CHANGELOG 字节不许变小、盘符路径计数不许变大。
    => ratchet-regression（拦）。
  - 旧提交对象不在本仓 / git 失败 => unknown-old（拦：先 fetch；看不见不等于没问题）。

exit：0 安全；1 负更新/棘轮回退/取证失败；2 用法错。
prepush 只处理 refs/heads/*，删除分支（local SHA 全 0）放行；
绕过：ALLOW_NON_FF_PUSH=1（重写）、ALLOW_RATCHET_PUSH=1（棘轮）。
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
ZERO = "0" * 40
MAX_LIST = 20

TEST_SURFACE = "scripts/test_surface_baseline.json"
MODULE_BUDGET = "scripts/module_budget_baseline.json"
DOCS_HYGIENE = "scripts/docs_hygiene_baseline.json"


def say(text: str) -> None:
    stream = sys.stdout
    encoding = getattr(stream, "encoding", None) or "utf-8"
    try:
        print(text)
    except UnicodeEncodeError:
        print(text.encode(encoding, "replace").decode(encoding, "replace"))


def git(root: pathlib.Path, *args: str) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            ["git", "-C", str(root), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
    except OSError as exc:
        return 127, str(exc)
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def is_zero(sha: str) -> bool:
    text = (sha or "").strip()
    return bool(text) and set(text) <= {"0"}


def commit_exists(root: pathlib.Path, rev: str) -> bool:
    rc, _ = git(root, "cat-file", "-e", f"{rev}^{{commit}}")
    return rc == 0


def is_ancestor(root: pathlib.Path, old: str, new: str) -> bool:
    rc, _ = git(root, "merge-base", "--is-ancestor", old, new)
    return rc == 0


def lost_commits(root: pathlib.Path, old: str, new: str) -> list[str]:
    rc, out = git(root, "rev-list", old, f"^{new}")
    if rc != 0:
        return []
    return [line.strip() for line in out.splitlines() if line.strip()]


def non_equivalent_lost(root: pathlib.Path, new: str, old: str) -> list[str]:
    """old 上不在 new 里、且补丁不等价的提交（`git cherry` 的 `+` 行）。"""
    rc, out = git(root, "cherry", new, old)
    if rc != 0:
        return lost_commits(root, old, new)
    lost = []
    for line in out.splitlines():
        if line.startswith("+"):
            lost.append(line[1:].strip())
    return lost


def deleted_files(root: pathlib.Path, old: str, new: str) -> list[str]:
    rc, out = git(root, "diff", "--name-status", old, new)
    if rc != 0:
        return []
    deleted = []
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2 and parts[0].startswith("D"):
            deleted.append(parts[1])
    return deleted[:MAX_LIST]


def blob_text(root: pathlib.Path, rev: str, path: str) -> str | None:
    rc, out = git(root, "show", f"{rev}:{path}")
    return out if rc == 0 else None


def parse_json(text: str | None) -> tuple[dict | None, str]:
    if text is None:
        return None, ""
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        return None, f"无法解析（{exc}）"
    if not isinstance(data, dict):
        return None, "不是 JSON 对象"
    return data, ""


def unit_path(key: str) -> str | None:
    """测试面台账的单位键 -> 仓内路径（`script/x.py` / `crate/名`）。"""
    if key.startswith("script/"):
        return "scripts/" + key[len("script/"):]
    if key.startswith("crate/"):
        return "crates/" + key[len("crate/"):]
    return None


def new_tree_exists(root: pathlib.Path, new: str, path: str) -> bool:
    rc, _ = git(root, "cat-file", "-e", f"{new}:{path}")
    return rc == 0


def compare_test_surface(old: dict, new: dict, exists) -> list[str]:
    issues = []
    for key, value in old.items():
        if not isinstance(value, int):
            continue
        current = new.get(key)
        if isinstance(current, int) and current >= value:
            continue
        target = unit_path(key)
        if target and not exists(target):
            continue  # 单位随文件/crate 一起删除：是删减，不是回退
        issues.append(f"测试面 {key}: {value} -> {current if current is not None else '消失'}")
    return issues


def compare_module_budget(old: dict, new: dict, exists) -> list[str]:
    def caps(data: dict) -> dict[str, int]:
        out = {}
        for entry in data.get("caps", []) or []:
            if isinstance(entry, dict) and "file" in entry and "lines" in entry:
                out[str(entry["file"])] = int(entry["lines"])
        return out

    old_caps, new_caps = caps(old), caps(new)
    issues = []
    for path, lines in old_caps.items():
        current = new_caps.get(path)
        if current is not None and current <= lines:
            continue
        if not exists(path):
            continue  # 文件已删除：上限一起消失不是放松
        issues.append(f"模块预算 {path}: 上限 {lines} -> {current if current is not None else '消失'}")
    return issues


def compare_docs_hygiene(old: dict, new: dict) -> list[str]:
    issues = []
    old_bytes = old.get("changelog")
    new_bytes = new.get("changelog")
    if isinstance(old_bytes, int) and isinstance(new_bytes, int) and new_bytes < old_bytes:
        issues.append(f"文档卫生 CHANGELOG: {old_bytes} -> {new_bytes} 字节")
    old_paths = old.get("drive_paths", {}) or {}
    new_paths = new.get("drive_paths", {}) or {}
    for path, count in new_paths.items():
        if isinstance(count, int) and count > int(old_paths.get(path, 0)):
            issues.append(f"文档卫生盘符路径 {path}: {old_paths.get(path, 0)} -> {count}")
    return issues


def ratchet_regressions(root: pathlib.Path, old: str, new: str) -> list[str]:
    issues: list[str] = []
    exists = lambda path: new_tree_exists(root, new, path)  # noqa: E731
    checks = (
        (TEST_SURFACE, compare_test_surface),
        (MODULE_BUDGET, compare_module_budget),
    )
    for path, compare in checks:
        old_text = blob_text(root, old, path)
        new_text = blob_text(root, new, path)
        if old_text is None or new_text is None:
            continue
        old_data, old_err = parse_json(old_text)
        new_data, new_err = parse_json(new_text)
        if old_data is None or new_data is None:
            issues.append(f"{path}: {old_err or new_err}")
            continue
        issues.extend(compare(old_data, new_data, exists))
    old_text = blob_text(root, old, DOCS_HYGIENE)
    new_text = blob_text(root, new, DOCS_HYGIENE)
    if old_text is not None and new_text is not None:
        old_data, old_err = parse_json(old_text)
        new_data, new_err = parse_json(new_text)
        if old_data is None or new_data is None:
            issues.append(f"{DOCS_HYGIENE}: {old_err or new_err}")
        else:
            issues.extend(compare_docs_hygiene(old_data, new_data))
    return issues


ENTRY = re.compile(
    r"^(\d+|-):\s+([0-9a-f-]+)\s+([=!<>])\s+(\d+|-):\s+([0-9a-f-]+)\s*(.*)$"
)


def range_diff_audit(root: pathlib.Path, old: str, new: str) -> dict:
    """`git cherry` 只看补丁等价；再让 range-diff 把“落点/补丁被改写”点出来。

    只做取证：rebase 到新基线时补丁上下文变化（`!`）是常态，不单独定罪；
    但“旧提交在新历史里无对应”（`<`）与补丁等价的结论矛盾 ⇒ 调用方 fail-closed。
    """
    rc, base = git(root, "merge-base", old, new)
    if rc != 0 or not base.strip():
        return {"error": "算不出 merge-base"}
    rc, out = git(root, "range-diff", "--no-color",
                  f"{base.strip()}..{old}", f"{base.strip()}..{new}")
    if rc != 0:
        return {"error": out.strip()[:200] or "range-diff 失败"}
    result = {"equal": 0, "changed": [], "old_only": [], "new_only": 0}
    entry = None
    section = ""
    patch_changed = False

    def flush() -> None:
        if entry is not None and entry["op"] == "!" and patch_changed:
            result["changed"].append({"old": entry["old"], "subject": entry["subject"]})

    for line in out.splitlines():
        match = ENTRY.match(line)
        if match:
            flush()
            old_idx, old_sha, op, _new_idx, _new_sha, subject = match.groups()
            entry = {"op": op, "old": old_sha, "subject": subject.strip()}
            section, patch_changed = "", False
            if op == "=":
                result["equal"] += 1
            elif op == ">":
                result["new_only"] += 1
            elif op == "<" and old_idx != "-":
                result["old_only"].append({"old": old_sha, "subject": subject.strip()})
            continue
        if entry is None:
            continue
        stripped = line.strip()
        if stripped.startswith("## "):
            section = stripped[3:].strip().strip("#").strip()
            continue
        if section and section not in ("Metadata", "Commit message"):
            if stripped.startswith(("+", "-")) and not stripped.startswith(("+++", "---")):
                patch_changed = True
    flush()
    return result


def classify(
    root: pathlib.Path,
    old: str,
    new: str,
    *,
    allow_rewrite: bool = False,
    allow_ratchet: bool = False,
) -> dict:
    result = {
        "old": old,
        "new": new,
        "kind": "",
        "ok": True,
        "reasons": [],
        "lost": [],
        "deleted": [],
        "ratchet": [],
    }
    if is_zero(old) or not commit_exists(root, old):
        if is_zero(old):
            result["kind"] = "new-branch"
            return result
        result["kind"] = "unknown-old"
        result["ok"] = False
        result["reasons"].append(f"旧提交 {old} 不在本仓（先 fetch；看不见不等于没问题）")
        return result
    if not commit_exists(root, new):
        result["kind"] = "unknown-new"
        result["ok"] = False
        result["reasons"].append(f"新提交 {new} 不在本仓")
        return result
    if is_ancestor(root, old, new):
        result["kind"] = "fast-forward"
    else:
        lost = non_equivalent_lost(root, new, old)
        result["lost"] = lost
        if lost:
            result["kind"] = "negative-update"
            result["deleted"] = deleted_files(root, old, new)
            if not allow_rewrite:
                result["ok"] = False
                shown = ", ".join(sha[:10] for sha in lost[:MAX_LIST])
                result["reasons"].append(
                    f"负更新：{len(lost)} 个提交在新分支里没有等价补丁（{shown}）"
                )
        else:
            result["kind"] = "clean-rewrite"
            audit = range_diff_audit(root, old, new)
            result["range_diff"] = audit
            if audit.get("error"):
                result["reasons"].append(f"range-diff 取证失败（{audit['error']}）")
            if audit.get("old_only") and not allow_rewrite:
                result["ok"] = False
                result["reasons"].append("range-diff 显示旧提交在新历史里无对应（与补丁等价结论矛盾）")
            for item in (audit.get("changed") or [])[:5]:
                result["reasons"].append(
                    f"补丁落点与旧侧不同（人工确认）：{item['subject'] or item['old'][:10]}"
                )
            if not allow_rewrite:
                result["reasons"].append("非快进但补丁全部等价（rebase/amend，警告放行）")
    ratchet = ratchet_regressions(root, old, new)
    result["ratchet"] = ratchet
    if ratchet and not allow_ratchet:
        result["ok"] = False
        result["reasons"].extend(ratchet[:MAX_LIST])
    return result


def render(result: dict, root: pathlib.Path) -> None:
    status = "OK" if result["ok"] else "FAIL"
    say(f"{status} {result['kind']}｜{result['old'][:12]} -> {result['new'][:12]}")
    for reason in result["reasons"]:
        say(f"  - {reason}")
    for sha in result["lost"][:5]:
        rc_text = git(root, "log", "-1", "--format=%h %s", sha)[1].strip()
        say(f"    丢失提交 {rc_text or sha[:12]}")
    for path in result["deleted"][:10]:
        say(f"    删除文件 {path}")


def run_checked(
    root: pathlib.Path,
    old: str,
    new: str,
    *,
    allow_rewrite: bool,
    allow_ratchet: bool,
    as_json: bool,
) -> int:
    result = classify(root, old, new, allow_rewrite=allow_rewrite, allow_ratchet=allow_ratchet)
    if as_json:
        say(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        render(result, root)
    return 0 if result["ok"] else 1


def prepush(root: pathlib.Path, argv: list[str]) -> int:
    if "--repo" not in argv:
        # 钩子的稳定副本不在仓里：以当前工作树的顶层为准，而不是 __file__ 推出来的路径
        rc, top = git(pathlib.Path.cwd(), "rev-parse", "--show-toplevel")
        if rc == 0 and top.strip():
            root = pathlib.Path(top.strip())
    allow_rewrite = "--allow-rewrite" in argv or os.environ.get("ALLOW_NON_FF_PUSH") == "1"
    allow_ratchet = "--allow-ratchet" in argv or os.environ.get("ALLOW_RATCHET_PUSH") == "1"
    lines = [line for line in sys.stdin.read().splitlines() if line.strip()]
    failures = 0
    for line in lines:
        parts = line.split()
        if len(parts) != 4:
            say(f"FAIL prepush: 看不懂的输入行：{line!r}")
            failures += 1
            continue
        local_ref, local_sha, remote_ref, remote_sha = parts
        if not remote_ref.startswith("refs/heads/") or is_zero(local_sha):
            continue
        result = classify(
            root, remote_sha, local_sha,
            allow_rewrite=allow_rewrite, allow_ratchet=allow_ratchet,
        )
        if result["kind"] in ("new-branch", "fast-forward"):
            continue
        if result["ok"]:
            say(f"WARN prepush {local_ref}：{result['kind']}（"
                + "；".join(result["reasons"][:2]) + "）")
            continue
        failures += 1
        say(f"FAIL prepush {local_ref}：{result['kind']}")
        for reason in result["reasons"][:MAX_LIST]:
            say(f"  - {reason}")
        for path in result["deleted"][:10]:
            say(f"  - 删除文件 {path}")
    if failures:
        say("prepush: 拒绝非快进/棘轮回退的更新（要重写历史：ALLOW_NON_FF_PUSH=1；"
            "确属收紧：ALLOW_RATCHET_PUSH=1）")
        return 1
    return 0


def self_test() -> int:
    import tempfile

    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}: 期望 {expected!r}，实际 {actual!r}")

    def run(repo: pathlib.Path, *args: str, stdin: str | None = None,
            env: dict | None = None) -> tuple[int, str]:
        merged = {**os.environ}
        merged.pop("ALLOW_NON_FF_PUSH", None)
        merged.pop("ALLOW_RATCHET_PUSH", None)
        if env:
            merged.update(env)
        proc = subprocess.run(
            list(args),
            cwd=repo,
            input=None if stdin is None else stdin.encode("utf-8"),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=merged,
        )
        return proc.returncode, proc.stdout.decode("utf-8", "replace")

    def g(repo: pathlib.Path, *args: str) -> str:
        rc, out = git(repo, *args)
        if rc != 0:
            raise RuntimeError(f"git {' '.join(args)} 失败：{out[:200]}")
        return out.strip()

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp)
        g(repo, "init", "-q")
        g(repo, "config", "user.email", "update@example.invalid")
        g(repo, "config", "user.name", "update selftest")
        g(repo, "config", "commit.gpgsign", "false")
        (repo / "seed.txt").write_text("x\n", encoding="utf-8")
        g(repo, "add", "seed.txt")
        g(repo, "commit", "-q", "-m", "base")
        base = g(repo, "rev-parse", "HEAD")

        (repo / "a.txt").write_text("a\n", encoding="utf-8")
        g(repo, "add", "a.txt")
        g(repo, "commit", "-q", "-m", "add a")
        ff_new = g(repo, "rev-parse", "HEAD")
        ff = classify(repo, base, ff_new)
        check("快进判为 fast-forward", ff["kind"], "fast-forward")
        check("快进放行", ff["ok"], True)

        (repo / "b.txt").write_text("b\n", encoding="utf-8")
        g(repo, "add", "b.txt")
        g(repo, "commit", "-q", "-m", "add b")
        before_amend = g(repo, "rev-parse", "HEAD")
        g(repo, "commit", "--amend", "-q", "-m", "add b (amended)")
        after_amend = g(repo, "rev-parse", "HEAD")
        rewritten = classify(repo, before_amend, after_amend)
        check("amend 判为 clean-rewrite", rewritten["kind"], "clean-rewrite")
        check("clean-rewrite 放行", rewritten["ok"], True)
        check("amend 只有元数据变化（不误报补丁改写）", rewritten["range_diff"]["changed"], [])

        g(repo, "switch", "-q", "-c", "rd-up", base)
        body = "".join(f"{n}" + chr(10) for n in range(1, 21))
        (repo / "ctx.txt").write_text(body, encoding="utf-8")
        g(repo, "add", "ctx.txt")
        g(repo, "commit", "-q", "-m", "ctx base")
        rd_base = g(repo, "rev-parse", "HEAD")
        (repo / "ctx.txt").write_text(body.replace("1" + chr(10), "one" + chr(10), 1), encoding="utf-8")
        g(repo, "add", "ctx.txt")
        g(repo, "commit", "-q", "-m", "up changes line 1")
        g(repo, "switch", "-q", "-c", "rd-old", rd_base)
        (repo / "ctx.txt").write_text(body.replace("15" + chr(10), "fifteen" + chr(10), 1), encoding="utf-8")
        g(repo, "add", "ctx.txt")
        g(repo, "commit", "-q", "-m", "old changes line 15")
        rd_old = g(repo, "rev-parse", "HEAD")
        g(repo, "switch", "-q", "rd-up")
        g(repo, "cherry-pick", rd_old)
        rd_new = g(repo, "rev-parse", "HEAD")
        rd = classify(repo, rd_old, rd_new)
        check("补丁等价但落点不同：放行", rd["ok"], True)
        check("range-diff 点名落点变化", any("落点" in r for r in rd["reasons"]), True)

        g(repo, "switch", "-q", "-c", "neg", base)
        (repo / "lost.txt").write_text("lost\n", encoding="utf-8")
        g(repo, "add", "lost.txt")
        g(repo, "commit", "-q", "-m", "add lost")
        neg_old = g(repo, "rev-parse", "HEAD")
        (repo / "keep.txt").write_text("keep\n", encoding="utf-8")
        g(repo, "add", "keep.txt")
        g(repo, "commit", "-q", "-m", "add keep")
        neg_new = g(repo, "rev-parse", "HEAD")
        neg = classify(repo, neg_new, neg_old)
        check("丢提交判为 negative-update", neg["kind"], "negative-update")
        check("丢提交判红", neg["ok"], False)
        check("列出删除文件", "keep.txt" in neg["deleted"], True)
        check("--allow-rewrite 放行重写",
              classify(repo, neg_new, neg_old, allow_rewrite=True)["ok"], True)

        g(repo, "switch", "-q", "-c", "ratchet", base)
        scripts = repo / "scripts"
        scripts.mkdir(exist_ok=True)
        (scripts / "u.py").write_text("print('u')", encoding="utf-8")
        (scripts / "test_surface_baseline.json").write_text('{"script/u.py": 5}', encoding="utf-8")
        g(repo, "add", "scripts/u.py", "scripts/test_surface_baseline.json")
        g(repo, "commit", "-q", "-m", "baseline 5")
        ratchet_old = g(repo, "rev-parse", "HEAD")
        (scripts / "test_surface_baseline.json").write_text('{"script/u.py": 4}', encoding="utf-8")
        g(repo, "add", "scripts/test_surface_baseline.json")
        g(repo, "commit", "-q", "-m", "baseline 4")
        ratchet_new = g(repo, "rev-parse", "HEAD")
        shrink = classify(repo, ratchet_old, ratchet_new)
        check("测试面变小判红", shrink["ok"], False)
        check("点名缩小单位", any("u.py" in issue for issue in shrink["ratchet"]), True)
        check("--allow-ratchet 放行",
              classify(repo, ratchet_old, ratchet_new, allow_ratchet=True)["ok"], True)

        (repo / "x.rs").write_text("fn x() {}", encoding="utf-8")
        (scripts / "module_budget_baseline.json").write_text(
            '{"caps": [{"file": "x.rs", "lines": 10}]}\n', encoding="utf-8")
        g(repo, "add", "x.rs", "scripts/module_budget_baseline.json")
        g(repo, "commit", "-q", "-m", "cap 10")
        cap_old = g(repo, "rev-parse", "HEAD")
        (scripts / "module_budget_baseline.json").write_text(
            '{"caps": [{"file": "x.rs", "lines": 12}]}\n', encoding="utf-8")
        g(repo, "add", "scripts/module_budget_baseline.json")
        g(repo, "commit", "-q", "-m", "cap 12")
        cap_new = g(repo, "rev-parse", "HEAD")
        cap = classify(repo, cap_old, cap_new)
        check("模块预算放松判红", cap["ok"], False)
        check("点名放松文件", any("x.rs" in issue for issue in cap["ratchet"]), True)

        g(repo, "switch", "-q", "-c", "removed", base)
        scripts.mkdir(exist_ok=True)
        (scripts / "gone.py").write_text("print('gone')", encoding="utf-8")
        (scripts / "test_surface_baseline.json").write_text('{"script/gone.py": 1}', encoding="utf-8")
        g(repo, "add", "scripts/gone.py", "scripts/test_surface_baseline.json")
        g(repo, "commit", "-q", "-m", "add gone")
        rem_old = g(repo, "rev-parse", "HEAD")
        (scripts / "gone.py").unlink()
        (scripts / "test_surface_baseline.json").write_text("{}", encoding="utf-8")
        g(repo, "add", "-A")
        g(repo, "commit", "-q", "-m", "remove gone")
        rem_new = g(repo, "rev-parse", "HEAD")
        rem = classify(repo, rem_old, rem_new)
        check("单位随文件删除不算回退", rem["ok"], True)

        g(repo, "switch", "-q", "-c", "docs", base)
        scripts.mkdir(exist_ok=True)
        (scripts / "docs_hygiene_baseline.json").write_text(
            '{"changelog": 100, "drive_paths": {"a.md": 1}}\n', encoding="utf-8")
        g(repo, "add", "scripts/docs_hygiene_baseline.json")
        g(repo, "commit", "-q", "-m", "docs 100")
        docs_old = g(repo, "rev-parse", "HEAD")
        (scripts / "docs_hygiene_baseline.json").write_text(
            '{"changelog": 90, "drive_paths": {"a.md": 2}}\n', encoding="utf-8")
        g(repo, "add", "scripts/docs_hygiene_baseline.json")
        g(repo, "commit", "-q", "-m", "docs 90")
        docs_new = g(repo, "rev-parse", "HEAD")
        docs = classify(repo, docs_old, docs_new)
        check("CHANGELOG 变小/盘符路径变多判红", docs["ok"], False)
        check("点名字节缩小", any("CHANGELOG" in issue for issue in docs["ratchet"]), True)
        check("点名盘符路径", any("盘符" in issue for issue in docs["ratchet"]), True)

        missing = classify(repo, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", ff_new)
        check("旧提交不在本仓判红", missing["ok"], False)
        check("旧提交缺失分类", missing["kind"], "unknown-old")

        def prepush(rows: str, env: dict | None = None) -> int:
            return run(
                repo, sys.executable, str(pathlib.Path(__file__)), "prepush",
                "--repo", str(repo), stdin=rows, env=env,
            )[0]

        check("prepush 新分支放行",
              prepush(f"refs/heads/x {ff_new} refs/heads/x {ZERO}\n"), 0)
        check("prepush 快进放行",
              prepush(f"refs/heads/x {ff_new} refs/heads/x {base}\n"), 0)
        check("prepush 删除分支放行",
              prepush(f"(delete) {ZERO} refs/heads/x {base}\n"), 0)
        neg_line = f"refs/heads/x {neg_old} refs/heads/x {neg_new}\n"
        check("prepush 负更新拦", prepush(neg_line), 1)
        check("prepush 允许重写放行", prepush(neg_line, {"ALLOW_NON_FF_PUSH": "1"}), 0)

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
    root = ROOT
    if "--repo" in argv:
        root = pathlib.Path(argv[argv.index("--repo") + 1]).resolve()
    command = argv[0] if argv else ""
    if command == "check":
        def value(flag: str) -> str:
            return argv[argv.index(flag) + 1] if flag in argv else ""
        old, new = value("--old"), value("--new")
        if not old or not new:
            say("check 需要 --old <rev> 与 --new <rev>")
            return 2
        return run_checked(
            root, old, new,
            allow_rewrite="--allow-rewrite" in argv,
            allow_ratchet="--allow-ratchet" in argv,
            as_json="--json" in argv,
        )
    if command == "prepush":
        return prepush(root, argv[1:])
    say(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
