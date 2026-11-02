#!/usr/bin/env python3
"""gitflow：一句话把改动送达 GitHub——提交 / 推送 / 开 PR / 看 CI / 绿即合。

用法：
  python scripts/gitflow.py publish --title "标题" [--body-file 正文.md] --files a b c
                                     [--branch 分支名] [--base main] [--no-merge] [--dry-run]
  python scripts/gitflow.py secret-scan --files a b c
  python scripts/gitflow.py status
  python scripts/gitflow.py branch list|start <名>|sync|prune [--apply]|protect [--apply] [--dry-run]
  python scripts/gitflow.py worktree add|list|remove
  python scripts/gitflow.py pr triage|sync <n> [--dry-run]
  python scripts/gitflow.py stack | diagnose --pr <n> | flaky | dco | queue | merge <n> [--wait-minutes N]
  python scripts/gitflow.py --self-test

为什么不是"git add -A"：
  开源仓库最怕把密钥/大文件/无关改动顺手提交。这里**必须显式点名文件**，
  提交前对内容做密钥扫描（sk-*、GitHub/Slack token、私钥头、AKIA…），命中就拒绝。
  没有任何路径会 force-push；`--dry-run` 只打印将执行的命令，不碰远端。

调用方只需要说"提交"，由代理跑这条命令——用户不必自己敲 git/gh。
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]

# 模式串在运行期拼接：本文件自己的源码里不许出现能被这些模式命中的字面量，
# 否则扫描器会把"它自己"判成泄密，工具就永远提交不了自己（自锁时真踩到过）。
SECRET_PATTERNS = [
    (re.compile("sk" + r"-[A-Za-z0-9]{16,}"), "OpenAI/DeepSeek 形状的 sk- 密钥"),
    (re.compile("gh" + r"p_[A-Za-z0-9]{36}"), "GitHub PAT"),
    (re.compile("AK" + r"IA[0-9A-Z]{16}"), "AWS access key"),
    (re.compile("xox" + r"[baprs]-[A-Za-z0-9-]{10,}"), "Slack token"),
    (re.compile(r"-----BEGIN " + r"[A-Z ]*PRIVATE KEY-----"), "私钥"),
]
ALLOW_MARKER = "secret-scan: allow"
SECRET_FILE_NAMES = {"credentials.json", "id_rsa", "id_ed25519", "id_dsa", ".npmrc", ".pypirc", "secrets.json", "secrets.yaml"}


def git(args: list[str], cwd: pathlib.Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args], cwd=ROOT if cwd is None else cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, encoding="utf-8", errors="replace",
    )


def gh(args: list[str], cwd: pathlib.Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["gh", *args], cwd=ROOT if cwd is None else cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, encoding="utf-8", errors="replace",
    )


def pick_base(branches: list[str]) -> str:
    """默认 base：有这个仓的默认分支就用它（main / master），否则 main。"""
    return "main" if "main" in branches else ("master" if "master" in branches else "main")


def detect_remote(root: pathlib.Path = ROOT) -> str:
    remotes = git(["remote"], root).stdout.split()
    if "origin" in remotes:
        return "origin"
    return remotes[0] if remotes else "origin"


def detect_base(remote: str, root: pathlib.Path = ROOT) -> str:
    names = [line.split()[-1] for line in git(["branch", "-r"], root).stdout.splitlines() if line.strip()]
    return pick_base([name.split("/", 1)[-1] for name in names])


REMOTE_RE = re.compile(r"github\.com[:/]([^/]+/[^/]+?)(?:\.git)?$")


def repo_from_url(url: str) -> str:
    """从 git remote URL 抠 `owner/name`；认不出就回落到默认仓。"""
    match = REMOTE_RE.search(url.strip().rstrip("/"))
    return match.group(1) if match else ""


def detect_repo(root: pathlib.Path = ROOT) -> str:
    """默认仓 = 本仓 remote 指向的那个；换仓使用同一份脚本不必改参数。"""
    lines = git(["remote", "-v"], root).stdout.splitlines()
    urls = [line.split()[1] for line in lines if len(line.split()) >= 2]
    for url in urls:
        name = repo_from_url(url)
        if name:
            return name
    return "bfxh/trit"


BRANCH_BAD = set(" ~^:?*[\\")
REPO_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


def branch_problems(name: str) -> list[str]:
    """分支名会直接进 git 命令：拒绝选项注入与 git 自己也不收的形态。"""
    problems: list[str] = []
    if not name:
        problems.append("分支名为空")
    if name.startswith("-"):
        problems.append("分支名不许以 - 开头（会被当成 git 选项）")
    if any(ch in BRANCH_BAD or ord(ch) < 32 for ch in name):
        problems.append("分支名含空白/控制字符或 ~^:?*[\\")
    if ".." in name or "//" in name or name.endswith("/") or name.endswith(".lock"):
        problems.append("分支名形态非法（..、//、结尾 / 或 .lock）")
    return problems


def repo_problems(repo: str) -> list[str]:
    return [] if REPO_RE.match(repo) else [f"仓库名应为 owner/name：{repo}"]


def staged_outside(root: pathlib.Path, files: list[str]) -> list[str]:
    """索引里已有的、但不在本次点名列表里的文件——提交会把它们一起带上。"""
    staged = [line for line in git(["diff", "--cached", "--name-only"], root).stdout.splitlines() if line.strip()]
    return sorted(set(staged) - set(files))


PROTECTED = {"main", "master"}


def parse_ahead_behind(text: str) -> tuple[int, int]:
    """`git rev-list --left-right --count` 的输出 -> (ahead, behind)。"""
    parts = text.split()
    if len(parts) == 2 and all(part.isdigit() for part in parts):
        return int(parts[0]), int(parts[1])
    return 0, 0


def prune_candidates(names: list[str], merged: set[str], current: str, bases: set[str]) -> list[str]:
    """可安全删除的本地分支：已并入 base、不是当前分支、不是受保护分支。"""
    return sorted(name for name in names if name in merged and name != current and name not in bases)


def mark_stale(rows: list[dict], now: int, stale_days: int) -> list[dict]:
    for row in rows:
        row["stale"] = bool(row.get("merged")) and (now - int(row.get("last_commit", now))) > stale_days * 86400
    return sorted(rows, key=lambda row: (not row["stale"], row["name"]))


def sanitize(branch: str) -> str:
    return branch.replace("/", "-").replace("\\", "-")


def worktree_path_for(root: pathlib.Path, branch: str) -> pathlib.Path:
    """默认放在仓的**旁边**：放进仓里会被自身扫描/提交。"""
    return root.parent / f"{root.name}-{sanitize(branch)}"


def parse_worktrees(text: str) -> list[dict]:
    entries: list[dict] = []
    current: dict = {}
    for line in text.splitlines():
        if not line.strip():
            if current:
                entries.append(current)
                current = {}
            continue
        key, _, value = line.partition(" ")
        if key == "worktree":
            current["path"] = value
        elif key == "branch":
            current["branch"] = value.replace("refs/heads/", "")
        elif key == "HEAD":
            current["head"] = value
        elif key == "bare":
            current["bare"] = True
        elif key == "detached":
            current["detached"] = True
    if current:
        entries.append(current)
    return entries


def pr_chains(prs: list[dict]) -> tuple[list[list[dict]], list[dict]]:
    """把 open PR 按 base->head 串成链；返回 (链列表, 基不是主干的 PR)。"""
    by_base: dict[str, list[dict]] = {}
    for pr in prs:
        by_base.setdefault(pr["baseRefName"], []).append(pr)
    heads = {pr["headRefName"] for pr in prs}
    roots = [pr for pr in prs if pr["baseRefName"] not in heads]
    chains: list[list[dict]] = []
    for root in roots:
        chain = [root]
        seen = {root["number"]}
        while True:
            children = [c for c in by_base.get(chain[-1]["headRefName"], []) if c["number"] not in seen]
            if not children:
                break
            chain.append(children[0])
            seen.add(children[0]["number"])
        chains.append(chain)
    off_trunk = [pr for pr in prs if pr["baseRefName"] not in PROTECTED]
    return chains, off_trunk


def file_jaccard(left: list[str], right: list[str]) -> float:
    a, b = set(left), set(right)
    return len(a & b) / len(a | b) if (a | b) else 0.0


def classify_pr(pr: dict, now: float, stale_days: int, required: list[str], others: list[dict]) -> list[str]:
    """给一个 open PR 打标签：draft / stale / conflicting / missing-required-check / superseded-by-#N。"""
    labels: list[str] = []
    if pr.get("isDraft"):
        labels.append("draft")
    updated = pr.get("updatedAt") or ""
    try:
        # ISO8601 -> epoch（只用于比较天数，粗糙到秒即可）
        stamp = time.mktime(time.strptime(updated[:19].replace("T", " "), "%Y-%m-%d %H:%M:%S"))
        if now - stamp > stale_days * 86400:
            labels.append("stale")
    except (ValueError, TypeError):
        pass
    if (pr.get("mergeable") or "").upper() == "CONFLICTING":
        labels.append("conflicting")
    present = {check.get("name") for check in (pr.get("statusCheckRollup") or [])}
    missing = [name for name in required if name not in present and name not in _matrix_covered(name, present)]
    if missing:
        labels.append("missing-required-check")
    files = [entry.get("path", "") for entry in (pr.get("files") or [])]
    for other in others:
        if other["number"] == pr["number"]:
            continue
        overlap = file_jaccard(files, [entry.get("path", "") for entry in (other.get("files") or [])])
        if overlap >= 0.8:
            labels.append(f"superseded-by-#{other['number']}")
            break
    return labels


