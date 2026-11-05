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

import ast
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


def subprocess_encoding_gaps(directory: pathlib.Path) -> list[str]:
    """`subprocess(..., text=True)` 没钉 `encoding=` ⇒ 子进程按本机 locale 解码。

    中文 Windows 上默认 cp936：我们的门脚本往 stdout 打 UTF-8 中文时，读线程直接抛
    UnicodeDecodeError，调用方拿到**空输出**——于是"读不到就等于没有"的分支被走进来，
    门变成假绿。这一类只能靠静态钉子钉住：AST 里找 text/universal_newlines 而没 encoding 的调用。
    """
    gaps: list[str] = []
    for path in sorted(directory.glob("*.py")):
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"))
        except (SyntaxError, UnicodeDecodeError, ValueError):
            gaps.append(f"{path.name}（解析不了，证明不了它接没接编码）")
            continue
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            name = getattr(node.func, "attr", "") or getattr(node.func, "id", "")
            if name not in ("run", "check_output", "Popen", "call", "communicate"):
                continue
            keywords = {k.arg for k in node.keywords if k.arg}
            if keywords & {"text", "universal_newlines"} and "encoding" not in keywords:
                gaps.append(f"{path.name}:{node.lineno}")
    return gaps


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


def history_big_blobs(root: pathlib.Path, limit: int = 5) -> list[tuple[str, int, str]]:
    """历史里出现过的大 blob（(路径, 字节, oid)）——HEAD 里的文件大小只是一半的事实。

    一次 `git rev-list --objects --all | git cat-file --batch-check`，不逐条起进程。
    """
    listing = run(["rev-list", "--objects", "--all"], root)
    if not listing:
        return []
    result = subprocess.run(
        ["git", "cat-file", "--batch-check=%(objecttype) %(objectname) %(objectsize) %(rest)"],
        cwd=root, input=listing, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, encoding="utf-8", errors="replace",
    )
    rows: list[tuple[str, int, str]] = []
    for line in result.stdout.splitlines():
        parts = line.split(" ", 3)
        if len(parts) < 4 or parts[0] != "blob":
            continue
        size = int(parts[2]) if parts[2].isdigit() else 0
        if size >= BIG_FILE:
            rows.append((parts[3], size, parts[1]))
    rows.sort(key=lambda item: -item[1])
    return rows[:limit]


def lfs_plan(root: pathlib.Path) -> list[str]:
    """把大文件迁到 LFS 的命令（会改写历史，所以只出方案，不自动执行）。"""
    big = []
    for name in tracked_files(root):
        path = root / name
        try:
            if path.is_file() and path.stat().st_size > BIG_FILE:
                big.append(name)
        except OSError:
            continue
    if not big:
        return ["当前没有 > 5 MiB 的受控文件，不需要上 LFS"]
    suffixes = sorted({pathlib.PurePosixPath(name).suffix for name in big if pathlib.PurePosixPath(name).suffix})
    include = " ".join(f'"*{suffix}"' for suffix in suffixes) or '"*"'
    return [
        f"要迁的类型：{', '.join(suffixes) or '(无扩展名)'}",
        f"git lfs install",
        f"git lfs migrate import --include={include} --everything   # 会改写历史，先打快照/备份",
        "回滚：git lfs migrate export --include=<同样的模式> --everything",
        "推之前确认：git log --stat | grep -c 'Pointer'（应大于 0）；CI 需装 git-lfs",
    ]


