#!/usr/bin/env python3
"""合并取证：冲突解完，另一边有没有被整块丢掉。

用法：
  python scripts/merge_forensics.py <合并提交> [--repo .] [--json]
  python scripts/merge_forensics.py --recent 5 [--repo .]
  python scripts/merge_forensics.py --self-test

钉的是这个事故（社区高频、且**不会有任何门变红**）：
  解冲突时 `git checkout --ours <file>`（或 rebase 里 ours/theirs 反了）整文件取一边，
  另一边的改动就**静默消失**——编译照过、测试照绿（对面的测试也一起没了），
  几天后才发现"我明明合进去了啊"。

判据（分两档，绝不靠猜）：
  拦：某文件两边都改过，合并结果却与其中一边**逐字节相同** ⇒ 另一边整块没进；
  拦：合并结果里文件直接没了，而两边都有它；
  警：个别新增行找不到（可能被改写或挪位，要人看一眼）；
  拦：算不出 merge-base（shallow/grafted 克隆）或取不到父提交 ⇒ 无法取证就是红。
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

ADDED_LINE = re.compile(r"^\+(?!\+\+)")


def run(argv: list[str], root: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", "-C", str(root), *argv], stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")


def git(argv: list[str], root: pathlib.Path) -> str:
    """成功才给 stdout（可能是空串）。失败必须抛——"命令没跑成"和"结果是空"要能分开。"""
    result = run(argv, root)
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(argv)} 失败：{result.stdout.strip()[:200]}")
    return result.stdout


def parents(sha: str, root: pathlib.Path) -> list[str]:
    return git(["rev-list", "--parents", "-n", "1", sha], root).split()[1:]


def blob_of(sha: str, path: str, root: pathlib.Path) -> str:
    """文件在该提交里的 blob id；不存在给空串（不抛，让调用方区分"没了"和"没改"）。"""
    if run(["cat-file", "-e", f"{sha}:{path}"], root).returncode != 0:
        return ""
    return git(["rev-parse", f"{sha}:{path}"], root).strip()


def changed_files(base: str, side: str, root: pathlib.Path) -> set[str]:
    return {line for line in git(["diff", "--name-only", base, side], root).splitlines() if line.strip()}


def added_lines(base: str, side: str, path: str, root: pathlib.Path) -> list[str]:
    """side 相对 base 新增的行（去掉 +++ 头与空行）。"""
    diff = git(["diff", "-U0", "--no-color", base, side, "--", path], root)
    lines = []
    for line in diff.splitlines():
        if ADDED_LINE.match(line) and line[1:].strip():
            lines.append(line[1:].strip())
    return lines


def text_of(sha: str, path: str, root: pathlib.Path) -> str:
    try:
        return git(["show", f"{sha}:{path}"], root)
    except RuntimeError:
        return ""


def analyze(merge_sha: str, root: pathlib.Path) -> dict:
    root = pathlib.Path(root)
    base = {"tool": "merge-forensics", "sha": merge_sha, "findings": [], "red": False,
            "blocking": 0, "warnings": 0}
    try:
        pars = parents(merge_sha, root)
    except RuntimeError as exc:
        return {**base, "findings": [{"kind": "取不到父提交", "detail": str(exc), "blocking": True}],
                "red": True, "blocking": 1}
    if len(pars) < 2:
        base["parents"] = pars
        base["note"] = "只有一个父（squash/rebase 落地）——没有「冲突解法」可取证，跳过而不是放行"
        return base
    left, right = pars[0], pars[1]
    merge_base = run(["merge-base", left, right], root)
    if merge_base.returncode != 0 or not merge_base.stdout.strip():
        return {**base, "parents": pars,
                "findings": [{"kind": "算不出 merge-base",
                              "detail": "取不到共同祖先（shallow/grafted 克隆？），取证不成立就判红："
                                        + merge_base.stdout.strip()[:160],
                              "blocking": True}],
                "red": True, "blocking": 1}
    base = {**base, "parents": pars, "merge_base": merge_base.stdout.strip()}
    changed_left = changed_files(base["merge_base"], left, root)
    changed_right = changed_files(base["merge_base"], right, root)
    overlap = sorted(changed_left & changed_right)
    findings: list[dict] = []
    generated_skipped = 0
    for path in overlap:
        left_blob, right_blob = blob_of(left, path, root), blob_of(right, path, root)
        merged_blob = blob_of(merge_sha, path, root)
        if merged_blob == "":
            findings.append({"kind": "合并结果里文件没了", "file": path,
                             "detail": "两边都改过这个文件，合并后它不存在——确认是有意删除"})
            continue
        if merged_blob == left_blob and right_blob != left_blob:
            findings.append({"kind": "整块取了 ours", "file": path,
                             "detail": f"结果与 {left[:12]} 逐字节相同 ⇒ 另一边 {right[:12]} 对这个文件的改动"
                                       "一条没进（`checkout --ours`，或 rebase 里 ours/theirs 反了）"})
            continue
        if merged_blob == right_blob and right_blob != left_blob:
            findings.append({"kind": "整块取了 theirs", "file": path,
                             "detail": f"结果与 {right[:12]} 逐字节相同 ⇒ 另一边 {left[:12]} 对这个文件的改动"
                                       "一条没进"})
            continue
        text = text_of(merge_sha, path, root)
        if _is_generated(path):
            # 锁文件/生成物只比"整块有没有丢"，不逐行——逐行永远是噪音，
            # 但数量要报出来，不能变成没人知道的豁免面。
            generated_skipped += 1
            continue
        for side in (left, right):
            for line in added_lines(base["merge_base"], side, path, root):
                if line not in text:
                    findings.append({"kind": "新增行没落到结果里", "file": path, "blocking": False,
                                     "detail": f"{side[:12]} 加的 `{line[:70]}` 在合并结果里找不到"
                                               "（可能被改写或挪位；整边都找不到就是丢工）"})
    findings = [{**finding, "blocking": finding.get("blocking", True)} for finding in findings]
    return {**base, "overlap_files": len(overlap), "findings": findings,
            "generated_files": generated_skipped,
            "blocking": sum(1 for f in findings if f["blocking"]),
            "warnings": sum(1 for f in findings if not f["blocking"]),
            "red": any(finding["blocking"] for finding in findings)}


def _is_generated(path: str) -> bool:
    name = pathlib.PurePosixPath(path).name
    return name in {"Cargo.lock", "package-lock.json", "yarn.lock", "poetry.lock", "uv.lock"} \
        or path.endswith((".snap", ".golden"))


def recent(count: int, root: pathlib.Path) -> list[str]:
    out = git(["log", "--merges", "-n", str(count), "--format=%H"], root)
    return [line.strip() for line in out.splitlines() if line.strip()]


def render(result: dict) -> None:
    """打印单独成函数：自测要用假控制台钉住它（cp936 放不下 ⇒ 这类字符）。"""
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    lines = [f"{result['sha'][:12]}：两边都改过 {result.get('overlap_files', 0)} 个文件"
             f"（生成物只比整块 {result.get('generated_files', 0)} 个）｜"
             f"拦 {result.get('blocking', 0)} 条｜警告 {result.get('warnings', 0)} 条"]
    if result.get("note"):
        lines.append(f"  备注：{result['note']}")
    for finding in result["findings"]:
        mark = "拦" if finding["blocking"] else "警"
        lines.append(f"  [{mark}|{finding['kind']}] {finding.get('file', '')} {finding['detail']}")
    lines.append("结论：" + ("红（有一边的改动没落地）" if result["red"]
                           else "绿（两边的改动都能在结果里找到）"))
    for line in lines:
        try:
            print(line)
        except UnicodeEncodeError:
            print(line.encode(encoding, "replace").decode(encoding, "replace"))


def parse(argv: list[str]) -> tuple[list[str], pathlib.Path, bool]:
    root, targets = pathlib.Path("."), []
    as_json = "--json" in argv
    asked_recent = False
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--repo" and index + 1 < len(argv):
            root = pathlib.Path(argv[index + 1])
            index += 2
            continue
        if arg == "--recent" and index + 1 < len(argv):
            if not re.fullmatch(r"\d+", argv[index + 1] or ""):
                raise SystemExit("--recent 需要一个数字")
            asked_recent = True
            targets += recent(int(argv[index + 1]), root)
            index += 2
            continue
        if not arg.startswith("--"):
            targets.append(arg)
        index += 1
    if asked_recent and not targets:
        # 找不到合并提交要大声说：静默打印用法等于"没查"
        print("--recent：这个历史里没有合并提交（全 squash 落地的仓就是这样）")
    return targets, root, as_json


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    targets, root, as_json = parse(argv)
    if not targets:
        print(__doc__)
        return 0
    worst = 0
    for sha in targets:
        result = analyze(sha, root)
        if as_json:
            print(json.dumps(result, ensure_ascii=False, indent=1))
        else:
            render(result)
        worst = 1 if result["red"] else worst
    return worst


def self_test() -> int:
    """每条判红都要有真的会红的反证；绿例必须真跑。"""
    import io
    import tempfile

    total, failures = 0, []

    def check(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    def make_merge(root: pathlib.Path, resolution: str, with_lock: bool = False) -> str:
        """base 三行；停在 left 分支上把 right 合进来 ⇒ P1=left=git 口径的 ours。

        分支故意不叫 ours/theirs：`git merge X` 时 X 是 theirs，用名字当夹具会把
        两边的标签记反（第一版就在这里判成了"没重叠文件"）。
        """
        root.mkdir(parents=True, exist_ok=True)
        for argv in (["init", "-q", "-b", "main"], ["config", "user.email", "t@example.com"],
                     ["config", "user.name", "t"]):
            _git_or_die(root, *argv)
        (root / "a.txt").write_text("one\ntwo\nthree\n", encoding="utf-8")
        if with_lock:
            (root / "Cargo.lock").write_text("ver = 1\n", encoding="utf-8")
        _git_or_die(root, "add", "-A")
        _git_or_die(root, "commit", "-qm", "base")
        _git_or_die(root, "checkout", "-qb", "left")
        (root / "a.txt").write_text("one\nLEFT-2\nthree\n", encoding="utf-8")
        if with_lock:
            (root / "Cargo.lock").write_text("ver = 2\n", encoding="utf-8")
        _git_or_die(root, "commit", "-qam", "left 改第二行")
        _git_or_die(root, "checkout", "-qb", "right", "main")
        (root / "a.txt").write_text("one\nRIGHT-2\nthree\n", encoding="utf-8")
        if with_lock:
            (root / "Cargo.lock").write_text("ver = 3\n", encoding="utf-8")
        _git_or_die(root, "commit", "-qam", "right 改第二行")
        _git_or_die(root, "checkout", "-q", "left")
        merged = subprocess.run(["git", "merge", "--no-commit", "--no-ff", "right"], cwd=root,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                encoding="utf-8", errors="replace")
        assert merged.returncode != 0, "夹具应当冲突：" + merged.stdout
        (root / "a.txt").write_text(resolution, encoding="utf-8")
        if with_lock:
            (root / "Cargo.lock").write_text("ver = 2\nver = 3\n", encoding="utf-8")
        _git_or_die(root, "add", "-A")
        _git_or_die(root, "commit", "-qm", "merge")
        return _git_or_die(root, "rev-parse", "HEAD").strip()

    with tempfile.TemporaryDirectory() as tmp:
        box = pathlib.Path(tmp)

        # 正例：两边都保住 ⇒ 绿，且真的认到了重叠文件（不是"什么都没扫到"的绿）
        both = box / "both"
        sha = make_merge(both, "one\nLEFT-2\nRIGHT-2\nthree\n")
        result = analyze(sha, both)
        check("两边都落地的合并判绿", result["red"], False)
        check("正例真扫到了重叠文件", result.get("overlap_files"), 1)

        # 反证 1：整块取 ours（结果与 P1 逐字节相同 ⇒ P2 的改动一条没进）
        only_ours = box / "ours"
        sha = make_merge(only_ours, "one\nLEFT-2\nthree\n")
        result = analyze(sha, only_ours)
        check("整块取一边被拦", result["red"], True)
        check("点出取的是哪一边", any(f["kind"] == "整块取了 ours" for f in result["findings"]), True)

        # 反证 2：整块取 theirs（rebase 里方向正好相反，所以两边都要钉）
        only_theirs = box / "theirs"
        sha = make_merge(only_theirs, "one\nRIGHT-2\nthree\n")
        check("反向整块取也被拦",
              any(f["kind"] == "整块取了 theirs" for f in analyze(sha, only_theirs)["findings"]), True)

        # 反证 3：单边改动没有冲突可言 ⇒ 不牵连
        clean = box / "clean"
        make_merge(clean, "one\nLEFT-2\nRIGHT-2\nthree\n")
        _git_or_die(clean, "checkout", "-q", "main")
        (clean / "c.txt").write_text("only main\n", encoding="utf-8")
        _git_or_die(clean, "add", "c.txt")
        _git_or_die(clean, "commit", "-qm", "main 自己加文件")
        _git_or_die(clean, "merge", "--no-ff", "-qm", "无重叠", "right")
        sha = _git_or_die(clean, "rev-parse", "HEAD").strip()
        check("没有重叠改动的合并不判红", analyze(sha, clean)["red"], False)

        # 反证 4：行被改写只警告，不拦（RIGHT-2 变成了 R2， substring 也认不出）
        reword = box / "reword"
        reword_sha = make_merge(reword, "one\nLEFT-2\nR2\nthree\n")
        result = analyze(reword_sha, reword)
        check("改写行被抓出来当警告",
              any(f["kind"] == "新增行没落到结果里" for f in result["findings"]), True)
        check("改写行不判红", result["red"], False)

        # 反证 5：结果里文件没了
        vanished = box / "vanish"
        make_merge(vanished, "one\nLEFT-2\nthree\n")
        (vanished / "a.txt").unlink()
        _git_or_die(vanished, "add", "-A", "a.txt")
        _git_or_die(vanished, "commit", "--amend", "-qm", "merge 把文件删了")
        # amend 会换掉 commit id，必须重取——拿旧 id 取证等于查了一个不存在的提交
        vanished_sha = _git_or_die(vanished, "rev-parse", "HEAD").strip()
        check("文件整块消失被拦",
              any(f["kind"] == "合并结果里文件没了" for f in analyze(vanished_sha, vanished)["findings"]), True)

        # 反证 6：单父提交说清楚"跳过"，不许算成通过
        plain = box / "plain"
        make_merge(plain, "one\nLEFT-2\nRIGHT-2\nthree\n")
        _git_or_die(plain, "checkout", "-q", "main")
        (plain / "d.txt").write_text("d\n", encoding="utf-8")
        _git_or_die(plain, "add", "d.txt")
        _git_or_die(plain, "commit", "-qm", "单亲")
        result = analyze(_git_or_die(plain, "rev-parse", "HEAD").strip(), plain)
        check("单父提交不判红但要说明", (result["red"], bool(result.get("note"))), (False, True))

        # 反证 7：取不到的 sha ⇒ 红（不是崩溃、不是静默绿）
        result = analyze("deadbeef" * 5, both)
        check("坏 sha 判红", result["red"], True)

        # 反证 8：打印不许被控制台编码崩掉
        arrow = analyze(reword_sha, reword)
        arrow["findings"].append({"kind": "夹具", "file": "", "blocking": True,
                                  "detail": "带 ⇒ 与 \U0001f642 的判决"})
        probe = io.TextIOWrapper(io.BytesIO(), encoding="cp936", errors="strict", newline="\n")
        saved, sys.stdout = sys.stdout, probe
        try:
            render(arrow)
            rc = 0
        except UnicodeEncodeError:
            rc = -1
        finally:
            sys.stdout = saved
        check("cp936 控制台下打印判决不崩", rc, 0)

        # 反证 9：生成物豁免面必须自己可查（不许悄悄把某类文件全放过）
        check("Cargo.lock 算生成物", _is_generated("Cargo.lock"), True)
        check("源码不算生成物", _is_generated("crates/trit-core/src/lib.rs"), False)
        locked = box / "locked"
        locked_sha = make_merge(locked, "one\nLEFT-2\nRIGHT-2\nthree\n", with_lock=True)
        result = analyze(locked_sha, locked)
        check("重叠文件含生成物", result.get("overlap_files"), 2)
        check("生成物计数要报出来", result.get("generated_files"), 1)
        check("生成物不产逐行噪音",
              any(f.get("file") == "Cargo.lock" and f["kind"] == "新增行没落到结果里"
                  for f in result["findings"]), False)
        check("生成物整块丢了照样拦",
              any(f["kind"].startswith("整块取了") and f.get("file") == "a.txt"
                  for f in analyze(make_merge(box / "locked2", "one\nLEFT-2\nthree\n",
                                              with_lock=True), box / "locked2")["findings"]), True)

        # 反证 10：两个毫无共同祖先的分支合并 ⇒ 取证不成立必须判红，
        # 不许把"算不出 base"折成"没有重叠文件"的绿（shallow/grafted 克隆就是这个形状）。
        orphan = box / "orphan"
        orphan.mkdir()
        _git_or_die(orphan, "init", "-q", "-b", "main")
        _git_or_die(orphan, "config", "user.email", "t@example.com")
        _git_or_die(orphan, "config", "user.name", "t")
        (orphan / "a.txt").write_text("a\n", encoding="utf-8")
        _git_or_die(orphan, "add", "a.txt")
        _git_or_die(orphan, "commit", "-qm", "root A")
        _git_or_die(orphan, "checkout", "-q", "--orphan", "side")
        _git_or_die(orphan, "rm", "-q", "--cached", "a.txt")
        (orphan / "a.txt").unlink()  # 留着当未跟踪文件会挡住后面切回 main
        (orphan / "b.txt").write_text("b\n", encoding="utf-8")
        _git_or_die(orphan, "add", "b.txt")
        _git_or_die(orphan, "commit", "-qm", "root B")
        _git_or_die(orphan, "checkout", "-q", "main")
        _git_or_die(orphan, "merge", "--allow-unrelated-histories", "-qm", "无共同祖先", "side")
        result = analyze(_git_or_die(orphan, "rev-parse", "HEAD").strip(), orphan)
        check("算不出 merge-base 判红", result["red"], True)
        check("判红的理由是 merge-base",
              any(f["kind"] == "算不出 merge-base" for f in result["findings"]), True)

    print(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


def _git_or_die(root: pathlib.Path, *argv: str) -> str:
    result = subprocess.run(["git", *argv], cwd=root, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")
    if result.returncode != 0:
        raise AssertionError(f"git {' '.join(argv)} 失败：{result.stdout}")
    return result.stdout


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