def _matrix_covered(name: str, present: set[str]) -> set[str]:
    """矩阵展开的检查名：`X (${{ matrix.os }})` 之类按前缀算已覆盖。"""
    covered = set()
    for existing in present:
        for context in (name,):
            if existing.startswith(context.split(" (")[0] + " ("):
                covered.add(context)
    return covered


def missing_signoff(log_text: str) -> list[str]:
    """`git log --format=%H%x1f%B%x1e` 的输出 -> 缺 Signed-off-by 的短 sha 列表。"""
    missing: list[str] = []
    for record in log_text.split(chr(30)):
        record = record.strip("\n")
        if not record.strip():
            continue
        sha, _, body = record.partition(chr(31))
        if "Signed-off-by:" not in body:
            missing.append(sha.strip()[:12])
    return missing


def flaky_groups(rows: list[dict]) -> list[dict]:
    """同一提交多次运行、同一 job 结论不一致的组。

    rows: [{"sha", "run", "jobs": {job_name: conclusion}}]
    """
    # 按 (提交, workflow) 分组：同一提交的 CI / t3 / mutation 是不同 workflow，job 集合本来就不同
    buckets: dict[tuple[str, str], list[dict]] = {}
    for row in rows:
        buckets.setdefault((row["sha"], row.get("workflow", "")), []).append(row)
    out: list[dict] = []
    for (sha, workflow), group in buckets.items():
        if len(group) < 2:
            continue
        names = set().union(*(row["jobs"].keys() for row in group))
        unstable = {}
        for name in names:
            conclusions = sorted({row["jobs"].get(name, "missing") for row in group})
            if len(conclusions) > 1:
                unstable[name] = conclusions
        if unstable:
            out.append({"sha": sha[:8], "workflow": workflow, "retries": len(group), "unstable": unstable})
    return out


def failed_checks(rollup: list[dict]) -> list[dict]:
    """从 PR 的 statusCheckRollup 里挑出失败的；成功/跳过/未完成都不算失败。"""
    bad: list[dict] = []
    for check in rollup:
        conclusion = (check.get("conclusion") or "").upper()
        if conclusion in ("SUCCESS", "SKIPPED", "NEUTRAL", ""):
            continue
        bad.append(check)
    return bad


def branch_list(repo: str, base: str, stale_days: int, remote_state: bool) -> int:
    refs = git(["for-each-ref", "--format=%(refname:short)|%(committerdate:unix)", "refs/heads"]).stdout
    names: list[str] = []
    dates: dict[str, int] = {}
    for line in refs.splitlines():
        if "|" not in line:
            continue
        name, stamp = line.rsplit("|", 1)
        names.append(name)
        dates[name] = int(stamp) if stamp.strip().isdigit() else 0
    merged = {
        line.strip().lstrip("* ").strip()
        for line in git(["branch", "--merged", base]).stdout.splitlines()
        if line.strip()
    }
    current = git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip()
    rows: list[dict] = []
    for name in names:
        ahead, behind = parse_ahead_behind(git(["rev-list", "--left-right", "--count", f"{base}...{name}"]).stdout)
        row = {"name": name, "ahead": ahead, "behind": behind, "merged": name in merged, "last_commit": dates[name]}
        if remote_state and name not in PROTECTED:
            listing = gh(["pr", "list", "--repo", repo, "--head", name, "--state", "all", "--json", "number,state", "--limit", "1"])
            if listing.returncode == 0 and listing.stdout.strip():
                try:
                    data = json.loads(listing.stdout)
                    row["pr"] = f"#{data[0]['number']}/{data[0]['state']}" if data else "-"
                except (json.JSONDecodeError, KeyError, IndexError):
                    row["pr"] = "?"
        rows.append(row)
    rows = mark_stale(rows, int(time.time()), stale_days)
    print(f"本地分支（base={base}，当前={current}，陈旧阈值={stale_days} 天）")
    for row in rows:
        flags = []
        if row["name"] == current:
            flags.append("当前")
        if row["merged"]:
            flags.append("已并入")
        if row["stale"]:
            flags.append("陈旧")
        if row.get("pr"):
            flags.append(row["pr"])
        print(f"  {row['name']:<40} +{row['ahead']}/-{row['behind']}  " + " ".join(flags))
    return 0


def branch_start(name: str, base: str, remote: str, dry_run: bool) -> int:
    problems = branch_problems(name)
    if problems:
        for problem in problems:
            print(f"拒绝：{problem}")
        return 2
    if name in PROTECTED:
        print(f"拒绝：{name} 是受保护分支名")
        return 2
    if git(["branch", "--list", name]).stdout.strip():
        print(f"拒绝：分支已存在：{name}")
        return 2
    commands = [
        ["git", "fetch", remote, base],
        ["git", "checkout", "-b", name, "--no-track", f"{remote}/{base}"],
    ]
    if dry_run:
        print("dry-run：将执行")
        for command in commands:
            print("  " + " ".join(command))
        return 0
    for command in commands:
        result = git(command[1:])
        print(f"$ {' '.join(command[:3])} … rc={result.returncode}")
        if result.returncode != 0:
            print(result.stdout[-1000:])
            return 1
    print(f"已从 {remote}/{base} 起分支：{name}")
    return 0


def branch_sync(base: str, remote: str, dry_run: bool) -> int:
    current = git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip()
    if current in PROTECTED:
        print(f"拒绝：当前在受保护分支 {current} 上，先切到你的分支")
        return 2
    dirty = [line for line in git(["status", "--porcelain"]).stdout.splitlines() if line.strip()]
    if dirty:
        print("拒绝：工作树有未提交改动，先提交或清理（rebase 会拒绝/丢改动）")
        return 2
    target = f"{remote}/{base}"
    if dry_run:
        print(f"dry-run：git fetch {remote} {base} && git rebase {target}")
        return 0
    fetched = git(["fetch", remote, base])
    if fetched.returncode != 0:
        print(fetched.stdout[-1000:])
        return 1
    rebase = git(["rebase", target])
    if rebase.returncode != 0:
        conflicted = [line for line in git(["diff", "--name-only", "--diff-filter=U"]).stdout.splitlines() if line.strip()]
        print("rebase 冲突，已 abort（分支回到原样）：")
        for name in conflicted:
            print(f"  {name}")
        git(["rebase", "--abort"])
        return 1
    print(f"已把 {current} rebase 到 {target}")
    return 0


def branch_prune(base: str, apply_changes: bool) -> int:
    current = git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip()
    names = [line.strip().lstrip("* ").strip() for line in git(["branch", "--format=%(refname:short)"]).stdout.splitlines() if line.strip()]
    merged = {
        line.strip().lstrip("* ").strip()
        for line in git(["branch", "--merged", base]).stdout.splitlines()
        if line.strip()
    }
    candidates = prune_candidates(names, merged, current, PROTECTED | {base})
    if not candidates:
        print(f"没有可清理的分支（base={base}，当前={current}）")
        return 0
    for name in candidates:
        print(f"  {'删除' if apply_changes else '候选'} {name}")
    if not apply_changes:
        print(f"dry-run：加 --apply 才会删除这 {len(candidates)} 个已并入 {base} 的本地分支")
        return 0
    failed = 0
    for name in candidates:
        result = git(["branch", "-d", name])
        print(f"$ git branch -d {name} … rc={result.returncode}")
        if result.returncode != 0:
            failed += 1
    return 1 if failed else 0


def branch_cmd(repo: str, argv: list[str]) -> int:
    base_arg = argv[argv.index("--base") + 1] if "--base" in argv else ""
    base = base_arg or detect_base(detect_remote())
    remote = argv[argv.index("--remote") + 1] if "--remote" in argv else detect_remote()
    sub = argv[0] if argv else "list"
    rest = [a for a in argv[1:] if not a.startswith("--")]
    if sub == "list":
        stale_days = int(argv[argv.index("--stale-days") + 1]) if "--stale-days" in argv else 14
        return branch_list(repo, base, stale_days, remote_state="--no-remote" not in argv)
    if sub == "start":
        if not rest:
            print("用法：gitflow.py branch start <名字> [--from main]")
            return 2
        source = argv[argv.index("--from") + 1] if "--from" in argv else base
        return branch_start(rest[0], source, remote, "--dry-run" in argv)
    if sub == "sync":
        return branch_sync(base, remote, "--dry-run" in argv)
    if sub == "prune":
        if "--remote" in argv:
            return branch_prune_remote(repo, base, remote, "--apply" in argv)
        return branch_prune(base, "--apply" in argv)
    if sub == "protect":
        target = argv[argv.index("--branch") + 1] if "--branch" in argv else base
        return protect(repo, target, "--apply" in argv, "--dry-run" in argv, "--force" in argv)
    print(f"未知分支子命令: {sub}（list / start / sync / prune）")
    return 2