def diagnose(root: pathlib.Path, remote: bool, history: bool = False) -> list[dict]:
    findings: list[dict] = []

    def add(kind: str, level: str, detail: str, fix: str) -> None:
        findings.append({"kind": kind, "level": level, "detail": detail, "fix": fix})

    files = tracked_files(root)
    if not files and not history:
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

    gaps = subprocess_encoding_gaps(root / "scripts")
    if gaps:
        add("子进程编码", "high", f"{len(gaps)} 处 subprocess(text=True) 没钉 encoding（例：{gaps[0]}）",
            '补 encoding="utf-8", errors="replace"（中文 Windows 默认 cp936，读不到输出＝假绿）')

    if history:
        for path, size, oid in history_big_blobs(root):
            add("历史大文件", "medium", f"{path}（{size / 1024 / 1024:.1f} MiB，blob {oid[:10]}…）",
                "git filter-repo --strip-blobs-bigger-than 5M   # 会改写历史，先快照")
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
        # 夹具必须自带口径：不钉 autocrlf 的话，Linux CI 上 renormalize 不做转换，
        # 索引侧仍是 CRLF，这条 would 只能判 high —— 断言就跟着机器跑了。
        subprocess.run(["git", "config", "core.autocrlf", "true"], cwd=root)
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
        # 接线面：诊断必须真的把「子进程编码」挂进 findings（只测纯函数测不到这条线）
        scripts_dir = root / "scripts"
        scripts_dir.mkdir(exist_ok=True)
        (scripts_dir / "tool.py").write_text(
            'import subprocess\nsubprocess.run(["x"], text=True)\n', encoding="utf-8")
        wired = {finding["kind"]: finding["level"] for finding in diagnose(root, remote=False)}
        check("诊断里有「子进程编码」这一类", "子进程编码" in wired, True)
        check("子进程编码判 high（不降级成没人看的 low）", wired.get("子进程编码"), "high")
        (scripts_dir / "tool.py").write_text(
            'import subprocess\nsubprocess.run(["x"], text=True, encoding="utf-8")\n',
            encoding="utf-8")
        check("钉好编码后该类消失",
              "子进程编码" in {f["kind"] for f in diagnose(root, remote=False)}, False)
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

        # 子进程编码钉子：text=True 不钉 encoding 在中文 Windows 上会把门变成假绿
        enc = root / "enc"
        enc.mkdir()
        (enc / "bad.py").write_text('import subprocess\nsubprocess.run(["x"], text=True)\n', encoding="utf-8")
        (enc / "good.py").write_text('import subprocess\n'
                                       'subprocess.run(["x"], text=True, encoding="utf-8", errors="replace")\n',
                                       encoding="utf-8")
        (enc / "bytes.py").write_text('import subprocess\nsubprocess.run(["x"])\n', encoding="utf-8")
        check("没钉 encoding 的被点名", subprocess_encoding_gaps(enc), ["bad.py:2"])
        (enc / "bad.py").write_text('import subprocess\n'
                                     'subprocess.run(["x"], text=True, encoding="utf-8")\n', encoding="utf-8")
        check("钉了 encoding 就放行", subprocess_encoding_gaps(enc), [])
        (enc / "nl.py").write_text('import subprocess\n'
                                    'subprocess.call(["x"], universal_newlines=True)\n', encoding="utf-8")
        check("universal_newlines 同罪", subprocess_encoding_gaps(enc), ["nl.py:2"])
        (enc / "broken.py").write_text("def (\n", encoding="utf-8")
        check("解析不了的文件也点名（不静默跳过）",
              [g for g in subprocess_encoding_gaps(enc) if g.startswith("broken.py")],
              ["broken.py（解析不了，证明不了它接没接编码）"])

        # 历史大文件：提交一个 6 MiB 文件再删掉，HEAD 里没有、历史里有
        big_repo = pathlib.Path(tmp) / "big"
        big_repo.mkdir()
        gitc = lambda args: subprocess.run(["git", *args], cwd=big_repo, stdout=subprocess.PIPE,
                                           stderr=subprocess.STDOUT, text=True,
                                           encoding="utf-8", errors="replace")
        gitc(["init", "-q"])
        gitc(["config", "user.email", "t@example.com"])
        gitc(["config", "user.name", "t"])
        (big_repo / "blob.bin").write_bytes(b"0" * (6 * 1024 * 1024))
        gitc(["add", "-A"])
        gitc(["commit", "-qm", "add big"])
        (big_repo / "blob.bin").unlink()
        gitc(["add", "-A"])
        gitc(["commit", "-qm", "remove big"])
        check("HEAD 里没有大文件", [n for n in tracked_files(big_repo) if n == "blob.bin"], [])
        check("历史里能找出大 blob", bool(history_big_blobs(big_repo)), True)
        check("历史检查接入诊断",
              any(f["kind"] == "历史大文件" for f in diagnose(big_repo, remote=False, history=True)), True)
        (big_repo / "big2.bin").write_bytes(b"0" * (6 * 1024 * 1024))
        gitc(["add", "-A"])
        gitc(["commit", "-qm", "add big2"])
        plan = lfs_plan(big_repo)
        check("LFS 方案含 migrate", any("lfs migrate" in line for line in plan), True)
        check("LFS 方案含回滚", any("export" in line for line in plan), True)
        check("LFS 方案点名类型", any("*.bin" in line for line in plan), True)

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
    if "--lfs-plan" in argv:
        for line in lfs_plan(root):
            print(line)
        return 0
    findings = diagnose(root, remote="--remote" in argv, history="--history" in argv)
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
