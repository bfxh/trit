"""仓库体检：一次把社区里最常"踩坑"的几类问题列出来，并给可执行修复命令。

用法：
  python scripts/repo_doctor.py              # 本地检查
  python scripts/repo_doctor.py --remote     # 再加 GitHub 侧（保护规则/合并策略/PR 状态）
  python scripts/repo_doctor.py --json
  python scripts/repo_doctor.py --self-test

检查项（每项都有"怎么修"）：
  行尾        受版本控制的文本文件里 CRLF 的比例（Windows 上的经典坑）
  大文件      当前被跟踪的 > 5 MiB 文件（历史里的大文件请用 git-filter-repo）
  构建产物    是否把 target/ 之类提交进去了
  子模块      .gitmodules 里未初始化 / 与远端不一致的子模块
  CODEOWNERS  * 兜底之后仍覆盖不到的关键目录
  分支        已并入 main 且陈旧的本地分支数量（用 gitflow branch prune 清）
  必需检查    与 workflow 是否对得上（复用 ci_integrity）
  密钥        高置信度密钥（复用 secret_scan）
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BIG_FILE = 5 * 1024 * 1024
BUILD_DIRS = ("target/", "node_modules/", "dist/", "build/")


def run(args: list[str], cwd: pathlib.Path) -> str:
    result = subprocess.run(
        ["git", *args], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, encoding="utf-8", errors="replace",
    )
    return result.stdout if result.returncode == 0 else ""


def load_tool(name: str):
    path = pathlib.Path(__file__).resolve().parent / f"{name}.py"
    if not path.is_file():
        return None
    spec = importlib.util.spec_from_file_location(f"doctor_{name}", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[f"doctor_{name}"] = module
    spec.loader.exec_module(module)
    return module


def tracked_files(root: pathlib.Path) -> list[str]:
    return [line for line in run(["ls-files"], root).splitlines() if line.strip()]


def eol_report(root: pathlib.Path) -> tuple[list[str], int]:
    """(索引里真的存了 CRLF 的文件, 本机工作树是 CRLF 但索引是 LF 的文件数)。

    只看工作树会误报：Windows 上 `core.autocrlf=true` 本来就把 LF 检出成 CRLF。
    真正的仓库问题是 `git ls-files --eol` 里 **i/crlf**。
    """
    index_crlf: list[str] = []
    worktree_only = 0
    for line in run(["ls-files", "--eol"], root).splitlines():
        parts = line.split("\t", 1)
        if len(parts) != 2:
            continue
        flags, name = parts[0], parts[1]
        if "i/crlf" in flags:
            index_crlf.append(name)
        elif "w/crlf" in flags:
            worktree_only += 1
    return index_crlf, worktree_only


def parse_gitmodules(text: str) -> list[dict]:
    modules: list[dict] = []
    current: dict = {}
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("[submodule"):
            if current:
                modules.append(current)
            current = {}
        elif "=" in line:
            key, _, value = line.partition("=")
            current[key.strip()] = value.strip().strip('"')
    if current:
        modules.append(current)
    return modules


def codeowners_gaps(text: str, required_dirs: list[str]) -> list[str]:
    patterns = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        patterns.append(line.split()[0])
    covered = set()
    for pattern in patterns:
        for directory in required_dirs:
            clean = pattern.lstrip("/").rstrip("*").rstrip("/")
            if clean and (directory == clean or directory.startswith(clean + "/")):
                covered.add(directory)
    return [directory for directory in required_dirs if directory not in covered]


def diagnose(root: pathlib.Path, remote: bool) -> list[dict]:
    findings: list[dict] = []

    def add(kind: str, level: str, detail: str, fix: str) -> None:
        findings.append({"kind": kind, "level": level, "detail": detail, "fix": fix})

    files = tracked_files(root)
    if not files:
        return [{"kind": "repo", "level": "high", "detail": "不是 git 仓库或没有跟踪文件", "fix": "先 git init"}]

    if not (root / ".gitattributes").is_file():
        add("行尾", "high", "缺少根 .gitattributes（Windows/macOS 会各写各的换行）",
            "新建 .gitattributes：`* text=auto eol=lf` + 二进制标记")
    index_crlf, worktree_only = eol_report(root)
    if index_crlf:
        add("行尾", "high", f"{len(index_crlf)} 个文件的**索引里**存了 CRLF（例：{', '.join(index_crlf[:3])}）",
            "git add --renormalize . && git commit -m 'normalize eol'")
    elif worktree_only:
        add("行尾", "low", f"本机工作树有 {worktree_only} 个 CRLF 文件，但索引是 LF（checkout 设置导致，不是仓库问题）",
            "无需修；要本地也统一可设 core.autocrlf=input")

    big = []
    for name in files:
        path = root / name
        try:
            if path.is_file() and path.stat().st_size > BIG_FILE:
                big.append((name, path.stat().st_size))
        except OSError:
            continue
    for name, size in sorted(big, key=lambda item: -item[1])[:5]:
        add("大文件", "medium", f"{name}（{size / 1024 / 1024:.1f} MiB）",
            "用 Git LFS，或 git-filter-repo 从历史里移除")
    if big:
        add("大文件", "low", f"共 {len(big)} 个 > 5 MiB 的受控文件", "评估是否转 LFS")

    tracked_build = [name for name in files if any(name.startswith(prefix) for prefix in BUILD_DIRS)]
    if tracked_build:
        add("构建产物", "high", f"{len(tracked_build)} 个构建产物被提交（例：{tracked_build[0]}）",
            "git rm -r --cached <目录> && 写进 .gitignore")

    if (root / ".gitmodules").is_file():
        modules = parse_gitmodules((root / ".gitmodules").read_text(encoding="utf-8"))
        status = run(["submodule", "status"], root)
        dirty = [line for line in status.splitlines() if line.startswith(("-", "+", "U"))]
        if dirty:
            add("子模块", "medium", f"{len(dirty)} 个子模块未初始化或指向不一致",
                "git submodule update --init --recursive（+ 的表示远端已前进）")
        if modules:
            add("子模块", "low", f"仓库里有 {len(modules)} 个子模块（拉取/CI 都要额外一步）",
                "考虑 vendor 或改成包依赖")

    owners = root / ".github" / "CODEOWNERS"
    owner_text = owners.read_text(encoding="utf-8") if owners.is_file() else (root / "CODEOWNERS").read_text(encoding="utf-8") if (root / "CODEOWNERS").is_file() else ""
    crates = [p.name for p in (root / "crates").iterdir() if p.is_dir()] if (root / "crates").is_dir() else []
    if not owner_text:
        add("CODEOWNERS", "medium", "没有 CODEOWNERS", "新建 .github/CODEOWNERS：`* @owner` + 关键目录")
    elif crates:
        gaps = codeowners_gaps(owner_text, [f"crates/{name}" for name in crates])
        if gaps:
            add("CODEOWNERS", "low", f"{len(gaps)} 个 crate 没被显式覆盖（例：{gaps[0]}）", "在 CODEOWNERS 补一行")

    base = "main" if run(["rev-parse", "--verify", "refs/remotes/origin/main"], root) else "master"
    merged = {line.strip().lstrip("* ").strip() for line in run(["branch", "--merged", base], root).splitlines() if line.strip()}
    names = [line.strip().lstrip("* ").strip() for line in run(["branch", "--format=%(refname:short)"], root).splitlines() if line.strip()]
    current = run(["rev-parse", "--abbrev-ref", "HEAD"], root).strip()
    stale = [name for name in names if name in merged and name not in (current, base, "master")]
    if stale:
        add("分支", "low", f"{len(stale)} 个已并入 {base} 的本地分支（例：{stale[0]}）",
            "python scripts/gitflow.py branch prune --apply")

    secrets = load_tool("secret_scan")
    if secrets is not None and hasattr(secrets, "scan"):
        hits = secrets.scan(root, 2)
        if hits:
            add("密钥", "high", f"{len(hits)} 处高置信度密钥（{hits[0]['file']}:{hits[0]['line']}）",
                "轮换密钥；历史里出现过的用 git-filter-repo 清除")

    if remote:
        integrity = load_tool("ci_integrity")
        if integrity is not None and hasattr(integrity, "validate"):
            required = integrity.read_required(root)
            problems = integrity.validate(integrity.parse_workflows(root), required, integrity.remote_required("bfxh/trit"))
            for problem in problems:
                add("必需检查", "high", problem, "按 .github/required-checks.txt 与 workflow 对齐")
    return findings


def self_test() -> int:
    failures: list[str] = []
    total: list[int] = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    modules = parse_gitmodules(
        '[submodule "a"]\n\tpath = vendor/a\n\turl = https://x/a\n\n[submodule "b"]\n\tpath = vendor/b\n\turl = https://x/b\n'
    )
    check("解析 .gitmodules 条数", len(modules), 2)
    check("解析 path", modules[0].get("path"), "vendor/a")

    owners = "# 注释\n* @o\n/crates/core/ @o\n/scripts/ @o\n"
    check("CODEOWNERS 缺口", codeowners_gaps(owners, ["crates/core", "crates/new", "scripts"]), ["crates/new"])
    check("* 兜底不算覆盖具体目录", codeowners_gaps("* @o\n", ["crates/a"]), ["crates/a"])

    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        subprocess.run(["git", "init", "-q"], cwd=root)
        subprocess.run(["git", "config", "user.email", "t@example.com"], cwd=root)
        subprocess.run(["git", "config", "user.name", "t"], cwd=root)
        (root / "a.txt").write_bytes(b"line\r\n")
        (root / "target").mkdir()
        (root / "target" / "junk.bin").write_bytes(b"junk")
        subprocess.run(["git", "add", "-A"], cwd=root)
        findings = diagnose(root, remote=False)
        kinds = {finding["kind"] for finding in findings}
        check("缺 .gitattributes 会被报", "行尾" in kinds, True)
        # 索引侧 LF、工作树 CRLF 时只能算 low（本机 artifact）
        subprocess.run(["git", "add", "--renormalize", "."], cwd=root)
        subprocess.run(["git", "commit", "-qm", "init"], cwd=root)
        (root / "b.txt").write_bytes(b"x\r\n")
        subprocess.run(["git", "add", "-f", "b.txt"], cwd=root)
        levels = {finding["kind"]: finding["level"] for finding in diagnose(root, remote=False)}
        check("工作树 CRLF 不判 high", levels.get("行尾"), "low")
        check("构建产物会被报", "构建产物" in kinds, True)
        check("每条发现都有修复命令", all(finding["fix"] for finding in findings), True)
        tab = chr(9)
        fake = (
            "i/crlf          w/crlf          attr/" + tab + "a.txt" + chr(10) +
            "i/lf            w/crlf          attr/text=auto eol=lf" + tab + "b.txt" + chr(10)
        )
        original_run = globals()["run"]
        globals()["run"] = lambda args, cwd: fake
        try:
            index_crlf, worktree_only = eol_report(root)
        finally:
            globals()["run"] = original_run
        check("索引 CRLF 被抓出来", index_crlf, ["a.txt"])
        check("只工作树是 CRLF 的只计数", worktree_only, 1)

        empty = pathlib.Path(tmp) / "empty"
        empty.mkdir()
        check("非 git 目录会说清楚", diagnose(empty, remote=False)[0]["kind"], "repo")

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
    findings = diagnose(root, remote="--remote" in argv)
    if "--json" in argv:
        print(json.dumps(findings, ensure_ascii=False, indent=1))
        return 1 if any(f["level"] == "high" for f in findings) else 0
    if not findings:
        print("体检通过：没有发现已知坑")
        return 0
    order = {"high": 0, "medium": 1, "low": 2}
    for finding in sorted(findings, key=lambda f: order.get(f["level"], 3)):
        print(f"[{finding['level']:<6}] {finding['kind']}：{finding['detail']}")
        print(f"          修复：{finding['fix']}")
    high = sum(1 for finding in findings if finding["level"] == "high")
    print(f"\n共 {len(findings)} 项，其中 high {high} 项")
    return 1 if high else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