def worktree_list() -> int:
    rows = parse_worktrees(git(["worktree", "list", "--porcelain"]).stdout)
    current = git(["rev-parse", "--show-toplevel"]).stdout.strip()
    for row in rows:
        flags = []
        if row.get("path") == current:
            flags.append("当前")
        if row.get("bare"):
            flags.append("裸仓")
        if row.get("detached"):
            flags.append("detached")
        print(f"  {row.get('path', '?'):<60} {row.get('branch', '-')}  {' '.join(flags)}")
    return 0


def worktree_add(branch: str, base: str, remote: str, path: str, dry_run: bool) -> int:
    problems = branch_problems(branch)
    if problems:
        for problem in problems:
            print(f"拒绝：{problem}")
        return 2
    target = pathlib.Path(path) if path else worktree_path_for(ROOT, branch)
    if target.resolve() == ROOT.resolve() or ROOT.resolve() in target.resolve().parents:
        print(f"拒绝：worktree 不能放在仓里（{target}）")
        return 2
    if target.exists():
        print(f"拒绝：目标已存在：{target}")
        return 2
    exists = bool(git(["branch", "--list", branch]).stdout.strip())
    commands = [["git", "fetch", remote, base]]
    if exists:
        commands.append(["git", "worktree", "add", str(target), branch])
    else:
        commands.append(["git", "worktree", "add", "-b", branch, str(target), f"{remote}/{base}"])
    if dry_run:
        print("dry-run：将执行")
        for command in commands:
            print("  " + " ".join(command))
        return 0
    for command in commands:
        result = git(command[1:])
        print(f"$ {' '.join(command[:3])} … rc={result.returncode}")
        if result.returncode != 0:
            print(result.stdout[-1000:])
            return 1
    print(f"worktree 已就绪：{target}（分支 {branch}）")
    return 0


def worktree_remove(path: str, force: bool) -> int:
    target = pathlib.Path(path).resolve()
    current = pathlib.Path(git(["rev-parse", "--show-toplevel"]).stdout.strip()).resolve()
    if target == current:
        print("拒绝：不能删当前所在的工作树")
        return 2
    args = ["worktree", "remove", str(target)]
    if force:
        args.append("--force")
    result = git(args)
    print(f"$ git worktree remove {target} … rc={result.returncode}")
    if result.returncode != 0:
        print(result.stdout[-1000:])
        return 1
    return 0


def stack(repo: str, remote_state: bool) -> int:
    if not remote_state:
        print("stack 需要 gh（去掉 --no-remote）")
        return 2
    listing = gh(["pr", "list", "--repo", repo, "--state", "open", "--json",
                  "number,title,headRefName,baseRefName", "--limit", "100"])
    if listing.returncode != 0:
        print(listing.stdout[-1000:])
        return 1
    try:
        prs = json.loads(listing.stdout or "[]")
    except json.JSONDecodeError:
        print("gh 输出解析失败")
        return 1
    if not prs:
        print("没有 open PR")
        return 0
    chains, off_trunk = pr_chains(prs)
    for chain in chains:
        flow = " → ".join(f"{pr['baseRefName']}" for pr in chain)
        flow += f" → {chain[-1]['headRefName']}"
        print(flow)
        for pr in chain:
            print(f"    #{pr['number']} {pr['title'][:60]}")
    if off_trunk:
        print("警告：下列 PR 的 base 不是 main/master。按 base 过滤的 CI 可能零 check，合并前先确认：")
        for pr in off_trunk:
            print(f"    #{pr['number']} base={pr['baseRefName']}")
    return 0


