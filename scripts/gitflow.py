#!/usr/bin/env python3
"""gitflow：一句话把改动送达 GitHub——提交 / 推送 / 开 PR / 看 CI / 绿即合。

用法：
  python scripts/gitflow.py publish --title "标题" [--body-file 正文.md] --files a b c
                                     [--branch 分支名] [--base main] [--no-merge] [--dry-run]
  python scripts/gitflow.py secret-scan --files a b c
  python scripts/gitflow.py status
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


def failed_checks(rollup: list[dict]) -> list[dict]:
    """从 PR 的 statusCheckRollup 里挑出失败的；成功/跳过/未完成都不算失败。"""
    bad: list[dict] = []
    for check in rollup:
        conclusion = (check.get("conclusion") or "").upper()
        if conclusion in ("SUCCESS", "SKIPPED", "NEUTRAL", ""):
            continue
        bad.append(check)
    return bad


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
        for pattern, label in SECRET_PATTERNS:
            if pattern.search(text):
                hits.append(f"{name}: 命中 {label}")
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


def watch_and_merge(branch: str, repo: str) -> int:
    checks = gh(["pr", "checks", "--repo", repo, branch, "--watch"])
    if checks.returncode != 0:
        print("CI 未通过：")
        print(checks.stdout[-2000:])
        return 1
    merge = gh(["pr", "merge", "--repo", repo, branch, "--squash", "--delete-branch"])
    if merge.returncode != 0:
        print("合并失败：")
        print(merge.stdout[-1000:])
        return 1
    print(f"CI 全绿，已 squash 合入 {branch} 并删分支")
    return 0


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
        check("默认 base 优先 main", pick_base(["main", "master"]), "main")
        check("只有 master 时用 master", pick_base(["master"]), "master")
        check("都没有时回落 main", pick_base([]), "main")

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
