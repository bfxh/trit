#!/usr/bin/env python3
"""`oops`：找回被 reset --hard / branch -D / stash drop 丢掉的活，而且不动 HEAD。

次序固定成三步：**探明 → 落快照 → 从 sha 开新分支**。不 checkout、不 reset、不 force、不删东西。

取证渠道（任何一路跑不动都必须出声，不能当成"没有"）：
  - reflog：`git log -g --all` 能看到被顶掉的提交和动作名；
  - `git fsck --no-reflogs --dangling`：`--no-reflogs` 是关键，否则 reflog 还指着的东西算"可达"，
    branch -D 之后的孤儿一条也列不出来；
  - `git stash list`：还在的 stash（被 drop 的会以悬挂提交形态出现在 fsck 里）。

用法：
  python scripts/oops.py search [--grep 词] [--author 名] [--since 2026-11-01] [--limit 40]
  python scripts/oops.py show <sha>
  python scripts/oops.py restore <sha> [--branch 名字]
  python scripts/oops.py --self-test
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
FSCK_DANGLING = re.compile(r"^dangling commit ([0-9a-f]{7,40})$")
SHA_SHAPE = re.compile(r"^[0-9a-fA-F]{4,40}$")
STASH_MARKS = ("WIP on", "On ")
KNOWN_COMMANDS = ("search", "show", "restore")
TAB = "%x09"


def _say(text: str) -> None:
    """中文 Windows（cp936）控制台上打印带 ⇒ 的判定会炸——炸了就等于门没跑。"""
    try:
        print(text)
    except UnicodeEncodeError:
        sys.stdout.buffer.write((text + "\n").encode("utf-8", "replace"))


def git(root: pathlib.Path, args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=str(root), stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True,
                          encoding="utf-8", errors="replace")


def works(root: pathlib.Path) -> bool:
    return git(root, ["rev-parse", "--git-dir"]).returncode == 0


def exists(root: pathlib.Path, sha: str) -> bool:
    """对象还在不在——被 gc 收走就直说，别给人假希望。"""
    if not sha or not SHA_SHAPE.match(sha):
        return False
    return git(root, ["cat-file", "-e", sha + "^{commit}"]).returncode == 0


def commit_subject(root: pathlib.Path, sha: str) -> str:
    result = git(root, ["log", "-1", "--format=%s", sha])
    return result.stdout.strip() if result.returncode == 0 else ""


def commit_date(root: pathlib.Path, sha: str) -> str:
    result = git(root, ["log", "-1", "--date=iso-strict", "--format=%ad", sha])
    return result.stdout.strip() if result.returncode == 0 else ""


def author_of(root: pathlib.Path, sha: str) -> str:
    result = git(root, ["log", "-1", "--format=%an", sha])
    return result.stdout.strip() if result.returncode == 0 else ""


def reachable_set(root: pathlib.Path) -> tuple[set[str], str]:
    """一次 `git rev-list --all` 拿到「还挂在任何 ref 上」的提交集合。

    原来逐条候选跑 `git branch --all --contains`：几百条 reflog = 几百次进程起停，真仓上要几十秒
    （今天实测到的）。改成一次遍历 + 集合判断，口径不变（在任何 ref 上 = 没丢）。
    """
    result = git(root, ["rev-list", "--branches", "--tags", "--remotes"])
    if result.returncode != 0:
        return set(), "rev-list --branches --tags --remotes 跑不动 ⇒ 判不了「什么还挂在 ref 上」，不能当成没丢"
    return {line.strip() for line in result.stdout.splitlines() if line.strip()}, ""


def on_any_ref(root: pathlib.Path, sha: str) -> list[str]:
    result = git(root, ["branch", "--all", "--contains", sha])
    if result.returncode != 0:
        return []
    return [line.strip().lstrip("*").strip() for line in result.stdout.splitlines()
            if line.strip()]


def reflog_items(root: pathlib.Path) -> tuple[list[dict], str]:
    result = git(root, ["log", "-g", "--all", "--date=iso-strict",
                        "--format=%H" + TAB + "%gD" + TAB + "%gs" + TAB + "%s" + TAB + "%ad"])
    if result.returncode != 0:
        return [], "reflog 这一路跑不动：" + result.stdout.strip()[:120]
    out: list[dict] = []
    for line in result.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 5:
            continue
        # subject/when 直接取同一趟调用里的 %s/%ad（提交自身的），不再逐条起 git 进程；
        # %gs 是"动作"文本（`checkout: moving from A to B`），当标题会让 --grep/--since 认不出来
        out.append({"sha": parts[0], "where": parts[1], "action": parts[2],
                    "subject": parts[3], "when": parts[4], "source": "reflog"})
    return out, ""


def fsck_items(root: pathlib.Path) -> tuple[list[dict], str]:
    result = git(root, ["fsck", "--full", "--no-reflogs", "--dangling", "--no-progress"])
    if result.returncode != 0:
        return [], "fsck 这一路跑不动：" + result.stdout.strip()[:120]
    out: list[dict] = []
    for line in result.stdout.splitlines():
        match = FSCK_DANGLING.match(line.strip())
        if not match:
            continue
        sha = match.group(1)
        out.append({"sha": sha, "where": "fsck", "when": commit_date(root, sha),
                    "subject": commit_subject(root, sha), "source": "dangling"})
    return out, ""


def stash_items(root: pathlib.Path) -> tuple[list[dict], str]:
    result = git(root, ["stash", "list", "--format=%H" + TAB + "%gd" + TAB + "%s"])
    if result.returncode != 0:
        return [], "stash 这一路跑不动：" + result.stdout.strip()[:120]
    out: list[dict] = []
    for line in result.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 3:
            continue
        out.append({"sha": parts[0], "where": parts[1], "action": "stash: " + parts[2],
                    "when": commit_date(root, parts[0]),
                    "subject": commit_subject(root, parts[0]), "source": "stash"})
    return out, ""


def gather(root: pathlib.Path, quick: bool = False) -> tuple[list[dict], list[str]]:
    """三路合流、按 sha 去重（一个提交可以同时出现在 reflog 与 fsck 里）。

    `quick=True` 跳过 fsck（大仓上 `git fsck --full` 要几分钟）——但这条必须**出声**，
    因为 branch -D 之后的孤儿主要就是靠 fsck 认出来的，跳过不等于查过。
    """
    problems: list[str] = []
    merged: dict[str, dict] = {}
    collectors = [reflog_items, stash_items] if quick else [reflog_items, fsck_items, stash_items]
    if quick:
        problems.append("quick 模式跳过了 fsck ⇒ branch -D / drop 掉的 stash 这一轮查不到（不是没有，是没查）")
    for collect in collectors:
        items, problem = collect(root)
        if problem:
            problems.append(problem + "（这一路没查到，不能当成没有）")
        for item in items:
            entry = merged.setdefault(item["sha"], {**item, "sources": [item["source"]]})
            if item["source"] not in entry["sources"]:
                entry["sources"].append(item["source"])
            for field in ("subject", "when"):
                if not entry.get(field) and item.get(field):
                    entry[field] = item[field]
    return list(merged.values()), problems


def lost(root: pathlib.Path, limit: int = 40, quick: bool = False) -> tuple[list[dict], list[str], dict]:
    """"丢掉的活" = 对象还在库里，但不在任何分支上。"""
    if not works(root):
        return [], ["不在 git 仓里 ⇒ 什么都没查（这条必须响，不能静默给空表）"], {
            "scanned": 0, "lost": 0, "gc_gone": 0}
    candidates, problems = gather(root, quick)
    reachable, reach_problem = reachable_set(root)
    if reach_problem:
        # 判不了"还挂在 ref 上"就不能断言"丢了"——只报问题，不端出一堆假候选
        return [], problems + [reach_problem], {"scanned": len(candidates), "lost": 0, "gc_gone": 0}
    out: list[dict] = []
    gone = 0
    for item in candidates:
        if not exists(root, item["sha"]):
            gone += 1
            continue
        if item["sha"] in reachable:
            continue
        out.append({**item, "refs": []})
    out.sort(key=lambda entry: (entry.get("when") or "", entry["sha"]), reverse=True)
    stats = {"scanned": len(candidates), "lost": len(out), "gc_gone": gone}
    return out[:limit], problems, stats


def matches(root: pathlib.Path, item: dict, pattern: str, since: str, author: str) -> bool:
    if pattern and pattern.lower() not in (item.get("subject") or "").lower():
        return False
    if since:
        when = item.get("when") or ""
        if not when or when[:10] < since:
            return False
    if author and author.lower() not in author_of(root, item["sha"]).lower():
        return False
    return True


def search(root: pathlib.Path, pattern: str = "", since: str = "", author: str = "",
           limit: int = 40, quick: bool = False) -> tuple[list[dict], list[str], dict]:
    items, problems, stats = lost(root, limit, quick)
    kept = [item for item in items if matches(root, item, pattern, since, author)]
    return kept, problems, {**stats, "shown": len(kept)}


def free_branch_name(root: pathlib.Path, wanted: str) -> str:
    """已存在就加序号：覆盖别人（或上一次的恢复结果）这条路根本不存在。"""
    name = wanted
    checked = git(root, ["show-ref", "--verify", "--quiet", "refs/heads/" + name])
    suffix = 0
    while checked.returncode == 0:
        suffix += 1
        name = f"{wanted}-{suffix}"
        checked = git(root, ["show-ref", "--verify", "--quiet", "refs/heads/" + name])
    return name


def restore(root: pathlib.Path, sha: str, branch: str | None = None) -> int:
    """从 sha 开一条新分支：先落快照，再建分支；**不动 HEAD、不删东西、不 force**。"""
    if not works(root):
        _say("不在 git 仓里 ⇒ 拒绝执行（不是没找到，是没地方找）")
        return 1
    if not exists(root, sha):
        _say("对象 " + sha + " 不在库里（多半被 gc 收走）⇒ 不可恢复；这里不动任何 ref")
        return 1
    full = git(root, ["rev-parse", sha + "^{commit}"]).stdout.strip()
    if not full:
        _say(sha + " 解不出提交号 ⇒ 拒绝执行")
        return 1
    head = git(root, ["rev-parse", "--short", "HEAD"]).stdout.strip()
    stamp = time.strftime("%Y%m%d-%H%M%S")
    wanted = branch or ("recovered/" + full[:7] + "-" + stamp)
    name = free_branch_name(root, wanted)
    snap = subprocess.run([sys.executable, str(root / "scripts" / "snapshot.py"), "save",
                           "--label", "before-oops-restore-" + stamp],
                          cwd=str(root), stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, encoding="utf-8", errors="replace")
    if snap.returncode != 0:
        _say("第 1 步（快照）失败，rc=" + str(snap.returncode) + " ⇒ 拒绝建分支；先解决快照再恢复")
        for line in (snap.stdout or "").splitlines()[:6]:
            _say("    " + line)
        return 1
    _say("第 1 步：已存快照 before-oops-restore-" + stamp + "（当前 HEAD " + head + "）")
    made = git(root, ["branch", name, full])
    if made.returncode != 0:
        _say("第 2 步（建分支）失败：" + made.stdout.strip() + " ⇒ 快照仍在，HEAD 未动")
        return 1
    _say("第 2 步：已建分支 " + name + " 指向 " + full[:10] + "；HEAD 仍在 " + head + "，工作树没动")
    _say("接下来自己定：git checkout " + name + " 看内容，或 git cherry-pick " + full[:10] + " 只取那一个提交")
    _say("回滚这次恢复：git branch -D " + name +
         "；要连工作树一起退：python scripts/snapshot.py restore --label before-oops-restore-" + stamp)
    return 0
def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    # 先把选项连取值一起剥掉并校验：以前 `--root` 的取值是在这里直接读的，缺取值就 IndexError 崩掉；
    # 而且 `--root X show <sha>` 会因为 argv[0] 是选项而退化成 search——真仓第一次跑就撞上，两个坑一起修
    options: dict[str, str] = {}
    positional = list(argv)
    for flag in ("--root", "--limit", "--grep", "--since", "--author", "--branch"):
        while flag in positional:
            at = positional.index(flag)
            if at + 1 >= len(positional):
                _say(flag + " 后面少了取值 ⇒ 判红（不猜它指哪儿）")
                return 1
            options[flag] = positional[at + 1]
            del positional[at:at + 2]
    root = pathlib.Path(options["--root"]) if "--root" in options else ROOT
    limit = int(options.get("--limit", "40"))
    if positional and positional[0] not in KNOWN_COMMANDS:
        _say("不认识的子命令 " + positional[0] + "（可用：search / show / restore）⇒ 判红，不猜")
        return 1
    command = positional[0] if positional else "search"
    if command == "search":
        pattern = options.get("--grep", "")
        since = options.get("--since", "")
        author = options.get("--author", "")
        kept, problems, stats = search(root, pattern, since, author, limit,
                                       "--quick" in argv)
        for problem in problems:
            _say("  [拦] " + problem)
        _say("扫过 " + str(stats["scanned"]) + " 个候选｜丢掉且还能救 " + str(stats["lost"]) +
             " 条｜本次显示 " + str(stats["shown"]) + " 条｜已被 gc 收走 " + str(stats["gc_gone"]) + " 条")
        for item in kept:
            _say("  " + item["sha"][:10] + "  " + (item.get("when") or "-") + "  [" +
                 ",".join(item["sources"]) + "]  " + (item.get("subject") or "")[:60])
        if problems:
            _say("结论：红（有渠道没查到，不能当成没有）")
            return 1
        if not kept and pattern:
            _say("没有匹配 " + pattern + " 的可恢复提交（不代表没丢过东西，换个词或去掉 --grep）")
        _say("恢复某一条：python scripts/oops.py restore <sha>（先快照、再建分支、不动 HEAD）")
        return 0
    if command == "show":
        sha = positional[1] if len(positional) > 1 else ""
        if not exists(root, sha):
            _say((sha or "(空)") + " 解不出提交 ⇒ 判红；不做任何猜测性恢复")
            return 1
        full = git(root, ["rev-parse", sha + "^{commit}"]).stdout.strip()
        _say(git(root, ["show", "--stat", "--format=%H%n%an %ad%n%s", full]).stdout)
        refs = on_any_ref(root, full)
        _say("还在这些分支上：" + (", ".join(refs) if refs else "没有 ⇒ 这就是丢了的那份"))
        _say("恢复：python scripts/oops.py restore " + full[:10])
        return 0
    if command == "restore":
        if len(positional) < 2:
            _say("restore 需要一个 sha（不给 sha 的「恢复」等于随便挑一个）")
            return 1
        return restore(root, positional[1], options.get("--branch"))
    _say("不认识的子命令 " + command + "（可用：search / show / restore）⇒ 判红，不猜")
    return 1


def self_test() -> int:
    import io
    import tempfile

    passed, failed = 0, 0

    def check(label: str, got: object, want: object) -> None:
        nonlocal passed, failed
        if got == want:
            passed += 1
        else:
            failed += 1
            print("  FAIL " + label + ": 期望 " + repr(want) + "，实得 " + repr(got))

    def init_repo(root: pathlib.Path) -> None:
        for args in (["init", "-q"], ["config", "user.email", "t@example.invalid"],
                     ["config", "user.name", "tester"], ["config", "advice.detachedHead", "false"]):
            made = git(root, args)
            if made.returncode != 0:
                raise SystemExit("夹具建仓失败：" + " ".join(args) + " -> " + made.stdout)

    def commit_file(root: pathlib.Path, name: str, message: str) -> str:
        (root / name).write_text(message + "\n", encoding="utf-8")
        git(root, ["add", "--", name])
        made = git(root, ["commit", "-qm", message])
        if made.returncode != 0:
            raise SystemExit("夹具提交失败：" + made.stdout)
        return git(root, ["rev-parse", "HEAD"]).stdout.strip()

    def branches(root: pathlib.Path) -> list[str]:
        return sorted(line.strip() for line in
                      git(root, ["for-each-ref", "--format=%(refname:short)",
                                 "refs/heads"]).stdout.splitlines() if line.strip())

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)

        check("不在 git 仓里要说清楚（不是空表混过去）",
              (works(root), any("不在 git 仓" in problem for problem in lost(root)[1])), (False, True))
        init_repo(root)
        (root / "scripts").mkdir()
        (root / "scripts" / "snapshot.py").write_text("print('snapshot stub ok')\n", encoding="utf-8")
        base = commit_file(root, "a.txt", "基线提交")

        # 场景一：branch -D 之后，提交只剩 reflog 与 fsck 两条线索
        commit_file(root, "b.txt", "普通提交")
        git(root, ["checkout", "-q", "-b", "side"])
        secret = commit_file(root, "c.txt", "要找回的活：feature 草稿")
        git(root, ["checkout", "-q", base])
        git(root, ["branch", "-D", "side"])
        items, problems, stats = search(root)
        check("branch -D 掉的提交被找回来", secret in [i["sha"] for i in items], True)
        check("候选标为丢失（不在任何分支上）",
              [i["refs"] for i in items if i["sha"] == secret], [[]])
        check("还在分支上的活不算丢", base in [i["sha"] for i in items], False)
        check("取证渠道没有哑火", problems, [])
        quick_items, quick_problems, _ = search(root, quick=True)
        check("quick 模式必须出声说没查 fsck",
              any("跳过了 fsck" in problem for problem in quick_problems), True)
        check("quick 模式不算查过（结果比完整模式少）",
              len(quick_items) <= len(items), True)
        check("两条线索都指向同一个孤儿",
              sorted(next(i["sources"] for i in items if i["sha"] == secret)),
              ["dangling", "reflog"])
        check("统计能对上", (stats["scanned"] > 0, stats["lost"] >= 1, stats["gc_gone"]),
              (True, True, 0))

        # 场景二：reset --hard 顶掉的提交（分离头指针上提交，再 reset 回去）
        git(root, ["checkout", "-q", secret])
        dropped = commit_file(root, "d.txt", "被 reset --hard 顶掉的那一次")
        git(root, ["reset", "--hard", base])
        check("reset --hard 顶掉的提交也在候选里",
              dropped in [i["sha"] for i in search(root)[0]], True)

        # 场景三：drop 掉的 stash 以悬挂提交形态回来
        (root / "a.txt").write_text("脏活\n", encoding="utf-8")
        pushed = git(root, ["stash", "push", "-m", "有东西要救"])
        stash_sha = git(root, ["stash", "list", "--format=%H"]).stdout.strip()
        check("stash push 成功且取得到 sha", (pushed.returncode, stash_sha != ""), (0, True))
        check("还在的 stash 也算候选", stash_sha in [i["sha"] for i in search(root)[0]], True)
        git(root, ["stash", "drop"])
        after_drop = search(root)[0]
        check("drop 掉的 stash 仍能从 fsck 找回", stash_sha in [i["sha"] for i in after_drop], True)
        check("它的 subject 认得出是 stash 活",
              any(mark in next(i["subject"] for i in after_drop if i["sha"] == stash_sha)
                  for mark in STASH_MARKS), True)

        # 过滤面
        check("--grep 只留命中的", [i["sha"] for i in search(root, pattern="feature")[0]], [secret])
        check("--grep 不命中时不硬凑", search(root, pattern="根本没这个词")[0], [])
        check("--grep 大小写不敏感", search(root, pattern="FEATURE")[0] != [], True)
        check("--author 走的是提交元数据", search(root, author="tester")[0] != [], True)
        check("--author 写错就一条不剩", search(root, author="没有这个人")[0], [])
        check("--since 太晚 ⇒ 一条不留", search(root, since="2999-01-01")[0], [])
        check("--since 很早 ⇒ 与不加过滤一致",
              len(search(root, since="1999-01-01")[0]), len(search(root)[0]))

        # 恢复：先快照、再建分支，HEAD/工作树都不动
        head_before = git(root, ["rev-parse", "HEAD"]).stdout.strip()
        tree_before = git(root, ["rev-parse", "HEAD^{tree}"]).stdout.strip()
        status_before = git(root, ["status", "--porcelain"]).stdout.strip()
        names_before = branches(root)
        check("恢复返回 0", restore(root, secret), 0)
        added = [name for name in branches(root) if name not in names_before]
        check("只多出一条分支", len(added), 1)
        check("新分支确实指向那个孤儿", git(root, ["rev-parse", added[0]]).stdout.strip(), secret)
        check("恢复不动 HEAD", git(root, ["rev-parse", "HEAD"]).stdout.strip(), head_before)
        check("恢复不动工作树（状态一模一样）",
              git(root, ["status", "--porcelain"]).stdout.strip(), status_before)
        check("恢复不改树内容", git(root, ["rev-parse", "HEAD^{tree}"]).stdout.strip(), tree_before)
        check("重复恢复另起一条，不覆盖上一次",
              restore(root, secret) == 0 and
              len([n for n in branches(root) if n not in names_before]), 2)
        check("恢复不删原对象", exists(root, secret), True)
        check("短 sha 也能恢复（人只敲前 7 位）", restore(root, secret[:7]) == 0, True)
        check("--branch 指定名字也走同一个不覆盖规矩",
              (restore(root, secret, "keepme") == 0 and "keepme" in branches(root)), True)
        check("同名分支已存在时不覆盖，加序号",
              (restore(root, secret, "keepme") == 0 and "keepme-1" in branches(root)), True)

        # 失败面
        check("形状不对的 sha 直接拒绝", restore(root, "not-a-sha"), 1)
        check("库里没有的 sha 说不可恢复",
              restore(root, "deadbeef" * 5), 1)
        with tempfile.TemporaryDirectory() as elsewhere:
            loud = io.StringIO()
            real_stdout, sys.stdout = sys.stdout, loud
            try:
                rc_barren = restore(pathlib.Path(elsewhere), secret)
            finally:
                sys.stdout = real_stdout
            check("非 git 仓里恢复要响亮拒绝并说清原因（不是撞到底层报错）",
                  (rc_barren, "不在 git 仓里" in loud.getvalue()), (1, True))
        count_before = len(branches(root))
        (root / "scripts" / "snapshot.py").write_text(
            "import sys\nsys.stderr.write('stub boom')\nsys.exit(3)\n", encoding="utf-8")
        check("快照失败 ⇒ 拒绝恢复并判红", restore(root, secret), 1)
        check("快照失败时没有偷偷建分支", len(branches(root)), count_before)

        # 命令面：把 ROOT 临时指到夹具仓，否则自测会去扫真仓（fsck 一趟就是几分钟）
        original_root = globals()["ROOT"]
        globals()["ROOT"] = root
        try:
            check("未知子命令判红而不是猜", main(["frobnicate"]), 1)
            check("search 子命令走的是取证那条路（rc 为 0）", main(["search", "--limit", "5"]), 0)
            with tempfile.TemporaryDirectory() as barren:
                barren_out = io.StringIO()
                real_stdout, sys.stdout = sys.stdout, barren_out
                try:
                    rc_barren = main(["--root", barren, "show", base])
                finally:
                    sys.stdout = real_stdout
                check("选项写在子命令前面也走 show（拒绝理由是「解不出提交」，不是不认识子命令）",
                      (rc_barren,
                       any("解不出提交" in line for line in barren_out.getvalue().splitlines()),
                       any("不认识的子命令" in line for line in barren_out.getvalue().splitlines())),
                      (1, True, False))
            short_out = io.StringIO()
            real_stdout, sys.stdout = sys.stdout, short_out
            try:
                rc_short = main(["show", "--root"])
            finally:
                sys.stdout = real_stdout
            check("选项少了取值 ⇒ 判红并点名是哪个选项（不猜它指哪儿）",
                  (rc_short, any("后面少了取值" in line for line in short_out.getvalue().splitlines())),
                  (1, True))
            show_out = io.StringIO()
            real_stdout, sys.stdout = sys.stdout, show_out
            try:
                rc_show = main(["show", base])
            finally:
                sys.stdout = real_stdout
            check("show 一个还在分支上的提交 ⇒ rc 为 0 且能说清它没丢",
                  (rc_show, any("还在这些分支上：" in line for line in show_out.getvalue().splitlines())),
                  (0, True))
            check("restore 不给 sha 就判红（不给 sha 的「恢复」等于随便挑一个）",
                  main(["restore", "--branch", "whatever"]), 1)
        finally:
            globals()["ROOT"] = original_root

        # 可达集合算不出来时不能报"丢了"：判不了就说判不了
        original_reachable = globals()["reachable_set"]
        globals()["reachable_set"] = lambda r: (set(), "rev-list 跑不动（合成故障）")
        try:
            unreachable_items, unreachable_problems, unreachable_stats = lost(root)
            check("可达集合读不出 ⇒ 不端出假候选，只报问题",
                  (unreachable_items == [],
                   any("rev-list 跑不动" in problem for problem in unreachable_problems),
                   unreachable_stats["lost"]), (True, True, 0))
        finally:
            globals()["reachable_set"] = original_reachable

        # 哑火必须出声：把每一路单独喂一个"不是 git 仓"的目录，看它是否报告跑不动
        with tempfile.TemporaryDirectory() as barren:
            bare = pathlib.Path(barren)
            check("reflog 一路哑火会点名", reflog_items(bare)[1] != "", True)
            check("fsck 一路哑火会点名", fsck_items(bare)[1] != "", True)
            check("stash 一路哑火会点名", stash_items(bare)[1] != "", True)

        # 列出来但对象已被 gc 收走：不能算"还能救"，要单列成 gc_gone
        original_gather = globals()["gather"]
        globals()["gather"] = lambda r, quick=False: ([{"sha": "f" * 40, "where": "x", "when": "",
                                                       "subject": "已经不存在的对象",
                                                       "sources": ["reflog"]}], [])
        try:
            _, _, gc_stats = lost(root)
            check("被 gc 收走的计入 gc_gone 而不是 lost", (gc_stats["gc_gone"], gc_stats["lost"]), (1, 0))
        finally:
            globals()["gather"] = original_gather

        # 恢复失败时说的必须是"不可恢复"，而不是继续往下摸（文案是判断依据，不能只看 rc）
        import io
        captured = io.StringIO()
        real_stdout, sys.stdout = sys.stdout, captured
        try:
            rc_missing = restore(root, "e" * 40)
        finally:
            sys.stdout = real_stdout
        check("库里没有的对象：直说不可恢复",
              (rc_missing, "不可恢复" in captured.getvalue()), (1, True))
        check("库里没有的 sha 说不可恢复（show 也一样判红）", main(["show", "not-a-sha"]), 1)
        check("restore 不给 sha 就判红（不给 sha 的「恢复」等于随便挑一个）", main(["restore"]), 1)

    print("oops 自测：" + str(passed) + " 过 / " + str(failed) + " 红")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