def read_required_checks(path: pathlib.Path) -> list[str]:
    """.github/required-checks.txt 的正文行（# 注释与空行不算）。"""
    if not path.is_file():
        return []
    return [line.strip() for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")]


def strip_urls(node):
    """GET 保护规则会带回只读的 url 字段，PUT 不认——原样送回会被 422。"""
    if isinstance(node, dict):
        return {key: strip_urls(value) for key, value in node.items() if key != "url"}
    if isinstance(node, list):
        return [strip_urls(item) for item in node]
    return node


def diff_contexts(got: list[str], wanted: list[str]) -> list[str]:
    """推送后复核：远端与本地台账的差异，空列表才叫真对上。"""
    problems = [f"少 {context}" for context in sorted(set(wanted) - set(got))]
    problems += [f"多 {context}" for context in sorted(set(got) - set(wanted))]
    return problems


def load_ci_integrity():
    """复用 ci_integrity 的 job 解析与矩阵匹配，避免两边口径漂移。"""
    import importlib.util

    path = pathlib.Path(__file__).resolve().parent / "ci_integrity.py"
    spec = importlib.util.spec_from_file_location("trit_ci_integrity", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def delta_contexts(current: list[str], wanted: list[str]) -> list[str]:
    """这次要新增的必需检查（老 PR 已经卡着的不在本次责任里）。"""
    return sorted(set(wanted) - set(current))


def missing_on_branch(jobs, wanted: list[str], ci) -> list[str]:
    """这个分支上不会上报的上下文。jobs=None（取不到 workflow）按"全都缺"算——
    证明不了它会跑，就不许把它设成必需。"""
    if jobs is None:
        return list(wanted)
    return [context for context in wanted
            if not any(ci.matches_context(job, context) for job in jobs)]


def protection_blockers(prs: list[dict], jobs_by_branch: dict, added: list[str], ci) -> list[str]:
    """加必需检查之前的拦路清单：开着的 PR 里，谁的分支根本不会跑这些 job。

    GitHub 的行为是"必需检查从不出现 ⇒ 这个 PR 永远合不了"，所以这一步不能省，
    也不能靠人肉记得（本仓差点就把两个新门推上去，卡住三条在跑的 PR）。
    """
    blockers: list[str] = []
    if not added:
        return blockers
    for pr in prs:
        branch = pr.get("headRefName") or ""
        if jobs_by_branch.get(branch) is None:
            blockers.append(f"#{pr.get('number')}（{branch}）读不到 workflow ⇒ 无法证明会跑："
                            f"{'、'.join(added)}")
            continue
        gaps = missing_on_branch(jobs_by_branch[branch], added, ci)
        if gaps:
            blockers.append(f"#{pr.get('number')}（{branch}）不会上报：{'、'.join(gaps)}")
    return blockers


# API 路径只认正斜杠；ci_integrity.WORKFLOWS 是本地 os.path 常量（Windows 上是反斜杠）
WORKFLOWS_URL = ".github/workflows"


def remote_workflow_jobs(ci, repo: str, branch: str) -> list[dict] | None:
    import base64

    listing = gh(["api", f"repos/{repo}/contents/{WORKFLOWS_URL}?ref={branch}"])
    if listing.returncode != 0:
        return None
    try:
        entries = json.loads(listing.stdout or "[]")
    except json.JSONDecodeError:
        return None
    if not isinstance(entries, list):
        return None
    with tempfile.TemporaryDirectory() as tmp:
        box = pathlib.Path(tmp)
        (box / WORKFLOWS_URL).mkdir(parents=True, exist_ok=True)
        found = 0
        for entry in entries:
            name = entry.get("name") or ""
            if not name.endswith((".yml", ".yaml")):
                continue
            raw = gh(["api", f"repos/{repo}/contents/{WORKFLOWS_URL}/{name}?ref={branch}",
                      "--jq", ".content"])
            if raw.returncode != 0:
                return None
            (box / WORKFLOWS_URL / name).write_bytes(base64.b64decode(raw.stdout))
            found += 1
        if found == 0:
            return None
        return ci.parse_workflows(box)


def open_pr_list(repo: str) -> list[dict] | None:
    result = gh(["pr", "list", "--repo", repo, "--state", "open", "--json", "number,headRefName"])
    if result.returncode != 0:
        return None
    try:
        data = json.loads(result.stdout or "[]")
    except json.JSONDecodeError:
        return None
    return data if isinstance(data, list) else None


def build_protection_body(rule: dict, wanted: list[str]) -> dict:
    """组 PUT /branches/<b>/protection 的请求体。GitHub 这套 schema 很挑：

    - GET 回来的只读 `url` 原样送回 ⇒ 422；
    - `restrictions` 整个键缺失（哪怕它本该是 null）⇒ 422；
    - `enforce_admins` 这里要布尔，不是 GET 那种 `{"enabled": true}`。
    所以显式列要送的字段，而不是把 GET 结果改两下就发出去（第一版就这么被拒了）。
    """
    status = rule.get("required_status_checks") or {}
    reviews = rule.get("required_pull_request_reviews")
    restrictions = rule.get("restrictions")
    body = {
        "required_status_checks": {"strict": bool(status.get("strict", True)),
                                   "contexts": list(wanted)},
        "required_pull_request_reviews": strip_urls(reviews) if reviews else None,
        "restrictions": strip_urls(restrictions) if restrictions else None,
        "enforce_admins": bool((rule.get("enforce_admins") or {}).get("enabled", False)),
    }
    for flag in ("allow_force_pushes", "allow_deletions", "required_linear_history",
                 "required_conversation_resolution"):
        node = rule.get(flag)
        if isinstance(node, dict) and "enabled" in node:
            body[flag] = bool(node["enabled"])
    return body


def has_url_key(node) -> bool:
    """递归查只读 url：payload 里残留一个就等于整份被拒。"""
    if isinstance(node, dict):
        return "url" in node or any(has_url_key(value) for value in node.values())
    if isinstance(node, list):
        return any(has_url_key(item) for item in node)
    return False


def apply_required_checks(repo: str, branch: str, rule: dict, wanted: list[str],
                          dry_run: bool, force: bool = False) -> int:
    """把本地台账推到分支保护，并把改动前的规则存成回滚点。

    这里唯一不能出错的是"空清单"：台账读出来是空（文件没了/全被注释）就当"撤掉所有必需检查"
    推上去，等于自己把保护关掉 ⇒ 必须先拒绝。
    """
    if not wanted:
        print("拒收：本地必需检查清单是空的（文件缺失或全是注释）。"
              "真要撤掉保护请显式操作，不许由一次读空触发。")
        return 1
    current = (rule.get("required_status_checks") or {}).get("contexts") or []
    added = delta_contexts(current, wanted)
    if added and not force:
        ci = load_ci_integrity()
        prs = open_pr_list(repo)
        if prs is None:
            print("拒推：读不到开着的 PR 清单，无法证明新检查不会把别人的 PR 卡死。"
                  "确认无误再加 --force。")
            return 1
        jobs_by_branch: dict[str, list[dict] | None] = {}
        for pr in prs:
            name = pr.get("headRefName") or ""
            if name not in jobs_by_branch:
                jobs_by_branch[name] = remote_workflow_jobs(ci, repo, name)
        blockers = protection_blockers(prs, jobs_by_branch, added, ci)
        if blockers:
            print(f"拒推：这次要新增 {len(added)} 条必需检查，但 {len(blockers)} 条开着的 PR "
                  "的分支上这些 job 不会跑 ⇒ 设成必需就把它们永久卡住：")
            for line in blockers:
                print(f"  - {line}")
            print("补救：先 `gitflow pr sync <n>` 把 main 并进那个分支（带来 workflow），"
                  "或本次先不加这两条必需。确认要强行加才用 --force。")
            return 1
    body = build_protection_body(rule, wanted)
    if dry_run:
        print(f"dry-run：将把 {len(wanted)} 条必需检查写入 {repo}:{branch}")
        for context in wanted:
            print(f"  + {context}")
        return 0
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    rollback = pathlib.Path(tempfile.gettempdir()) / f"protection_before_{repo.replace('/', '_')}_{stamp}.json"
    rollback.write_text(json.dumps(rule, ensure_ascii=False, indent=1), encoding="utf-8")
    payload = tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".json", delete=False)
    try:
        json.dump(body, payload, ensure_ascii=False)
        payload.close()
        result = gh(["api", "-X", "PUT", f"repos/{repo}/branches/{branch}/protection",
                     "--input", payload.name])
    finally:
        pathlib.Path(payload.name).unlink(missing_ok=True)
    if result.returncode != 0:
        print(f"写入失败（回滚点在 {rollback}）：{result.stdout.strip()[:600]}")
        return 1
    after = gh(["api", f"repos/{repo}/branches/{branch}/protection/required_status_checks"])
    try:
        got = (json.loads(after.stdout) or {}).get("contexts") or []
    except json.JSONDecodeError:
        print(f"写完读不回，无法核对（回滚点：{rollback}）")
        return 1
    problems = diff_contexts(got, wanted)
    if problems:
        print(f"写完复核不一致：远端 {len(got)} 条｜{'、'.join(problems)}（回滚点：{rollback}）")
        return 1
    print(f"已写入 {len(got)} 条必需检查；改动前的规则存在 {rollback}")
    return 0


def protect(repo: str, branch: str, apply_changes: bool = False, dry_run: bool = False,
          force: bool = False) -> int:
    info = gh(["api", f"repos/{repo}/branches/{branch}"])
    if info.returncode != 0:
        print(f"查不到分支 {branch}：")
        print(info.stdout[-800:])
        return 1
    try:
        data = json.loads(info.stdout)
    except json.JSONDecodeError:
        print("gh api 输出解析失败")
        return 1
    protected = bool(data.get("protected"))
    print(f"{repo}:{branch} 受保护={protected}")
    if not protected:
        print("提示：没有保护规则时，publish 仍会拒绝直接提交主干——但远端本身不做拦截。")
        return 0
    rules = gh(["api", f"repos/{repo}/branches/{branch}/protection"])
    if rules.returncode != 0:
        print("（有保护，但当前 token 无权读规则详情）")
        return 0
    try:
        rule = json.loads(rules.stdout)
    except json.JSONDecodeError:
        return 0
    checks = (rule.get("required_status_checks") or {}).get("contexts") or []
    reviews = (rule.get("required_pull_request_reviews") or {}).get("required_approving_review_count")
    print(f"  必需检查：{', '.join(checks) if checks else '（无）'}")
    print(f"  必需审批数：{reviews if reviews is not None else '（无）'}")
    print(f"  强制直推：{bool(rule.get('enforce_admins', {}).get('enabled'))}")
    if apply_changes:
        wanted = read_required_checks(ROOT / ".github" / "required-checks.txt")
        print(f"本地台账 {len(wanted)} 条 ⇒ 推送到 {repo}:{branch}")
        return apply_required_checks(repo, branch, rule, wanted, dry_run, force)
    return 0


def branch_prune_remote(repo: str, base: str, remote: str, apply_changes: bool) -> int:
    listing = git(["branch", "-r", "--merged", f"{remote}/{base}", "--format=%(refname:short)"])
    names = []
    for line in listing.stdout.splitlines():
        name = line.strip()
        if not name or name.endswith("/HEAD"):
            continue
        short = name.split("/", 1)[-1]
        if short in PROTECTED or short == base:
            continue
        names.append(short)
    if not names:
        print(f"没有已并入 {remote}/{base} 的远端分支")
        return 0
    candidates: list[str] = []
    for name in sorted(set(names)):
        merged_pr = gh(["pr", "list", "--repo", repo, "--head", name, "--state", "merged",
                        "--json", "number", "--limit", "1"])
        if merged_pr.returncode == 0 and merged_pr.stdout.strip() not in ("", "[]"):
            candidates.append(name)
    if not candidates:
        print("有已并入的远端分支，但没有 MERGED 的 PR 记录（不删，避免误删他人分支）")
        return 0
    for name in candidates:
        print(f"  {'删除' if apply_changes else '候选'} 远端 {name}")
    if not apply_changes:
        print(f"dry-run：加 --apply 才会删这 {len(candidates)} 个远端分支")
        return 0
    failed = 0
    for name in candidates:
        result = git(["push", remote, "--delete", name])
        print(f"$ git push {remote} --delete {name} … rc={result.returncode}")
        if result.returncode != 0:
            failed += 1
    return 1 if failed else 0


def pr_list_payload(repo: str) -> list[dict]:
    listing = gh(["pr", "list", "--repo", repo, "--state", "open", "--limit", "100", "--json",
                  "number,title,headRefName,baseRefName,isDraft,updatedAt,mergeable,statusCheckRollup,files"])
    if listing.returncode != 0:
        print(listing.stdout[-1000:])
        return []
    try:
        return json.loads(listing.stdout or "[]")
    except json.JSONDecodeError:
        print("gh 输出解析失败")
        return []


def pr_triage(repo: str, stale_days: int, as_json: bool, apply_changes: bool) -> int:
    prs = pr_list_payload(repo)
    if not prs:
        print("没有 open PR")
        return 0
    required = []
    required_file = ROOT / ".github" / "required-checks.txt"
    if required_file.is_file():
        required = [
            line.strip()
            for line in required_file.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.strip().startswith("#")
        ]
    now = time.time()
    rows = []
    for pr in prs:
        others = [other for other in prs if other["number"] != pr["number"]]
        rows.append({"pr": pr, "labels": classify_pr(pr, now, stale_days, required, others)})
    if as_json:
        print(json.dumps(
            [{"number": row["pr"]["number"], "title": row["pr"]["title"],
              "branch": row["pr"]["headRefName"], "labels": row["labels"]} for row in rows],
            ensure_ascii=False, indent=1,
        ))
        return 0
    actionable = 0
    for row in rows:
        pr = row["pr"]
        labels = row["labels"] or ["ok"]
        print(f"#{pr['number']:<4} {pr['headRefName']:<40} [{', '.join(labels)}] {pr['title'][:50]}")
        if set(labels) & {"stale", "conflicting", "missing-required-check"}:
            actionable += 1
    print(f"\n可动作 {actionable} / 共 {len(rows)}")
    if actionable and not apply_changes:
        print("dry-run：对可动作项跑 `gitflow pr sync <编号>` 把 main 合进分支（不 force、不改写历史）；"
              "`--apply` 会逐个执行")
        return 0
    if apply_changes:
        for row in rows:
            if set(row["labels"]) & {"stale", "conflicting", "missing-required-check"}:
                print(f"== 同步 #{row['pr']['number']}（{row['pr']['headRefName']}）")
                if pr_sync(repo, str(row["pr"]["number"])) != 0:
                    print("  失败，停下（后面不继续动）")
                    return 1
    return 0


CONFLICT_RECORD = re.compile(r"^\s+(?:base|our|their|ancestor)\s+\d{6}\s+[0-9a-f]+\s+(\S.*)$")


def merge_tree_conflicts(text: str) -> list[str]:
    """从 `git merge-tree <base> <ours> <theirs>` 的输出里抠出冲突文件。

    "changed in both" 只是**块头**，路径在紧随其后的 base/our/their 三行里——
    只印块头等于什么都没点名（第一版就是这样：预览说"会冲突"却不说是哪几个文件）。
    """
    paths: list[str] = []
    in_block = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.endswith("in both") or stripped.startswith("<<<<<<<"):
            in_block = True
            continue
        if not in_block:
            continue
        match = CONFLICT_RECORD.match(line)
        if match:
            if match.group(1) not in paths:
                paths.append(match.group(1))
        elif line[:1] not in (" ", "\t"):
            in_block = False
    return paths


OID_RE = re.compile(r"^[0-9a-f]{40}$")


def merge_preview(modern_rc: int, modern_out: str, legacy_out: str) -> tuple[bool, list[str], str]:
    """这次合并会不会冲突、冲突在哪些文件、用的哪种 merge-tree 口径。

    老式 `git merge-tree <base> <ours> <theirs>` 的 `changed in both` 只是块头，
    而且**两边都改过但能自动合并的文件也打这块** ⇒ 拿它判冲突会过度报警
    （#198/#199 就被它报成"有真冲突"，实际干净合上了）。
    所以优先 `--write-tree`（rc≠0 才是真冲突，oid 之后是冲突文件名），不可用才退回老口径。
    """
    lines = [line for line in modern_out.splitlines() if line.strip()]
    if lines and OID_RE.fullmatch(lines[0].strip()) and modern_rc in (0, 1):
        paths = [line.strip() for line in lines[1:]
                 if not line.strip().startswith(("Auto-merging", "CONFLICT"))]
        return (modern_rc != 0 or bool(paths)), paths, "write-tree"
    paths = merge_tree_conflicts(legacy_out)
    return bool(paths) or "in both" in legacy_out, paths, "legacy"


def pr_sync(repo: str, number: str, dry_run: bool = False) -> int:
    """把 main merge 进 PR 分支并 push：不改写历史，不动别人的提交。"""
    view = gh(["pr", "view", number, "--repo", repo, "--json", "headRefName,number"])
    if view.returncode != 0:
        print(view.stdout[-1000:])
        return 1
    try:
        branch = json.loads(view.stdout)["headRefName"]
    except (json.JSONDecodeError, KeyError):
        print("解析 PR 分支失败")
        return 1
    base = detect_base(detect_remote())
    if dry_run:
        # 只预览冲突：merge-tree 不碰工作树，也不产生提交
        git(["fetch", "origin", base])
        base_sha = git(["rev-parse", f"origin/{base}"]).stdout.strip()
        branch_sha = git(["rev-parse", f"origin/{branch}"]).stdout.strip()
        if not branch_sha:
            print(f"拉不到 origin/{branch}（先 push 或 fetch）")
            return 1
        merge_base = git(["merge-base", base_sha, branch_sha]).stdout.strip() or base_sha
        modern = git(["merge-tree", "--write-tree", "--name-only", base_sha, branch_sha])
        legacy = git(["merge-tree", merge_base, base_sha, branch_sha])
        conflicted, conflicts, how = merge_preview(modern.returncode, modern.stdout, legacy.stdout)
        if conflicted:
            print(f"预览：把 {base} 合进 {branch} 会冲突（{len(conflicts)} 个文件，口径 {how}）")
            for path in conflicts[:12]:
                print(f"  - {path}")
            if not conflicts:
                print("  （merge-tree 说有冲突但没解析出文件名，原文前几行：）")
                for line in tree.stdout.splitlines()[:6]:
                    print(f"    {line}")
            return 1
        print(f"预览：把 {base} 合进 {branch} 无冲突（口径 {how}）")
        return 0
    # 分支可能正被另一个 worktree 占用（多会话常态）：在**那里**操作，不强行 checkout
    worktrees = parse_worktrees(git(["worktree", "list", "--porcelain"]).stdout)
    host = next((entry.get("path") for entry in worktrees if entry.get("branch") == branch), "")
    workdir = pathlib.Path(host) if host else ROOT
    if host and workdir.resolve() != ROOT.resolve():
        print(f"分支在另一个 worktree：{workdir}（就在那里操作）")
    dirty = [line for line in git(["status", "--porcelain"], workdir).stdout.splitlines() if line.strip()]
    if dirty:
        print(f"拒绝：{workdir} 有未提交改动（先提交或清理）")
        return 2
    original = git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip() if not host else ""
    steps = [["git", "fetch", "origin", base]]
    if not host:
        steps.append(["git", "checkout", branch])
    steps += [
        ["git", "merge", f"origin/{base}", "--no-edit"],
        ["git", "push", "origin", branch],
    ]
    for command in steps:
        result = git(command[1:], workdir)
        print(f"$ {' '.join(command[:3])} … rc={result.returncode}")
        if result.returncode != 0:
            if command[1] == "merge":
                conflicted = [
                    line for line in git(["diff", "--name-only", "--diff-filter=U"], workdir).stdout.splitlines()
                    if line.strip()
                ]
                print("合并冲突，已 abort（分支保持原样）：")
                for name in conflicted:
                    print(f"  {name}")
                git(["merge", "--abort"], workdir)
            if original:
                git(["checkout", original])
            print(result.stdout[-800:])
            return 1
    if original:
        git(["checkout", original])
    print(f"已把 {base} 合入 #{number}（{branch}）并推送")
    return 0


def dco(range_text: str) -> int:
    text = git(["log", "--format=%H%x1f%B%x1e", range_text]).stdout
    missing = missing_signoff(text)
    if missing:
        print(f"缺 Signed-off-by 的提交（{len(missing)} 个）：")
        for sha in missing:
            print(f"  {sha}")
        print("补救：git rebase --signoff <range>  （或后续提交用 git commit -s）")
        return 1
    print(f"DCO：{range_text} 范围内所有提交都有 Signed-off-by")
    return 0


def flaky(repo: str, limit: int) -> int:
    listing = gh(["run", "list", "--repo", repo, "--limit", str(limit), "--json",
                  "databaseId,headSha,workflowName,conclusion"])
    if listing.returncode != 0:
        print(listing.stdout[-1000:])
        return 1
    try:
        runs = json.loads(listing.stdout or "[]")
    except json.JSONDecodeError:
        print("gh 输出解析失败")
        return 1
    buckets: dict[tuple[str, str], list[dict]] = {}
    for run in runs:
        buckets.setdefault((run["headSha"], run.get("workflowName", "")), []).append(run)
    rows: list[dict] = []
    fetched = 0
    for (sha, workflow), group in buckets.items():
        if len(group) < 2 or fetched >= 10:
            continue
        for run in group:
            if fetched >= 10:
                break
            detail = gh(["run", "view", str(run["databaseId"]), "--repo", repo, "--json", "jobs"])
            fetched += 1
            if detail.returncode != 0:
                continue
            try:
                jobs = {job["name"]: job.get("conclusion") or "pending" for job in json.loads(detail.stdout)["jobs"]}
            except (json.JSONDecodeError, KeyError):
                continue
            rows.append({"sha": sha, "workflow": workflow, "run": run["databaseId"], "jobs": jobs})
    groups = flaky_groups(rows)
    if not groups:
        print(f"最近 {len(runs)} 次运行里，没有发现同提交结论不一致的 job")
        return 0
    for group in groups:
        print(f"{group['sha']}（{group['workflow']}，{group['retries']} 次运行）不稳定：")
        for name, conclusions in group["unstable"].items():
            print(f"  {name}: {', '.join(map(str, conclusions))}")
    return 0


def queue_status(repo: str) -> int:
    listing = gh(["api", f"repos/{repo}/rulesets"])
    if listing.returncode != 0:
        print("读不到 rulesets（可能没开 rulesets 或权限不足）：")
        print(listing.stdout[-500:])
        return 1
    try:
        rulesets = json.loads(listing.stdout or "[]")
    except json.JSONDecodeError:
        print("rulesets 解析失败")
        return 1
    found = []
    for item in rulesets:
        detail = gh(["api", f"repos/{repo}/rulesets/{item['id']}"])
        if detail.returncode != 0:
            continue
        try:
            rules = json.loads(detail.stdout).get("rules", [])
        except json.JSONDecodeError:
            continue
        if any(rule.get("type") == "merge_queue" for rule in rules):
            found.append(item.get("name", item["id"]))
    if found:
        print(f"merge queue：已开启（ruleset: {', '.join(map(str, found))}）")
        print("注意：之后合并要走队列（`gh pr merge --merge-queue`），普通 merge 会被拒。")
        return 0
    print("merge queue：未开启")
    print("开启前先想清楚：合并会变成排队执行，`gitflow merge` 需要改用 --merge-queue；")
    print("建议做法：新建 ruleset（target=branch, 只对 main），加 merge_queue 规则，然后在 PR 上试一次。")
    return 0


def diagnose(pr: str, repo: str) -> int:
    """把失败检查的**地址**逐条列出来：检查名 → 运行链接 → 失败日志尾部。"""
    view = gh(["pr", "view", pr, "--repo", repo, "--json", "number,title,headRefName,statusCheckRollup"])
    if view.returncode != 0:
        print(view.stdout[-1000:])
        return 1
    data = json.loads(view.stdout)
    print(f"#{data['number']} {data['title']}｜分支 {data['headRefName']}｜{repo}")
    bad = failed_checks(data.get("statusCheckRollup") or [])
    if not bad:
        print("没有失败检查（可能还在跑）")
        return 0
    for check in bad:
        print(f"- 失败：{check.get('name')} [{check.get('conclusion')}]")
        url = check.get("detailsUrl") or ""
        print(f"  地址：{url}")
        match = re.search(r"/runs/(\d+)", url)
        if match:
            logs = gh(["run", "view", match.group(1), "--repo", repo, "--log-failed"])
            tail = [line for line in logs.stdout.splitlines() if line.strip()][-20:]
            for line in tail:
                print("  | " + line[:300])
    return 1


def secret_scan(root: pathlib.Path, files: list[str]) -> list[str]:
    """返回命中说明；空列表表示干净。只扫**要点名提交的这几个文件**。"""
    hits: list[str] = []
    for name in files:
        path = root / name
        if path.name in SECRET_FILE_NAMES or path.name == ".env" or path.name.startswith(".env."):
            hits.append(f"{name}: 按文件名判为敏感文件")
            continue
        if not path.is_file():
            hits.append(f"{name}: 文件不存在")
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            hits.append(f"{name}: 读不了（{exc}）")
            continue
        lines = text.splitlines()
        for index, line in enumerate(lines):
            # 与 scripts/secret_scan.py 同口径：本行或上一行带显式豁免标记就跳过
            if ALLOW_MARKER in line or (index > 0 and ALLOW_MARKER in lines[index - 1]):
                continue
            for pattern, label in SECRET_PATTERNS:
                if pattern.search(line):
                    hits.append(f"{name}:{index + 1}: 命中 {label}")
    return hits


def base_ok(root: pathlib.Path, files: list[str]) -> list[str]:
    problems: list[str] = []
    if not files:
        problems.append("必须显式点名要提交的文件（--files）；不做 git add -A")
    for name in files:
        posix = pathlib.PurePosixPath(name)
        if ".." in posix.parts or posix.is_absolute() or pathlib.Path(name).is_absolute():
            problems.append(f"{name}: 不许跳出仓库")
    top = git(["rev-parse", "--show-toplevel"], root)
    if top.returncode != 0:
        problems.append("不在 git 仓库里")
    return problems


def plan(args: dict) -> list[list[str]]:
    """纯函数：把要执行的命令列出来（--dry-run 与自测共用同一份口径）。"""
    files = args["files"]
    commands: list[list[str]] = [["git", "add", "--", *files]]
    commands.append(["git", "commit", "-F", "<message-file>"])
    commands.append(["git", "push", "-u", args["remote"], args["branch"]])
    commands.append([
        "gh", "pr", "create", "--base", args["base"], "--head", args["branch"],
        "--title", args["title"], "--body-file", args["body_file"] or "<body>",
    ])
    if not args["no_merge"]:
        commands.append(["gh", "pr", "checks", "--repo", args["repo"], args["branch"], "--watch"])
        commands.append(["gh", "pr", "merge", "--repo", args["repo"], args["branch"], "--squash", "--delete-branch"])
    return commands


def publish(args: dict) -> int:
    problems = base_ok(ROOT, args["files"])
    if not problems:
        problems += repo_problems(args["repo"])
        if args["branch"]:
            problems += branch_problems(args["branch"])
        outside = staged_outside(ROOT, args["files"])
        if outside:
            problems.append(
                "索引里已有不在本次列表的文件（提交会带上它们）：" + ", ".join(outside)
            )
    hits = secret_scan(ROOT, args["files"]) if not problems else []
    if problems or hits:
        for line in problems + hits:
            print(f"拒绝：{line}")
        return 1
    if args["dry_run"]:
        print("dry-run：将执行")
        for command in plan(args):
            print("  " + " ".join(command))
        return 0

    current = git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip()
    branch = args["branch"] or ("" if current in ("main", "master") else current)
    if not branch:
        print("拒绝：当前在主干上，必须给 --branch（不做直接在 main 上提交）")
        return 1
    if branch == current:
        pass
    elif branch in git(["branch", "--list", branch]).stdout:
        print(f"拒绝：分支已存在：{branch}")
        return 1
    else:
        created = git(["checkout", "-b", branch])
        if created.returncode != 0:
            print(created.stdout[-1000:])
            return 1

    message = args["title"] + "\n"
    if args["body_file"]:
        body = (ROOT / args["body_file"]).read_text(encoding="utf-8")
        message += "\n" + body
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".txt", delete=False) as handle:
        handle.write(message)
        message_file = handle.name

    existing = gh(["pr", "list", "--repo", args["repo"], "--head", branch, "--state", "open", "--json", "number"])
    has_pr = existing.returncode == 0 and existing.stdout.strip() not in ("", "[]")
    steps = [
        ["git", "add", "--", *args["files"]],
        ["git", "commit", "-F", message_file],
        ["git", "push", "-u", args["remote"], branch],
    ]
    if not has_pr:
        steps.append([
            "gh", "pr", "create", "--repo", args["repo"], "--base", args["base"], "--head", branch,
            "--title", args["title"], "--body-file", args["body_file"] or message_file,
        ])
    else:
        print("该分支已有 open PR：跳过 create，直接等 CI")
    try:
        for command in steps:
            result = git(command[1:]) if command[0] == "git" else gh(command[1:])
            print(f"$ {' '.join(command[:3])} … rc={result.returncode}")
            if result.returncode != 0:
                print(result.stdout[-2000:])
                return 1
    finally:
        pathlib.Path(message_file).unlink(missing_ok=True)
    if args["no_merge"]:
        print("已开 PR（--no-merge）：等 CI，绿了自己合或再来一次 gitflow merge")
        return 0
    return watch_and_merge(branch, args["repo"])


def checks_not_yet(output: str) -> bool:
    """检查还没注册时 gh 会说 no checks reported——这是竞态，不是失败。"""
    lowered = output.lower()
    return "no checks reported" in lowered or "no checks" in lowered


def wait_for_checks(repo: str, branch: str, attempts: int = 40, pause: int = 15) -> int:
    """等检查**注册**（不是等它们绿）：pending 时 gh 返回非零(8)，那也算已注册。

    真正的通过/失败交给后面的 `gh pr checks --watch` 判。
    """
    for index in range(attempts):
        listing = gh(["pr", "checks", "--repo", repo, branch])
        output = listing.stdout or ""
        if output.strip() and not checks_not_yet(output):
            return 0
        if index == 0:
            print("检查还没注册，先等它们出现…")
        time.sleep(pause)
    print("等检查出现超时（10 分钟）")
    return 1


def has_pending(output: str) -> bool:
    lowered = output.lower()
    return "pending" in lowered or "in_progress" in lowered or "queued" in lowered


def watch_and_merge(branch: str, repo: str, attempts: int = 60, pause: int = 30) -> int:
    """等检查**真正**落定再合。

    只靠 `gh pr checks --watch` 不够：push 之后新的 run 还没注册时，它会拿着上一轮的
    结果返回——然后 merge 被"requirements have not been met"拒掉（实跑踩过）。
    所以这里再轮询到没有 pending，并且失败时重试合并。
    """
    if wait_for_checks(repo, branch) != 0:
        return 1
    checks = gh(["pr", "checks", "--repo", repo, branch, "--watch"])
    if checks.returncode != 0 and not has_pending(checks.stdout or ""):
        print("CI 未通过：")
        print(checks.stdout[-2000:])
        return 1
    for index in range(attempts):
        listing = gh(["pr", "checks", "--repo", repo, branch])
        output = listing.stdout or ""
        if not output.strip() or has_pending(output):
            if index == 0:
                print("检查还在跑（可能是刚 push 触发的新一轮），等它落定…")
            time.sleep(pause)
            continue
        merge = gh(["pr", "merge", "--repo", repo, branch, "--squash", "--delete-branch"])
        if merge.returncode == 0:
            print(f"CI 全绿，已 squash 合入 {branch} 并删分支")
            return 0
        if index == attempts - 1:
            print("合并失败：")
            print(merge.stdout[-1500:])
            return 1
        time.sleep(pause)
    print(f"合并超时（约 {attempts * pause // 60} 分钟）")
    return 1


def status(repo: str) -> int:
    branch = git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip()
    dirty = git(["status", "--porcelain"]).stdout.strip().splitlines()
    prs = gh(["pr", "list", repo, "--limit", "5", "--json", "number,title,state,mergeStateStatus"])
    print(f"分支：{branch}｜未提交改动：{len(dirty)} 个文件")
    if prs.returncode == 0 and prs.stdout.strip():
        for pr in json.loads(prs.stdout):
            print(f"  #{pr['number']} {pr['title'][:60]} [{pr['state']}/{pr['mergeStateStatus']}]")
    return 0


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
        (root / "leak.py").write_text('KEY = "sk' + '-abcdefghijklmnopqrstu"\n', encoding="utf-8")
        (root / ".env").write_text("A=1\n", encoding="utf-8")
        (root / "key.pem").write_text("-----BEGIN " + "RSA PRIVATE KEY-----\n", encoding="utf-8")

        check("干净文件不报", secret_scan(root, ["clean.py"]), [])
        check("sk- 形状命中", any("sk-" in h for h in secret_scan(root, ["leak.py"])), True)
        check(".env 按文件名命中", any(".env" in h for h in secret_scan(root, [".env"])), True)
        check("私钥头命中", any("私钥" in h for h in secret_scan(root, ["key.pem"])), True)
        (root / "allowed.py").write_text(
            'KEY = "sk' + '-abcdefghijklmnopqrstu"  # secret-scan: allow 夹具' + chr(10), encoding="utf-8"
        )
        check("显式豁免标记被认", secret_scan(root, ["allowed.py"]), [])
        (root / ".env.production").write_text("A=1\n", encoding="utf-8")
        check(".env.* 也按敏感文件拒收",
              any(".env.production" in h for h in secret_scan(root, [".env.production"])), True)
        check("POSIX 绝对路径被拒", any("跳出仓库" in p for p in base_ok(root, ["/etc/passwd"])), True)
        check("不存在的文件要报", any("不存在" in h for h in secret_scan(root, ["nope.txt"])), True)

        args = {
            "title": "t", "body_file": "body.md", "files": ["a.py", "b.py"],
            "branch": "feat/x", "base": "main", "no_merge": False, "repo": "o/r", "dry_run": True,
            "remote": "origin",
        }
        commands = plan(args)
        check("第一条是显式 add", commands[0], ["git", "add", "--", "a.py", "b.py"])
        check("有 push", any(c[:2] == ["git", "push"] for c in commands), True)
        check("有开 PR", any(c[:3] == ["gh", "pr", "create"] for c in commands), True)
        check("默认绿即合", any(c[:3] == ["gh", "pr", "merge"] for c in commands), True)
        args["no_merge"] = True
        check("--no-merge 不出合并", any(c[:3] == ["gh", "pr", "merge"] for c in plan(args)), False)
        check("不做 git add -A", any("-A" in c for c in plan(args)), False)
        check("SSH remote 解析", repo_from_url("git@github.com:bfxh/trit.git"), "bfxh/trit")
        check("HTTPS remote 解析", repo_from_url("https://github.com/bfxh/trit-immune"), "bfxh/trit-immune")
        check("认不出的 remote 为空", repo_from_url("file:///tmp/x"), "")
        check("失败检查只挑失败的",
              [c["name"] for c in failed_checks([
                  {"name": "ok", "conclusion": "SUCCESS"},
                  {"name": "skip", "conclusion": "SKIPPED"},
                  {"name": "bad", "conclusion": "FAILURE"},
                  {"name": "run", "conclusion": None},
              ])], ["bad"])
        check("分支名以 - 开头被拒", bool(branch_problems("-x")), True)
        check("分支名含 .. 被拒", bool(branch_problems("feat/../x")), True)
        check("分支名带空格被拒", bool(branch_problems("feat x")), True)
        check("正常分支名放行", branch_problems("feat/x-1"), [])
        check("仓库名要 owner/name", bool(repo_problems("trit")), True)
        check("正常仓库名放行", repo_problems("bfxh/trit"), [])
        check("ahead/behind 解析", parse_ahead_behind("2\t5"), (2, 5))
        check("ahead/behind 脏输入回 0", parse_ahead_behind("oops"), (0, 0))
        check(
            "可清理分支：只挑已并入且非当前/非保护",
            prune_candidates(["a", "b", "main", "cur"], {"a", "main", "cur"}, "cur", {"main"}),
            ["a"],
        )
        stale = mark_stale(
            [
                {"name": "old-merged", "merged": True, "last_commit": 0},
                {"name": "new-merged", "merged": True, "last_commit": 1_999_996_400},
                {"name": "old-open", "merged": False, "last_commit": 0},
            ],
            now=2_000_000_000,
            stale_days=14,
        )
        check("陈旧只标已并入的", [r["name"] for r in stale if r["stale"]], ["old-merged"])
        check("陈旧排在最前", stale[0]["name"], "old-merged")
        check("分支名转目录名", sanitize("feat/x-y"), "feat-x-y")
        wt = worktree_path_for(pathlib.Path("/repo/trit"), "feat/x")
        check("worktree 放在仓外", wt.name == "trit-feat-x" and wt.parent.name == "repo", True)
        sample = "worktree /a/b\nHEAD abc\nbranch refs/heads/feat/x\n\nworktree /a/c\nHEAD def\ndetached\n"
        rows = parse_worktrees(sample)
        check("worktree 解析条数", len(rows), 2)
        check("worktree 解析分支", rows[0]["branch"], "feat/x")
        prs = [
            {"number": 1, "title": "a", "headRefName": "feat/a", "baseRefName": "main"},
            {"number": 2, "title": "b", "headRefName": "feat/b", "baseRefName": "feat/a"},
            {"number": 3, "title": "c", "headRefName": "feat/c", "baseRefName": "feat/missing"},
        ]
        chains, off = pr_chains(prs)
        check("链长度", [len(c) for c in chains], [2, 1])
        check("离干 PR 被标出", sorted(p["headRefName"] for p in off), ["feat/b", "feat/c"])
        check("竞态识别：no checks reported", checks_not_yet("no checks reported on the 'x' branch"), True)
        check("真实失败不算竞态", checks_not_yet("some check failed"), False)
        check("pending 识别", has_pending("CI\tpending\t0\thttps://x"), True)
        check("通过不算 pending", has_pending("CI\tpass\t1s\thttps://x"), False)
        check("文件重叠 Jaccard", round(file_jaccard(["a", "b"], ["a", "b", "c"]), 6), 0.666667)
        now = 2_000_000_000
        sample = [
            {"number": 1, "isDraft": False, "updatedAt": "2033-05-18T03:00:00Z", "mergeable": "MERGEABLE",
             "statusCheckRollup": [{"name": "CI"}], "files": [{"path": "a"}]},
            {"number": 2, "isDraft": True, "updatedAt": "2033-05-18T03:00:00Z", "mergeable": "MERGEABLE",
             "statusCheckRollup": [{"name": "CI"}], "files": [{"path": "b"}]},
            {"number": 3, "isDraft": False, "updatedAt": "2020-01-01T00:00:00Z", "mergeable": "CONFLICTING",
             "statusCheckRollup": [], "files": [{"path": "x"}]},
        ]
        labels1 = classify_pr(sample[0], now, 14, ["CI"], sample[1:])
        check("正常 PR 无标签", labels1, [])
        check("草稿被标", classify_pr(sample[1], now, 14, ["CI"], [sample[0], sample[2]]), ["draft"])
        labels3 = classify_pr(sample[2], now, 14, ["CI"], [sample[0], sample[1]])
        check("陈旧+冲突+缺检查", set(labels3) >= {"stale", "conflicting", "missing-required-check"}, True)
        duplicate = {"number": 9, "isDraft": False, "updatedAt": "2033-05-18T03:00:00Z",
                     "mergeable": "MERGEABLE", "statusCheckRollup": [{"name": "CI"}], "files": [{"path": "a"}]}
        check("重复 PR 被标", classify_pr(duplicate, now, 14, ["CI"], [sample[0]]), ["superseded-by-#1"])
        sep, end = chr(31), chr(30)
        sample_log = f"aaa111{sep}feat: x{chr(10)}Signed-off-by: t <t@e>{end}bbb222{sep}fix: y{end}"
        check("DCO 找出缺签名的提交", missing_signoff(sample_log), ["bbb222"])
        rows = [
            {"sha": "same", "workflow": "CI", "run": 1, "jobs": {"CI": "success", "t3": "success"}},
            {"sha": "same", "workflow": "CI", "run": 2, "jobs": {"CI": "failure", "t3": "success"}},
            {"sha": "other", "workflow": "CI", "run": 3, "jobs": {"CI": "success"}},
            {"sha": "other", "workflow": "CI", "run": 4, "jobs": {"CI": "success"}},
            # 不同 workflow 的 job 集合不同，不该互相比出 "missing"
            {"sha": "same", "workflow": "t3", "run": 5, "jobs": {"t3 fmt + clippy + test": "success"}},
        ]
        groups = flaky_groups(rows)
        check("flake 只报不一致的 job", [(g["sha"], list(g["unstable"])) for g in groups], [("same", ["CI"])])
        check("默认 base 优先 main", pick_base(["main", "master"]), "main")
        check("只有 master 时用 master", pick_base(["master"]), "master")
        check("都没有时回落 main", pick_base([]), "main")

        # 保护规则一键对账（本地台账 → GitHub）：读空不许当成"撤掉保护"
        checks_file = root / "checks.txt"
        checks_file.write_text("# 注释\n\nMSRV (1.88)\n  wasm (wasm32 build)  \n", encoding="utf-8")
        check("必需检查台账读正文", read_required_checks(checks_file),
              ["MSRV (1.88)", "wasm (wasm32 build)"])
        check("台账不存在算空清单", read_required_checks(root / "nope.txt"), [])
        check("PUT 前剥掉只读 url", strip_urls({"url": "x", "a": {"url": "y", "b": [1]}}),
              {"a": {"b": [1]}})
        bare = build_protection_body(
            {"required_status_checks": {"strict": True, "contexts": ["CI"], "url": "https://api/x"},
             "url": "https://api/y",
             "enforce_admins": {"enabled": True, "url": "https://api/z"},
             "restrictions": {"url": "https://api/r", "users": ["a"]}},
            ["CI", "新门"])
        check("payload 里不许有任何只读 url（GET 原样送回必 422）", has_url_key(bare), False)
        check("payload 必须带 restrictions 键（缺了也 422）", "restrictions" in bare, True)
        check("enforce_admins 要布尔不是对象", bare["enforce_admins"], True)
        check("要推的清单原样进 contexts", bare["required_status_checks"]["contexts"], ["CI", "新门"])
        check("没有 restrictions 时也显式给 null", build_protection_body({}, ["CI"])["restrictions"], None)
        check("空清单拒推（读空不等于撤保护）",
              apply_required_checks("o/r", "main", {}, [], True), 1)
        check("dry-run 只打印不碰远端",
              apply_required_checks("o/r", "main", {"required_status_checks":
                                                    {"strict": True, "contexts": ["CI"]}}, ["CI"], True), 0)
        check("新增必需检查时先过拦路检查（无网络就拒，不许盲推）",
              apply_required_checks("o/r", "main", {"required_status_checks":
                                                    {"strict": True, "contexts": []}}, ["CI"], True), 1)
        check("写完复核：一致没话说", diff_contexts(["CI", "t3"], ["t3", "CI"]), [])
        check("写完复核：少了要点名", diff_contexts(["CI"], ["CI", "t3"]), ["少 t3"])
        check("写完复核：多了也要点名", diff_contexts(["CI", "旧门"], ["CI"]), ["多 旧门"])

        # 推保护之前先看开着的 PR 会不会被卡住（本仓差点把两个新门推上去卡住三条 PR）
        ci = load_ci_integrity()
        check("只对本次新增的必需检查负责", delta_contexts(["CI", "t3"], ["CI", "t3", "新门"]), ["新门"])
        jobs_ok = [{"workflow": "ci.yml", "id": "surface", "name": "test surface (ratchet)",
                    "if": "", "on_pr": True, "has_matrix": False}]
        jobs_missing = [{"workflow": "ci.yml", "id": "old", "name": "old job",
                         "if": "", "on_pr": True, "has_matrix": False}]
        check("分支上有这个 job ⇒ 不缺", missing_on_branch(jobs_ok, ["test surface (ratchet)"], ci), [])
        check("分支上没这个 job ⇒ 缺",
              missing_on_branch(jobs_missing, ["test surface (ratchet)"], ci), ["test surface (ratchet)"])
        check("取不到 workflow 按全缺算（证明不了就不许放行）",
              missing_on_branch(None, ["a", "b"], ci), ["a", "b"])
        check("矩阵展开名也要能对上",
              missing_on_branch([{"workflow": "ci.yml", "id": "x", "name": "fmt + clippy + test",
                                  "if": "", "on_pr": True, "has_matrix": True}],
                                ["fmt + clippy + test (ubuntu-latest)"], ci), [])
        prs = [{"number": 7, "headRefName": "feat/a"}, {"number": 8, "headRefName": "feat/b"}]
        check("只点名会卡住的 PR",
              protection_blockers(prs, {"feat/a": jobs_ok, "feat/b": jobs_missing},
                                  ["test surface (ratchet)"], ci),
              ["#8（feat/b）不会上报：test surface (ratchet)"])
        check("没有新增就没有拦路", protection_blockers(prs, {"feat/a": None, "feat/b": None}, [], ci), [])
        check("读不到 workflow 要单独点名（不许混成\"确实没这个 job\"）",
              protection_blockers([{"number": 9, "headRefName": "feat/z"}], {"feat/z": None},
                                  ["新门"], ci),
              ["#9（feat/z）读不到 workflow ⇒ 无法证明会跑：新门"])
        legacy = (
            "Auto-merging a.txt\n"
            "changed in both\n"
            "  base   100644 " + "1" * 40 + " a.txt\n"
            "  our    100644 " + "2" * 40 + " a.txt\n"
            "  their  100644 " + "3" * 40 + " a.txt\n"
            "\n"
            "merged\n"
            "changed in both\n"
            "  base   100644 " + "4" * 40 + " b/c.txt\n"
            "  our    100644 " + "5" * 40 + " b/c.txt\n"
            "  their  100644 " + "6" * 40 + " b/c.txt\n"
        )
        check("冲突预览点名到文件", merge_tree_conflicts(legacy), ["a.txt", "b/c.txt"])
        check("没有冲突块就不点名", merge_tree_conflicts("merged\n  our 100644 a.txt\n"), [])
        check("预览优先 write-tree：能自动合并的不算冲突",
              merge_preview(0, "d" * 40 + "\nAuto-merging a.txt\n", ""), (False, [], "write-tree"))
        check("write-tree 报冲突时点名文件",
              merge_preview(1, "d" * 40 + "\nsrc/x.rs\nCONFLICT (content): Merge conflict in src/x.rs\n",
                            ""), (True, ["src/x.rs"], "write-tree"))
        check("write-tree 不可用才退回老口径",
              merge_preview(129, "usage: git merge-tree",
                            "changed in both\n  our    100644 " + "1" * 40 + " b/y.rs\n  their  100644 "
                            + "2" * 40 + " b/y.rs\n"),
              (True, ["b/y.rs"], "legacy"))
        check("write-tree 干净但列了文件名也算冲突",
              merge_preview(0, "d" * 40 + "\nsrc/y.rs\n", "")[0], True)
        check("write-tree rc≠0 但没列文件也算冲突（只看文件名会漏）",
              merge_preview(1, "d" * 40 + "\n", "")[0], True)
        check("has_url_key 是递归查的", has_url_key({"a": {"b": [{"url": "x"}]}}), True)

        problems = base_ok(root, [])
        check("空文件列表被拒（点名 --files）", any("--files" in p for p in problems), True)
        check("跳出仓库被拒", any("跳出仓库" in p for p in base_ok(root, ["../x"])), True)

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
    command = argv[0] if argv else "status"
    repo = argv[argv.index("--repo") + 1] if "--repo" in argv else detect_repo()
    remote = argv[argv.index("--remote") + 1] if "--remote" in argv else detect_remote()
    base = argv[argv.index("--base") + 1] if "--base" in argv else detect_base(remote)
    if command == "status":
        return status(repo)
    files: list[str] = []
    if "--files" in argv:
        index = argv.index("--files") + 1
        while index < len(argv) and not argv[index].startswith("--"):
            files.append(argv[index])
            index += 1
    if command == "secret-scan":
        hits = secret_scan(ROOT, files)
        for line in hits:
            print(f"命中：{line}")
        return 1 if hits else 0
    if command == "branch":
        return branch_cmd(repo, argv[1:])
    if command == "worktree":
        sub = argv[1] if len(argv) > 1 else "list"
        rest = [a for a in argv[2:] if not a.startswith("--")]
        if sub == "list":
            return worktree_list()
        if sub == "add":
            if not rest:
                print("用法：gitflow.py worktree add <分支> [--path 目录] [--from main]")
                return 2
            base = argv[argv.index("--from") + 1] if "--from" in argv else detect_base(detect_remote())
            path = argv[argv.index("--path") + 1] if "--path" in argv else ""
            return worktree_add(rest[0], base, detect_remote(), path, "--dry-run" in argv)
        if sub == "remove":
            if not rest:
                print("用法：gitflow.py worktree remove <目录> [--force]")
                return 2
            return worktree_remove(rest[0], "--force" in argv)
        print(f"未知 worktree 子命令: {sub}（list / add / remove）")
        return 2
    if command == "stack":
        return stack(repo, remote_state="--no-remote" not in argv)
    if command == "dco":
        default_range = f"{detect_remote()}/{detect_base(detect_remote())}..HEAD"
        return dco(argv[argv.index("--range") + 1] if "--range" in argv else default_range)
    if command == "flaky":
        limit = int(argv[argv.index("--limit") + 1]) if "--limit" in argv else 20
        return flaky(repo, limit)
    if command == "queue":
        return queue_status(repo)
    if command == "pr":
        sub = argv[1] if len(argv) > 1 else "triage"
        rest = [a for a in argv[2:] if not a.startswith("--")]
        if sub == "triage":
            stale_days = int(argv[argv.index("--stale-days") + 1]) if "--stale-days" in argv else 14
            return pr_triage(repo, stale_days, "--json" in argv, "--apply" in argv)
        if sub == "sync":
            if not rest:
                print("用法：gitflow.py pr sync <编号> [--dry-run]")
                return 2
            return pr_sync(repo, rest[0], dry_run="--dry-run" in argv)
        print(f"未知 pr 子命令: {sub}（triage / sync）")
        return 2
    if command == "merge":
        target = argv[argv.index("--branch") + 1] if "--branch" in argv else ""
        if not target:
            print("用法：gitflow.py merge --branch <分支> [--wait-minutes N]")
            return 2
        wait = int(argv[argv.index("--wait-minutes") + 1]) if "--wait-minutes" in argv else 30
        return watch_and_merge(target, repo, attempts=max(1, wait * 2), pause=30)
    if command == "diagnose":
        pr = argv[argv.index("--pr") + 1] if "--pr" in argv else ""
        if not pr:
            print("用法：gitflow.py diagnose --pr <编号>")
            return 2
        return diagnose(pr, repo)
    if command == "publish":
        args = {
            "title": argv[argv.index("--title") + 1] if "--title" in argv else "",
            "body_file": argv[argv.index("--body-file") + 1] if "--body-file" in argv else "",
            "files": files,
            "branch": argv[argv.index("--branch") + 1] if "--branch" in argv else "",
            "base": base,
            "remote": remote,
            "repo": repo,
            "no_merge": "--no-merge" in argv,
            "dry_run": "--dry-run" in argv,
        }
        if not args["title"]:
            print("publish 需要 --title")
            return 2
        return publish(args)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
