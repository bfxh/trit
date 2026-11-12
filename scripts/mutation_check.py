#!/usr/bin/env python3
"""变异门：对关键规则做单点文本变异，跑对应回归；测试必须变红。

用法：python scripts/mutation_check.py [--pkg <目标>]... [--only <名字片段>] [--shard i/n]
目标 = crate 名，或带自测面的脚本路径（`scripts/*.py`、`crates/*/scripts/*.sh`）。
`--only` 只用于本地复跑个别条目，会大声打 `PARTIAL RUN`，且一条都没选中时判红（打错字不等于跑过）。
`--shard i/n` 只跑第 i 片（crc32(file\0name) 稳定分片，1 ≤ i ≤ n，越界 rc2）；
各片必须在**不同检出**里跑（同树有单实例锁与共享文件）；空片是真空集，明说后放行。
`--partition n [--logs <目录>]` 是聚合门（不跑测试）：核对分片是干净划分；给了日志再逐条对账——
全表每条在日志里恰好一个判决且是杀死。CI 的 full 矩阵各片上传日志，聚合 job 用它收口。
每个变异都会在 finally 里**按字节、原子替换**恢复原文件；退出码 1 表示有变异存活、假杀、超时或无输出。

四个反脆弱的阀门（都是本机踩出来的，不是设想）：
  · **每条变异有超时**（`MUTANT_TIMEOUT_SECS`，默认 900 秒）。踩过两次：同一台机器两个会话各跑一轮门，
    抢同一把 target 目录锁 ⇒ `cargo` 只等不编（有 cargo 无 rustc 子进程），整轮静默挂 20 分钟以上。
    超时报 `TIMEOUT` 并判红 —— 那是"没结论"，不是"抓住"。杀超时进程时杀**整棵树**：
    `subprocess.run` 只杀直接子进程，cargo 死了、它拉起的测试 exe 还在原地烧一个核
    （2026-11-05 本地实测：pid 27480 空转，CPU 每秒 +1）。
  · **必然死循环的变异在登记表里写 `hang_secs`**：拆掉终止守卫这类变异（`if rows.pop().is_none() { break }`
    → 永不 break）没有断言可钉，只能靠超时判。写 `hang_secs` = 用短超时 + 单列成 `HANG-KILL`，
    **不计入"断言抓住"**；没写这条声明的超时仍然判红，防止把环境卡死当成抓住。
  · **单实例锁**（`scripts/.mutation-check.lock/`，记 pid）。已有活着的门在跑就直接拒；
    持有者已死算陈旧锁，接管并点名。确认要并行：`MUTATION_CHECK_ALLOW_CONCURRENT=1`。
  · **写文件走 temp + os.replace**。踩过：SIGKILL 正好落在 `write_text` 中间 ⇒ 源文件被截成 0 字节。

语言口径见 RUNNERS：
  cargo  —— `cargo test -p <crate> <测试名> --locked`
  python —— `<python> <脚本> --self-test`
  bash   —— `bash <脚本> --self-test`
登记表里没写 runner 时按 pkg 后缀推断（.py / .sh），避免脚本被当成 crate 去跑 cargo。
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import tempfile
import time
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[1]

# 一条变异正常是几秒到几十秒；900 秒是「慢」与「卡死」的分界，不是性能目标。
DEFAULT_TIMEOUT_SECS = float(os.environ.get("MUTANT_TIMEOUT_SECS", "900"))
# 超时用哨兵退出码，绝不能与真实 rc 撞上（真实测试失败是 101/10，崩溃是 0xC000…）。
TIMEOUT_RC = 100_000
LOCK_DIR = pathlib.Path(__file__).resolve().parent / ".mutation-check.lock"


def lock_path() -> pathlib.Path:
    """锁目录可注入：自测走临时目录，绝不碰仓里那把真锁。

    CI 会把 `mutation_check.py` 自己的变异也跑一遍（子进程 = `--self-test`），那时外层门正持有真锁；
    自测若去抢同一把就是"自己挡自己"，基线直接判红（2026-11-05 在 CI 上实测踩到）。"""
    override = os.environ.get("MUTATION_CHECK_LOCK_DIR")
    return pathlib.Path(override) if override else LOCK_DIR

MUTANT_DIR = pathlib.Path(__file__).resolve().parent / "mutants"


def load_mutants(directory: pathlib.Path) -> list[dict]:
    """把登记表按目标拆成 `mutants/<目标>.json` 再拼回来。

    为什么不是一张内联大表（之前 4983 行、占全文 90%）：多个智能体各自给**不同目标**
    加变异时，插入点全在同一处 ⇒ 必然冲突（今天真撞过一次，还是靠认领闸门先点的名）。
    拆开后只有真的改同一个目标才会撞。文件名与条目都排序，diff 稳定。
    """
    if not directory.is_dir():
        raise SystemExit(f"找不到登记表目录 {directory} —— 没表不等于没有变异，判红")
    found: list[dict] = []
    for entry_file in sorted(directory.glob("*.json")):
        data = json.loads(entry_file.read_text(encoding="utf-8"))
        if not isinstance(data, list):
            raise SystemExit(f"登记表格式不对：{entry_file.name} 应该是数组")
        found.extend(data)
    return found


MUTANTS = load_mutants(MUTANT_DIR)


def ran_any(output: str) -> bool:
    match = re.search(r"test result: ok\. (\d+) passed", output)
    return bool(match and int(match.group(1)) > 0)


# 每种语言一条命令口径：pkg 就是要跑的目标（crate 名 或 脚本路径），
# 由 CI 的 collect 步骤按改动文件生成 `--pkg <目标>`。
# 门脚本自测面的标记串只能拼出来：本文件自己也是被检测对象，
# 写死字面量会让 `has_own_logic("scripts/mutation_check.py")` 把自己判成"有自测面"。
SELFTEST_MARKER = "def self_" + "test("
# bash 侧同理：标记串拼出来，别让自己的字面量把检测结果带偏。
SELFTEST_MARKER_SH = "self_" + "test() {"

RUNNERS: dict[str, dict[str, str]] = {
    # cargo：跑 crate 里被点名的那个测试
    "cargo": {"cmd": "cargo", "syntax": "error[E"},
    # python：门脚本自带的 --self-test（正例 + 逐项反证），失败时打印 `FAIL <例名>`
    "python": {"cmd": "python", "syntax": "SyntaxError"},
    # bash：冒烟/门脚本自带的 --self-test，失败时同样打印 `FAIL <例名>`
    # 语法判红的签名分语言环境：中文 Windows 的 bash 打「语法错误」而不是 "syntax error"
    "bash": {"cmd": "bash", "syntax": "syntax error|语法错误"},
}


def selftest_marker(runner: str) -> str:
    return SELFTEST_MARKER_SH if runner == "bash" else SELFTEST_MARKER


def argv_of(pkg: str, test_filter: str, runner: str) -> list[str]:
    """跑法：脚本走自己的 --self-test，crate 走 cargo test（只点名那一条测试）。"""
    if runner == "python":
        # -X utf8 是硬要求：中文 Windows 默认 cp936，子进程打的 `FAIL <例名>` 会被
        # 按 utf-8 解成乱码 ⇒ 门还能判红，但点名的那条断言读不出人话。
        return [sys.executable, "-X", "utf8", pkg, "--self-test"]
    if runner == "bash":
        return ["bash", pkg, "--self-test"]
    return ["cargo", "test", "-q", "-p", pkg, test_filter, "--locked"]


def kill_tree(proc: subprocess.Popen[str]) -> None:
    """连子进程一起杀。

    `subprocess.run(timeout=)` 只杀直接子进程：cargo 死了，它拉起的测试 exe 还在原地空转烧核
    （拆掉终止守卫的那条变异正是这种）。Windows 用 taskkill /T，POSIX 用进程组。"""
    with contextlib.suppress(OSError, subprocess.TimeoutExpired):
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=60,
            )
        else:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    with contextlib.suppress(OSError):
        proc.kill()


def proc_gone(pid: int) -> bool:
    """孙进程是否真的没了。

    POSIX 上 `os.kill(pid, 0)` 对**僵尸**仍然成功：父进程一起被杀后，孩子要等 init 收尸，
    这中间它既是"死的"又"还在"。CI 上踩过：判据用 _pid_alive ⇒ 把僵尸读成"没杀掉"，自测自己假红。"""
    if not _pid_alive(pid):
        return True
    if os.name == "nt":
        return False
    with contextlib.suppress(Exception):
        state = subprocess.run(
            ["ps", "-o", "stat=", "-p", str(pid)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=10,
        ).stdout.strip()
        if not state or state.startswith("Z"):
            return True
    return False


def run_command(argv: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    """跑一条命令，超时**不当成抓住**：返回哨兵 rc + 点明最常见成因（target 目录锁被别的门占着）。"""
    kwargs: dict = {
        "cwd": ROOT,
        "stdout": subprocess.PIPE,
        "stderr": subprocess.STDOUT,
        "text": True,
        "encoding": "utf-8",
        "errors": "replace",
    }
    if os.name != "nt":
        # POSIX：自成进程组，超时可以整组端掉（CI 的 ubuntu/macos 走这条）。
        kwargs["start_new_session"] = True
    proc = subprocess.Popen(argv, **kwargs)
    try:
        out, _ = proc.communicate(timeout=timeout)
        return subprocess.CompletedProcess(argv, proc.returncode, stdout=out, stderr="")
    except subprocess.TimeoutExpired as exc:
        kill_tree(proc)
        with contextlib.suppress(Exception):
            # 树已死、管道随之关闭：把残留输出收干净，别丢掉半截日志。
            exc.stdout, _ = proc.communicate(timeout=60)
        partial = exc.stdout or b""
        if isinstance(partial, bytes):
            partial = partial.decode("utf-8", errors="replace")
        tail = (
            f"\nMUTANT TIMEOUT {timeout:.0f}s：子进程连同它的测试 exe 已被杀 ⇒ 这条没有结论。"
            "本机常见原因是另一个门/编译占着 target 目录锁"
            "（`cargo` 在 `Blocking waiting for file lock`：有 cargo、没有 rustc 子进程）。\n"
        )
        return subprocess.CompletedProcess(argv, TIMEOUT_RC, stdout=partial + tail, stderr="")


def hang_secs_of(mutant: dict) -> float | None:
    """登记表里声明"这条变异必然死循环"时用它自己的短超时；没声明就用默认。"""
    declared = mutant.get("hang_secs")
    return float(declared) if declared else None


def run_tests(
    pkg: str, test_filter: str, runner: str = "cargo", timeout: float | None = None
) -> subprocess.CompletedProcess[str]:
    return run_command(argv_of(pkg, test_filter, runner),
                      DEFAULT_TIMEOUT_SECS if timeout is None else timeout)


# 显式豁免：只有"整个 crate 不含自有逻辑"这种**可以被机器复核**的理由才收在这里。
# 复核在 has_own_logic()：一旦该包出现了 fn/impl，豁免自动失效并判红（防止理由烂掉）。
EXEMPT: dict[str, str] = {
    "sansheng": "三值工具箱的伞包：lib.rs 只有 pub use 转发与文档 doctest，无自有规则",
    "scripts/gen_docs_charts.py": "出图脚本，无 --self-test 且非承重判定；不参与发布门禁",
    # 这两个是薄壳（5 / 19 行），没有可钉的判定；一旦有人给它们补 self_test()，豁免自动过期。
    "scripts/bench.sh": "基准薄壳（5 行）：只是把参数转给 cargo bench，无可变异判定",
    "scripts/publish.sh": "发布薄壳（19 行）：版本校验后转给 cargo publish，无可变异判定",
}

FN_OR_IMPL = re.compile(
    r"^\s*(pub\s+)?(const\s+|unsafe\s+|async\s+)*(fn|impl)\b", re.M
)


def has_own_logic(pkg: str, *, root: pathlib.Path = ROOT) -> bool:
    """豁免是否已过期。

    - python 门脚本（`scripts/*.py`）：**有 `--self-test` 面却不登记变异** ⇒ 过期（该补条目）；
      没有自测面才允许豁免（钉不住任何东西），等它补上自测再逼登记。
    - bash 脚本（`*.sh`）：同上口径，认 `self_test() {`。
    - rust crate：出现了自有 `fn`/`impl` ⇒ 过期（纯转发伞包才允许豁免）。
    """
    if pkg.endswith(".py") or pkg.endswith(".sh"):
        script = root / pkg
        if not script.is_file():
            return False
        text = script.read_text(encoding="utf-8")
        return selftest_marker("bash" if pkg.endswith(".sh") else "python") in text
    src = root / "crates" / pkg / "src"
    if not src.is_dir():
        return True
    for path in sorted(src.rglob("*.rs")):
        body = "\n".join(
            line
            for line in path.read_text(encoding="utf-8").splitlines()
            if not line.lstrip().startswith(("//!", "///", "//"))
        )
        if FN_OR_IMPL.search(body):
            return True
    return False


def write_atomic(path: pathlib.Path, data: bytes) -> None:
    """原地替换：中途被杀不可能留下半篇源文件（踩过：write_text 被 SIGKILL 打断 ⇒ 整篇成 0 字节）。"""
    tmp = path.with_name(path.name + ".mutant-tmp")
    tmp.write_bytes(data)
    os.replace(tmp, path)


def _pid_alive(pid: int) -> bool:
    """跨平台的"这个 pid 还在不在"。查不到就当作还在（宁可拒，也不要两个门互锁）。"""
    if pid <= 0:
        return False
    if os.name == "nt":
        import ctypes

        PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
        kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
        handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not handle:
            return False
        kernel32.CloseHandle(handle)
        return True
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def acquire_lock() -> tuple[bool, str]:
    """单实例：两个门互抢 target 锁会一起挂死 ⇒ 后来者直接被拒，而不是排队装忙。"""
    if os.environ.get("MUTATION_CHECK_ALLOW_CONCURRENT") == "1":
        return True, "并行已显式允许（MUTATION_CHECK_ALLOW_CONCURRENT=1）"
    here = lock_path()
    if here.exists():
        pid = -1
        try:
            pid = int((here / "pid").read_text(encoding="utf-8").strip() or "-1")
        except (OSError, ValueError):
            pass
        if _pid_alive(pid):
            return False, f"另一个变异门正在跑（pid {pid}）⇒ 等它结束，或确认它已死后删 {here}"
        note = f"接管陈旧锁（原持有者 pid {pid} 已不在）"
        release_lock()
    else:
        note = ""
    here.mkdir(parents=True, exist_ok=True)
    (here / "pid").write_text(str(os.getpid()), encoding="utf-8")
    return True, note


def release_lock() -> None:
    here = lock_path()
    with contextlib.suppress(OSError):
        (here / "pid").unlink(missing_ok=True)
        here.rmdir()


def runner_of(mutant: dict) -> str:
    """条目的 runner 优先；没写就按 pkg 后缀推断，免得脚本被当成 crate 去 `cargo test`。"""
    explicit = mutant.get("runner")
    if explicit:
        return explicit
    pkg = mutant["pkg"]
    if pkg.endswith(".py"):
        return "python"
    if pkg.endswith(".sh"):
        return "bash"
    return "cargo"


def select(pkgs: list[str], files: list[str] | None = None) -> list[dict]:
    chosen = MUTANTS if not pkgs else [mutant for mutant in MUTANTS if mutant["pkg"] in pkgs]
    if files:
        wanted = {name.replace("\\", "/") for name in files if name}
        chosen = [mutant for mutant in chosen if mutant["file"] in wanted]
    return chosen


def shard_key(mutant: dict) -> int:
    """稳定分片键：file+name 的 crc32。不依赖登记表顺序、不依赖进程与对象身份——
    同一条在任何检出、任何一次运行里都落同一片；新增条目只挪它自己，不重排别人。"""
    return zlib.crc32((mutant["file"] + "\0" + mutant["name"]).encode("utf-8"))


def select_shard(mutants: list[dict], i: int, n: int) -> list[dict]:
    return [mutant for mutant in mutants if shard_key(mutant) % n == i - 1]


def partition_report(mutants: list[dict], n: int) -> dict:
    """纯计算：n 片是不是全表的干净划分。covers_all 按集合（有没有漏），
    disjoint 按重数（有没有一条落多片）——两个判据分开，坏法才能各自点名。"""
    shards = [select_shard(mutants, i, n) for i in range(1, n + 1)]
    union = [mutant["name"] for shard in shards for mutant in shard]
    registered = {mutant["name"] for mutant in mutants}
    return {
        "counts": [len(shard) for shard in shards],
        "covers_all": set(union) == registered,
        "disjoint": len(union) == len(set(union)),
    }


VERDICT_LINE = re.compile(
    r"^(caught|CRASH-KILL|HANG-KILL|TIMEOUT|SURVIVED|NO-OUTPUT|FALSE-KILL|UNNAMED)\s+(.+?)(?:\s\s\[.*)?$",
    re.M,
)


def verdicts_from_log(text: str) -> list[tuple[str, str]]:
    """只认行首的判决行：结尾缩进的 TIMEOUT/HANG-KILL 清单是汇总不是判决，
    算进去会把同一条计两次。名字里有空格与中文，按 `  [` 分隔截断。"""
    return [(match.group(2), match.group(1)) for match in VERDICT_LINE.finditer(text)]


def coverage_report(mutants: list[dict], logs: list[str]) -> dict:
    """各片日志合起来是否恰好覆盖全表：缺判决/_unknown/重复/非杀死都单列点名。"""
    registered = [mutant["name"] for mutant in mutants]
    seen: dict[str, list[str]] = {}
    for text in logs:
        for name, kind in verdicts_from_log(text):
            seen.setdefault(name, []).append(kind)
    kills = ("caught", "CRASH-KILL", "HANG-KILL")
    return {
        "missing": sorted(set(registered) - set(seen)),
        "unknown": sorted(set(seen) - set(registered)),
        "duplicated": sorted(name for name, kinds in seen.items() if len(kinds) > 1),
        "not_killed": sorted({name for name, kinds in seen.items() if any(kind not in kills for kind in kinds)}),
        "caught": sum(kinds.count("caught") for kinds in seen.values()),
        "crash": sum(kinds.count("CRASH-KILL") for kinds in seen.values()),
        "hang": sum(kinds.count("HANG-KILL") for kinds in seen.values()),
    }


def partition_audit(n: int, logs_dir: pathlib.Path | None) -> int:
    """聚合门：划分本身必须是干净划分；给了日志再逐条对账。
    读不到日志、缺条目、重条目、不认识条目、非杀死判决 ⇒ 全判红（读不到不等于跑过了）。"""
    if not MUTANTS:
        print("NO MUTANTS REGISTERED：聚合模式全表为空 ⇒ 判红。")
        return 1
    report = partition_report(MUTANTS, n)
    detail = ", ".join(f"片{i}={count}" for i, count in enumerate(report["counts"], 1))
    print(f"PARTITION {n} 片：{detail}（共 {len(MUTANTS)} 条）")
    red = False
    if not report["covers_all"]:
        print("PARTITION BROKEN：各片并集 ≠ 全表（有漏片）⇒ 判红")
        red = True
    if not report["disjoint"]:
        print("PARTITION BROKEN：同一条落进多片（重复）⇒ 判红")
        red = True
    if logs_dir is None:
        return 1 if red else 0
    files = sorted(path for path in logs_dir.glob("*.log") if path.is_file())
    if not files:
        print(f"NO SHARD LOGS：{logs_dir} 下没有 *.log ⇒ 判红（读不到不等于跑过了）")
        return 1
    cov = coverage_report(MUTANTS, [path.read_text(encoding="utf-8", errors="replace") for path in files])
    print(f"COVERAGE {len(files)} 份日志：caught {cov['caught']} + crash {cov['crash']} + hang {cov['hang']}（全表 {len(MUTANTS)} 条）")
    for label, names in (("缺判决", cov["missing"]), ("不认识条目", cov["unknown"]),
                         ("重复判决", cov["duplicated"]), ("非杀死", cov["not_killed"])):
        if names:
            print(f"COVERAGE BROKEN：{label} {len(names)} 条 ⇒ 判红：{', '.join(names[:20])}")
            red = True
    return 1 if red else 0


TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*mod\s+\w+")


def preflight(mutants: list[dict], *, root: pathlib.Path = ROOT) -> list[str]:
    """跑测试之前先把文件与锚点查一遍：路径写错/锚点漂移要立刻报全，不等到跑一半才崩。

    锚点落在 `#[cfg(test)] mod ...` 之后也算错：改测试代码不会改产品行为，
    这类变异就算「没抓住」也不说明断言弱，只会把债账搅浑（拆模块时真踩到过）。
    """
    problems: list[str] = []
    for mutant in mutants:
        raw = mutant["file"]
        if "\\" in raw:
            problems.append(f"{mutant['name']}: 路径含反斜杠（Linux 打不开）: {raw}")
            continue
        path = root / raw
        if not path.is_file():
            problems.append(f"{mutant['name']}: 文件不存在: {raw}")
            continue
        text = path.read_text(encoding="utf-8")
        count = text.count(mutant["old"])
        if count != 1:
            problems.append(f"{mutant['name']}: 锚点出现 {count} 次（{raw}）")
            continue
        anchor = text.find(mutant["old"])
        runner = runner_of(mutant)
        if runner in ("python", "bash"):
            marker = selftest_marker(runner)
            at = text.rfind(marker)
            label = marker
        else:
            # cargo 侧仍按原口径：只认 `#[cfg(test)] mod xxx`，
            # 放宽成裸 `#[cfg(test)]` 会把 trit-build 的 4 条正常条目误判成"落在测试里"。
            matches = [m.start() for m in TEST_MODULE.finditer(text)]
            at = matches[-1] if matches else -1
            label = "#[cfg(test)] mod"
        if at >= 0 and anchor > at:
            problems.append(f"{mutant['name']}: 锚点落在测试/自测代码里（{raw}，{label} 之后）")
    return problems


def classify(out: str, rc: int, runner: str) -> tuple[str, str]:
    """把一次变异运行的结果判成五类：caught / false_kill / crash / unnamed / inconclusive。

    rc==0 的"存活"由调用方先判，不在这里。
    返回 (kind, detail)：caught 的 detail 是点名，crash 的 detail 是崩溃提示。
    这一层是门最容易被改坏的地方（编译红当抓住、把崩溃当断言各踩过一次），
    所以抽成纯函数，由 `--self-test` 用合成输出逐类钉住。
    """
    syntax = RUNNERS[runner]["syntax"]
    if not out.strip():
        # 一条输出都没有 ⇒ 测试根本没跑起来（被杀、被锁挡在编译前）。
        # 这类"红"最容易被当成抓住：昨晚手工杀掉死循环的测试 exe，cargo 空输出退出，
        # 门就把那条记成了 caught。单列成没结论，判红。
        return "inconclusive", "无输出：进程没跑完就被杀或没跑起来"
    # 只认工具链自己打的诊断：断言行的例名里可能出现 "SyntaxError" 这类字样，
    # 把断言文本当成编译失败就会把"真抓住"误判成假杀（门自测第一版就这么翻车）。
    # lstrip 不是装饰：`ci_budget / secret_wiring / oops` 印的是缩进的 `  FAIL <例名>`，
    # 不认缩进 ⇒ 这 50 条既进不了 diag 剔除、也点不出名，全落 unnamed（2026-11-03 全表普查实测）。
    diag = "\n".join(line for line in out.splitlines() if not line.lstrip().startswith("FAIL "))
    if "could not compile" in diag or any(tok in diag for tok in syntax.split("|")):
        # 变异让代码编不过 ⇒ 测试根本没跑，这条"红"不证明任何断言。
        return "false_kill", ""
    named = next((line for line in out.splitlines() if "FAILED" in line), None)
    if runner in ("python", "bash"):
        # 门脚本自测面的点名方式各不相同，按能点名的优先：
        #   `FAIL <例名>`（module_budget / publish_graph / bash 冒烟脚本）、
        #   `SELF-TEST FAIL: <规则>`（hazard_gate）、
        #   裸 `assert` 抛出的 `AssertionError: <话>`（pr_hygiene）—— 那也是断言。
        for probe in (
            lambda: next((l for l in out.splitlines() if l.lstrip().startswith("FAIL ")), None),
            lambda: next((l for l in out.splitlines() if "SELF-TEST FAIL" in l), None),
            lambda: next(
                (
                    "断言失败: {} ｜ 位置 {}".format(
                        l.split("AssertionError", 1)[-1].strip(": ").strip() or "(无消息)",
                        next(
                            (
                                f"{m.group(1).rsplit('/', 1)[-1].rsplit(chr(92), 1)[-1]}:{m.group(2)} in {m.group(3)}"
                                for m in reversed(
                                    [
                                        fr
                                        for fr in re.finditer(
                                            r'File "([^"]+)", line (\d+), in (\S+)', out
                                        )
                                    ]
                                )
                                if "mutation_check" not in m.group(1)
                            ),
                            "?",
                        ),
                    )
                    for l in out.splitlines()
                    if "AssertionError" in l
                ),
                None,
            ),
        ):
            if named is None:
                named = probe()
        if named is not None and "self-test:" in out:
            tally = next((l for l in out.splitlines() if "self-test:" in l), "")
            named = f"{named.strip()} ｜ {tally.strip()}"
    tests = sorted({m.group(1) for m in re.finditer(r"^---- (\S+) stdout ----", out, re.M)})
    if named is None and tests:
        named = " / ".join(tests) + " 断言失败"
    if named is None:
        # 没有断言痕迹，只有进程异常退出（Windows 上常见 0xC00000FD 栈溢出、
        # 0xC0000005 访问违例；Linux 上是 signal: 6/11）。这类"红"只证明
        # 去掉守卫会炸，不证明有断言钉住它 ⇒ 单列成 CRASH-KILL，不与断言混计。
        crash = rc < 0 or rc >= 0xC0000000 or bool(
            re.search(r"overflowed its stack|signal: \d+|exit code: 0xc00000", out)
        ) or (
            runner == "python"
            and "Traceback (most recent call last)" in out
            and "AssertionError" not in out
        )
        if crash:
            hint = next(
                (
                    line.strip()
                    for line in out.splitlines()
                    if "overflowed" in line or "exit code" in line or "signal:" in line
                ),
                f"rc={rc}",
            )
            return "crash", hint
        return "unnamed", ""
    return "caught", named.strip()


def main(pkgs: list[str], only: str = "", shard: tuple[int, int] | None = None,
         partition: int = 0, logs: str = "", files: list[str] | None = None) -> int:
    # Windows 控制台默认可能是 GBK；变异名里有中国字与 Ł，统一按 UTF-8 输出。
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if partition:
        return partition_audit(partition, pathlib.Path(logs) if logs else None)
    mutants = select(pkgs, files)
    # fail-closed（**逐目标**）：某个目标在登记表里 0 条时，"它绿了"没有任何含义。
    # 多个目标一起选中时更危险——别的目标有变异可杀，这个目标会被静默跳过
    # （2026-11-02 的 trit-immune 就是这样在 CI 报 14s 通过的）。
    # 唯一的松口是 EXEMPT：必须是"纯转发/无判决面"这种**可自证**的理由，自证一失效就判红。
    counts = {pkg: sum(1 for mutant in MUTANTS if mutant["pkg"] == pkg) for pkg in pkgs}
    uncovered = [pkg for pkg, n in counts.items() if n == 0]
    unexplained = [pkg for pkg in uncovered if pkg not in EXEMPT]
    stale = [pkg for pkg in uncovered if pkg in EXEMPT and has_own_logic(pkg)]
    if unexplained or stale:
        scope = ", ".join(unexplained or ["(全仓)"]) if unexplained else ", ".join(stale)
        print(f"NO MUTANTS REGISTERED：{scope} 在 MUTANTS 里是 0 条 ⇒ 判红。"
              "补登记（锚点 + 对应测试名），或确认它是纯转发后登记进 EXEMPT 并写理由。")
        for pkg in stale:
            print(f"EXEMPT 过期：{pkg} 已经有自己的 fn/impl 了，豁免不再成立 ⇒ 请补登记变异。")
        return 1
    for pkg in uncovered:
        print(f"EXEMPT（已自证无自有逻辑）: {pkg} —— {EXEMPT[pkg]}")
    if not mutants:
        if not pkgs:
            print("NO MUTANTS REGISTERED：（全仓）MUTANTS 是空的 ⇒ 判红。")
            return 1
        return 0
    if pkgs:
        detail = ", ".join(f"{pkg}={count}" for pkg, count in sorted(counts.items()))
        print(f"只跑所选包（{detail}）：{len(mutants)} 个变异")
    if only:
        picked = [m for m in mutants if only in m["name"]]
        if not picked:
            print(f"NO MUTANTS MATCHED --only {only!r}：{len(mutants)} 条里一条都没选中 ⇒ 判红（打错字不等于跑过了）")
            return 1
        print(f"PARTIAL RUN --only {only!r}：{len(picked)}/{len(mutants)} 条 —— 这不是全量复跑，CI 仍按包跑全部")
        mutants = picked
    if shard is not None:
        i, n = shard
        picked = select_shard(mutants, i, n)
        print(f"SHARD {i}/{n}: {len(picked)}/{len(mutants)} 条 —— 这不是全量复跑；各片必须在**不同检出**里跑（同树有单实例锁与共享文件）")
        if not picked:
            print(f"SHARD {i}/{n}: 0 条（真空集：登记条目都落在别的片）⇒ 本片放行")
            return 0
        mutants = picked
    held, lock_note = acquire_lock()
    if not held:
        print(f"REFUSED：{lock_note}")
        return 1
    if lock_note:
        print(f"锁：{lock_note}")
    try:
        return _run(mutants)
    finally:
        release_lock()


def tally(
    total: int, survived: int, false_kills: int, crash_kills: int,
    timed_out: int, hang_kills: int, no_output: int, unnamed: int,
) -> tuple[int, bool]:
    """抓住多少条，以及门该不该判红。

    `unnamed` = 进程 rc≠0 但既没有断言点名也没有崩溃签名：磁盘写满导致链接失败、
    cargo 不在 PATH、harness 自己报错都落这一格。它不证明任何断言钉住了什么 ⇒
    既不能计成抓住，也不能静默算过（原先就是把它计进 killed，于是环境一坏门反而"全绿"）。
    `crash` / `hang` 单列但维持不判红：崩溃至少证明把守卫去掉会炸，只是没有断言兜着；
    hang 是登记表声明的死循环，无断言可钉。
    """
    killed = (total - survived - false_kills - crash_kills
              - timed_out - hang_kills - no_output - unnamed)
    return killed, bool(survived or false_kills or timed_out or no_output or unnamed)


def _run(mutants: list[dict]) -> int:
    problems = preflight(mutants)
    if problems:
        print(f"PREFLIGHT FAILED（{len(problems)}/{len(mutants)} 条）:")
        for problem in problems:
            print(f"  {problem}")
        return 1

    baseline: dict[tuple[str, str], bool] = {}
    timed_out: list[str] = []
    hang_kills: list[str] = []
    no_output: list[str] = []
    survived = 0
    false_kills: list[str] = []
    crash_kills: list[str] = []
    unnamed: list[str] = []
    for mutant in mutants:
        key = (mutant["pkg"], mutant["filter"], runner_of(mutant))
        pkg, filt, runner = key
        if key not in baseline:
            result = run_tests(pkg, filt, runner)
            if result.returncode == TIMEOUT_RC:
                print(f"BASELINE TIMEOUT：{key}\n{result.stdout[-1200:]}")
                return 1
            if result.returncode != 0:
                print(f"BASELINE RED: {key}\n{result.stdout[-2000:]}")
                return 1
            baseline[key] = True

        path = ROOT / mutant["file"]
        original_bytes = path.read_bytes()
        # 与 preflight 的 read_text 同口径（通用换行）：CRLF 工作树里锚点仍按 \n 匹配
        original = original_bytes.decode("utf-8").replace("\r\n", "\n")
        count = original.count(mutant["old"])
        if count != 1:
            print(f"BAD MUTANT {mutant['name']}: anchor 出现 {count} 次")
            return 1

        # newline="" 是硬要求：默认写回会把整篇 \n 翻成 \r\n（Windows），
        # bash/sh 见到 \r 直接语法错，结果被误判成"崩溃"而不是断言——本地与 CI 就分叉了。
        write_atomic(path, original.replace(mutant["old"], mutant["new"]).encode("utf-8"))
        # 声明了 hang_secs 的变异：拆掉终止守卫后测试永不返回，短超时即可判死循环
        hang = hang_secs_of(mutant)
        try:
            result = run_tests(pkg, filt, runner, timeout=hang)
        finally:
            # 恢复按字节，保证工作树回到原样（换行风格也不许被门顺手改掉）
            write_atomic(path, original_bytes)
            assert path.read_bytes() == original_bytes, "恢复失败"

        if result.returncode == TIMEOUT_RC:
            if hang is not None:
                hang_kills.append(mutant["name"])
                print(f"HANG-KILL {mutant['name']}  [登记表声明的死循环 {hang:.0f}s：无断言可钉，不计入断言抓住]")
                continue
            timed_out.append(mutant["name"])
            print(f"TIMEOUT   {mutant['name']}  [没结论：不算抓住，也不算存活]")
            continue
        if result.returncode == 0:
            survived += 1
            print(f"SURVIVED  {mutant['name']}")
            continue
        out = result.stdout
        runner = runner_of(mutant)
        kind, detail = classify(out, result.returncode, runner)
        if kind == "inconclusive":
            no_output.append(mutant["name"])
            print(f"NO-OUTPUT {mutant['name']}  [没结论：{detail}]")
            continue
        if kind == "false_kill":
            false_kills.append(mutant["name"])
            print(f"FALSE-KILL {mutant['name']}  [{runner} 编译/语法红：测试没跑，不算锁]")
            continue
        if kind == "crash":
            crash_kills.append(mutant["name"])
            print(f"CRASH-KILL {mutant['name']}  [崩溃红 {hex(result.returncode & 0xFFFFFFFF)}：{detail[:100]}]")
            continue
        if kind == "unnamed":
            unnamed.append(mutant["name"])
            print(f"UNNAMED   {mutant['name']}  [既无断言也无崩溃签名，多半是环境/工具链坏了]")
            continue
        print(f"caught    {mutant['name']}  [{detail}]")

    killed, red = tally(
        len(mutants), survived, len(false_kills), len(crash_kills),
        len(timed_out), len(hang_kills), len(no_output), len(unnamed),
    )
    print(
        f"\n断言抓住 {killed}，死循环抓住 {len(hang_kills)}，崩溃抓住 {len(crash_kills)}，"
        f"编译红假杀 {len(false_kills)}，存活 {survived}，超时 {len(timed_out)}，"
        f"无输出 {len(no_output)}，未点名 {len(unnamed)}（共 {len(mutants)}）"
    )
    for name in timed_out:
        print(f"  TIMEOUT {name}")
    for name in no_output:
        print(f"  NO-OUTPUT {name}")
    for name in false_kills:
        print(f"  FALSE-KILL {name}")
    if hang_kills:
        print("  HANG-KILL（登记表声明的必然死循环：拆掉守卫会转不停，没有断言能钉住）：")
        for name in hang_kills:
            print(f"    - {name}")
    if crash_kills:
        print("  CRASH-KILL（证明守卫移除会炸，但不是断言钉住）：")
        for name in crash_kills:
            print(f"    - {name}")
    if unnamed:
        print(f"  未点名（既无断言也无崩溃签名）: {', '.join(unnamed)}")
    if red:
        print("⇒ 门判红：假杀的条目要么改成语义变异（编得过、只改行为），要么删除；"
              "超时/无输出的条目要么等锁空闲重跑，要么抬 MUTANT_TIMEOUT_SECS 并写理由；"
              "未点名的条目要先把它跑红的原因查清（盘满 / cargo 缺失 / harness 自身报错）；"
              "确实只会死循环的条目在登记表里写 hang_secs 声明，别让它冒充抓住。")
        return 1
    return 0


def parse_shard(text: str) -> tuple[int, int]:
    match = re.fullmatch(r"(\d+)/(\d+)", text.strip())
    if not match:
        print(f"--shard 形状不对: {text!r}（要 i/n，例如 1/4）⇒ 用法错误", file=sys.stderr)
        raise SystemExit(2)
    i, n = int(match.group(1)), int(match.group(2))
    if n < 1 or not 1 <= i <= n:
        print(f"--shard 越界: {text!r}（要 1 ≤ i ≤ n，n ≥ 1）⇒ 用法错误", file=sys.stderr)
        raise SystemExit(2)
    return i, n


def parse_partition(text: str) -> int:
    match = re.fullmatch(r"\d+", text.strip())
    if not match or int(match.group(0)) < 1:
        print(f"--partition 形状不对: {text!r}（要正整数，例如 4）⇒ 用法错误", file=sys.stderr)
        raise SystemExit(2)
    return int(match.group(0))


def parse_args(argv: list[str]) -> tuple[list[str], str, tuple[int, int] | None, int, str, list[str]]:
    pkgs: list[str] = []
    only = ""
    shard: tuple[int, int] | None = None
    partition = 0
    logs = ""
    files: list[str] = []
    rest = list(argv)
    while rest:
        arg = rest.pop(0)
        if arg == "--pkg" and rest:
            pkgs.append(rest.pop(0))
        elif arg.startswith("--pkg="):
            pkgs.append(arg.split("=", 1)[1])
        elif arg == "--only" and rest:
            only = rest.pop(0)
        elif arg.startswith("--only="):
            only = arg.split("=", 1)[1]
        elif arg == "--shard" and rest:
            shard = parse_shard(rest.pop(0))
        elif arg.startswith("--shard="):
            shard = parse_shard(arg.split("=", 1)[1])
        elif arg == "--partition" and rest:
            partition = parse_partition(rest.pop(0))
        elif arg.startswith("--partition="):
            partition = parse_partition(arg.split("=", 1)[1])
        elif arg == "--logs" and rest:
            logs = rest.pop(0)
        elif arg.startswith("--logs="):
            logs = arg.split("=", 1)[1]
        elif arg == "--files" and rest:
            files = [part for part in rest.pop(0).split(",") if part]
        elif arg.startswith("--files="):
            files = [part for part in arg.split("=", 1)[1].split(",") if part]
        else:
            raise SystemExit(f"未知参数: {arg}")
    return pkgs, only, shard, partition, logs, files


def self_test() -> int:
    """门自身判定链的自测：合成输出逐类判定 + 临时夹具验 preflight/has_own_logic 边界。

    这不是"再跑一遍变异"：不碰 cargo，秒级（两条超时夹具各等 0.5 秒与 6 秒），
    专钉门里最容易改坏的那几行
    （编译红当抓住、崩溃当断言、空输出当抓住、脚本落到 cargo、空集放行，这五类都真踩过）。
    失败打印 `FAIL <例名>`（与门脚本族同口径），全过打印 `self-test: N/N passed`。
    """
    total = 0
    failures: list[str] = []

    def check(label: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(f"FAIL {label}: 期望 {expected!r}，实际 {actual!r}")

    # 一、五类裁决：编译红不算锁、崩溃不算断言、空输出不算抓住
    check("cargo 编译红→false_kill", classify("error[E0308]: mismatched types", 1, "cargo")[0], "false_kill")
    check("cargo 栈溢出→crash", classify("thread 'x' has overflowed its stack", 134, "cargo")[0], "crash")
    check("cargo 0xc00000fd→crash", classify("boom", 0xC00000FD, "cargo")[0], "crash")
    check(
        "cargo 点名测试→caught",
        classify("---- t1 stdout ----\nthread 't1' panicked", 101, "cargo"),
        ("caught", "t1 断言失败"),
    )
    check("cargo 无痕迹→unnamed", classify("nothing useful", 1, "cargo")[0], "unnamed")
    check("空输出→没结论（不算抓住）", classify("", 1, "cargo")[0], "inconclusive")
    check("只有空白也算空输出→没结论", classify("\n  \n", 101, "python")[0], "inconclusive")
    check("python SyntaxError→false_kill", classify("SyntaxError: invalid syntax", 1, "python")[0], "false_kill")
    check(
        "python FAIL 点名→caught",
        # 真脚本里 FAIL 行与 `self-test:` 计数行是两行，门会把计数拼在后面
        classify("FAIL red: stale baseline\nself-test: 9/10 passed", 1, "python"),
        ("caught", "FAIL red: stale baseline ｜ self-test: 9/10 passed"),
    )
    check("python 裸 traceback→crash", classify("Traceback (most recent call last):\nKeyError: 1", 1, "python")[0], "crash")
    check(
        "files 过滤只留改动文件",
        [m["file"] for m in select([], ["crates/trit-wspace/src/trit.rs"])],
        ["crates/trit-wspace/src/trit.rs"],
    )
    check("files 过滤命中 0 条", select([], ["crates/trit-wspace/src/nope.rs"]), [])
    check("python AssertionError→caught", classify("AssertionError: 断言失败: x", 1, "python")[0], "caught")
    check(
        "断言文本里的 SyntaxError 不算编译红",
        classify("FAIL python SyntaxError→false_kill: 期望 a，实际 b\nself-test: 26/27 passed", 1, "python")[0],
        "caught",
    )
    check("bash 中文语法红→false_kill", classify("行 13: 未预期的记号 附近有语法错误", 2, "bash")[0], "false_kill")
    check(
        "bash FAIL 点名→caught",
        classify("FAIL 自测：run 失败后没有中止脚本", 1, "bash"),
        ("caught", "FAIL 自测：run 失败后没有中止脚本"),
    )

    # 一b、计数口径：unnamed 不计算住且判红（环境坏了把整门"绿化"是踩过的假绿）
    check("全抓住→不判红", tally(3, 0, 0, 0, 0, 0, 0, 0), (3, False))
    check("未点名不算抓住且判红", tally(3, 0, 0, 0, 0, 0, 0, 1), (2, True))
    check("崩溃单列但不判红", tally(3, 0, 0, 1, 0, 0, 0, 0), (2, False))
    check("死循环单列但不判红", tally(3, 0, 0, 0, 0, 1, 0, 0), (2, False))
    check(
        "超时/无输出任一即判红",
        [tally(3, 0, 0, 0, 1, 0, 0, 0)[1], tally(3, 0, 0, 0, 0, 0, 1, 0)[1]],
        [True, True],
    )
    check(
        "缩进两格的 FAIL 也算点名（ci_budget/secret_wiring/oops 的印法）",
        classify("  FAIL red: stale baseline\nself-test: 9/10 passed", 1, "python"),
        ("caught", "FAIL red: stale baseline ｜ self-test: 9/10 passed"),
    )
    check(
        "缩进 FAIL 行里的 SyntaxError 同样不算编译红",
        classify("  FAIL x: 文案里有 SyntaxError 字样\nself-test: 2/3 passed", 1, "python")[0],
        "caught",
    )

    # 二、runner 推断：脚本绝不能落到 cargo 去跑
    check("runner .py→python", runner_of({"pkg": "scripts/x.py"}), "python")
    check("runner .sh→bash", runner_of({"pkg": "crates/a/scripts/x.sh"}), "bash")
    check("runner crate→cargo", runner_of({"pkg": "trit-core"}), "cargo")
    check("runner 显式优先", runner_of({"pkg": "scripts/x.py", "runner": "cargo"}), "cargo")

    # 三、空集 fail-closed：没有登记就"绿"没有含义（真跑 main，输出吞掉免得像报错）
    def rc_of(targets: list[str]) -> object:
        # 判定链被改坏时 main 可能直接抛异常（例如 EXEMPT[pkg] KeyError）；
        # 把异常也变成可比较的返回值，这样它会以断言失败被点名，而不是被算成"崩溃红"。
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                return main(targets)
        except Exception as exc:
            return f"异常 {type(exc).__name__}"

    empty_rc = rc_of(["no-such-target-xyz"])
    exempt_rc = rc_of(["sansheng"])
    # 多目标一起选中时，0 条的那个不能被别的目标"带绿"
    partial_rc = rc_of(["scripts/gen_docs_charts.py", "no-such-target-xyz"])
    covered_rc = rc_of(["sansheng", "scripts/gen_docs_charts.py"])
    saved = MUTANTS[:]
    try:
        MUTANTS.clear()
        whole_empty_rc = rc_of([])
    finally:
        MUTANTS.extend(saved)
    check("空集未豁免→判红", empty_rc, 1)
    # 这三条的夹具是主仓里真实存在的目标（伞包 sansheng、无自测面的 gen_docs_charts.py）；
    # 独立镜像仓里没有 ⇒ 大声 SKIP，不许把它们算成"通过"。
    has_ambient = (ROOT / "crates" / "sansheng").is_dir() and (ROOT / "scripts" / "gen_docs_charts.py").is_file()
    if has_ambient:
        check("空集但自证豁免→放行", exempt_rc, 0)
        check("多目标里 0 条的那个→判红", partial_rc, 1)
        check("全部是自证豁免→放行", covered_rc, 0)
    else:
        print("SKIP 豁免/多目标 3 条：本仓没有夹具目标（主仓专属）")
    check("全仓登记为空→判红", whole_empty_rc, 1)
    check("选中过滤", [m["pkg"] for m in select(["sansheng"])], [])
    check("空选择=全表", len(select([])), len(MUTANTS))
    # 豁免项自己也要可复核：路径写错时 has_own_logic 会返回 False，豁免会静静地套在空气上
    if (ROOT / "crates").is_dir():
        for pkg in EXEMPT:
            exists = pkg.endswith((".py", ".sh")) and (ROOT / pkg).is_file()
            check(f"EXEMPT 路径存在：{pkg}", exists or (ROOT / "crates" / pkg).is_dir(), True)
    else:
        print("SKIP EXEMPT 台账对账：本仓没有 crates/ 目录（台账属于主仓）")

    # 四、豁免过期与 preflight 边界（按语言分判），用临时夹具，不碰真仓
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "crates" / "umbrella" / "src").mkdir(parents=True)
        (root / "crates" / "umbrella" / "src" / "lib.rs").write_text("pub use trit_core::*;\n", encoding="utf-8")
        (root / "crates" / "real" / "src").mkdir(parents=True)
        (root / "crates" / "real" / "src" / "lib.rs").write_text("pub fn f() -> u8 {\n    1\n}\n", encoding="utf-8")
        (root / "scripts").mkdir()
        (root / "scripts" / "plain.py").write_text("print(1)\n", encoding="utf-8")
        (root / "scripts" / "gate.py").write_text("x = 1\ndef self_test():\n    return 0\ny = 2\n", encoding="utf-8")
        (root / "scripts" / "gate.sh").write_text("a=1\nself_test() {\n  :\n}\nb=2\n", encoding="utf-8")
        check("伞包（只 pub use）→不过期", has_own_logic("umbrella", root=root), False)
        check("自有 fn→过期", has_own_logic("real", root=root), True)
        check("脚本无自测面→不过期", has_own_logic("scripts/plain.py", root=root), False)
        check("python 有自测面→过期", has_own_logic("scripts/gate.py", root=root), True)
        check("bash 有自测面→过期", has_own_logic("scripts/gate.sh", root=root), True)

        # 三b、登记表拆成 mutants/<目标>.json 之后，loader 自己必须钉得住
        with tempfile.TemporaryDirectory() as md:
            mdir = pathlib.Path(md)
            (mdir / "b.json").write_text(
                '[{"name":"y","pkg":"p","file":"f","old":"a","new":"b"}]', encoding="utf-8")
            (mdir / "a.json").write_text(
                '[{"name":"x","pkg":"p","file":"f","old":"a","new":"b"}]', encoding="utf-8")
            check("拆开的表能拼回来", len(load_mutants(mdir)), 2)
            check("拼接顺序按文件名稳定（否则 diff 每次抖）",
                  [entry["name"] for entry in load_mutants(mdir)], ["x", "y"])
            (mdir / "bad.json").write_text('{"不是":"数组"}', encoding="utf-8")
            try:
                load_mutants(mdir)
                problem = "没报错"
            except SystemExit as exc:
                problem = str(exc)
            check("形状不对要报出是哪个文件", "bad.json" in problem, True)
            try:
                load_mutants(mdir / "没有这个目录")
                missing = "没报错"
            except SystemExit as exc:
                missing = str(exc)
            check("目录不存在要判红（没表不等于没变异）", "判红" in missing, True)

        def fixture(file: str, old: str, runner: str) -> list[dict]:
            return [{"name": "fixture", "file": file, "old": old, "pkg": file, "filter": "", "runner": runner}]

        check("preflight 锚点在 python 自测面之后→报错", len(preflight(fixture("scripts/gate.py", "y = 2", "python"), root=root)), 1)
        check("preflight 锚点在自测面之前→放行", preflight(fixture("scripts/gate.py", "x = 1", "python"), root=root), [])
        check("preflight 锚点在 bash 自测面之后→报错", len(preflight(fixture("scripts/gate.sh", "b=2", "bash"), root=root)), 1)

    # 五、门自己的五条反脆弱阀门：超时 / 杀整棵树 / 死循环声明 / 原子写 / 单实例锁（都是本机踩出来的）
    slow = run_command([sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.5)
    check("超时必须返回哨兵 rc（不是假装跑完）", slow.returncode == TIMEOUT_RC, True)
    check("超时要点名 MUTANT TIMEOUT", "MUTANT TIMEOUT" in slow.stdout, True)
    check("超时要指出 target 目录锁这个成因", "file lock" in slow.stdout, True)
    fast = run_command([sys.executable, "-c", "print(1)"], timeout=30)
    check("给足超时的正常命令不该被判超时（反证）", fast.returncode == 0, True)
    check("正常命令不该带超时字样（反证）", "MUTANT TIMEOUT" in fast.stdout, False)

    check("声明 hang_secs 的变异用短超时", hang_secs_of({"hang_secs": 300}), 300.0)
    check("没声明就是 None（走默认 900）", hang_secs_of({}), None)
    check("hang_secs=0 不算声明（0 秒超时会假杀一切）", hang_secs_of({"hang_secs": 0}), None)

    # 超时只杀直接子进程是不够的：cargo 死了，它拉起的测试 exe 还在原地空转烧核
    # （2026-11-05 本地实测 pid 27480，CPU 每秒 +1，杀掉 cargo 之后它照样活着）。
    with tempfile.TemporaryDirectory() as td:
        pid_file = pathlib.Path(td) / "kid.pid"
        pathlib.Path(td, "kid.py").write_text(
            "import os, time\n"
            'with open(os.environ["MUTANT_KID_PID_FILE"], "w") as f:\n'
            "    f.write(str(os.getpid()))\n"
            "time.sleep(120)\n",
            encoding="utf-8",
        )
        kid_py = str(pathlib.Path(td, "kid.py"))
        pathlib.Path(td, "parent.py").write_text(
            "import os, subprocess, sys, time\n"
            'subprocess.Popen([sys.executable, os.environ["MUTANT_KID_PY"]])\n'
            "time.sleep(120)\n",
            encoding="utf-8",
        )
        parent_py = str(pathlib.Path(td, "parent.py"))
        # 夹具靠环境变量认路：run_command 继承本进程环境，改完必须清掉。
        os.environ["MUTANT_KID_PID_FILE"] = str(pid_file)
        os.environ["MUTANT_KID_PY"] = kid_py
        try:
            tree = run_command([sys.executable, parent_py], timeout=6)
        finally:
            os.environ.pop("MUTANT_KID_PID_FILE", None)
            os.environ.pop("MUTANT_KID_PY", None)
        check("夹具要真的把孙进程起来（否则这条白测）", pid_file.exists(), True)
        check("父命令超时被判哨兵 rc", tree.returncode == TIMEOUT_RC, True)
        if pid_file.exists():
            kid_pid = int(pid_file.read_text(encoding="utf-8"))
            # 给 5 秒收尸窗口：Linux 上刚被 SIGKILL 的孩子会以僵尸身份短暂"还在"
            # （CI 上就是把僵尸读成"没杀掉"，自测自己假红了一次）。
            gone = False
            for _ in range(50):
                if proc_gone(kid_pid):
                    gone = True
                    break
                time.sleep(0.1)
            check("超时要把孙进程（真正的测试进程）一起端掉", gone, True)

    with tempfile.TemporaryDirectory() as td:
        target = pathlib.Path(td) / "src.txt"
        original = "原始" + chr(10)
        target.write_text(original, encoding="utf-8")
        write_atomic(target, ("变异后" + chr(10)).encode("utf-8"))
        check("原子写的内容要落对", target.read_text(encoding="utf-8"), "变异后" + chr(10))
        check("原子写不许留临时文件", list(pathlib.Path(td).glob("*.mutant-tmp")), [])
        write_atomic(target, original.encode("utf-8"))
        check("恢复要回到原文（幂等）", target.read_bytes(), original.encode("utf-8"))
        check("恢复后同样不许留临时文件", list(pathlib.Path(td).glob("*.mutant-tmp")), [])

    # 锁的断言一律跑在注入的临时目录里：CI 自测时外层门正持有仓里那把真锁（踩过）。
    # 守卫问的是"没碰那把真锁"，不是"那把锁不存在"——外层门正在跑时它本来就该在。
    real_pid_file = LOCK_DIR / "pid"
    before = real_pid_file.read_text(encoding="utf-8") if real_pid_file.exists() else None
    with tempfile.TemporaryDirectory() as lock_box:
        os.environ["MUTATION_CHECK_LOCK_DIR"] = str(pathlib.Path(lock_box) / "lock")
        try:
            ok, _ = acquire_lock()
            check("第一次拿锁要成功", ok, True)
            ok2, why = acquire_lock()
            check("同一个仓里第二个门必须被拒（互抢 target 锁会一起挂死）", ok2, False)
            check("拒绝时要点名持有者 pid", str(os.getpid()) in why, True)
            release_lock()
            check("释放后要能再拿", acquire_lock()[0], True)
            release_lock()
            lock_path().mkdir(parents=True, exist_ok=True)
            (lock_path() / "pid").write_text("4000000", encoding="utf-8")
            stale_ok, stale_note = acquire_lock()
            check("陈旧锁（持有者已死）要能接管，不能永久卡住门", stale_ok, True)
            check("接管要说明是陈旧锁", "陈旧" in stale_note, True)
            release_lock()
            check("锁释放后目录要消失", lock_path().exists(), False)
        finally:
            os.environ.pop("MUTATION_CHECK_LOCK_DIR", None)
        check("自测没碰仓里那把真锁（外层门在跑时也不许接管或释放它）",
              (real_pid_file.read_text(encoding="utf-8") if real_pid_file.exists() else None) == before,
              True)

    # 六、分片与聚合：稳定划分 / 越界拒 / 真空集 / preflight 只查所选 / 逐条对账
    check("shard 形状解析", parse_shard("1/4"), (1, 4))
    check("shard 末片", parse_shard("4/4"), (4, 4))
    for bad in ("0/4", "5/4", "1/0", "abc", "1/4/2", "-1/4", "1.5/4", ""):
        try:
            parse_shard(bad)
            code: object = "没报错"
        except SystemExit as exc:
            code = exc.code
        check(f"shard 越界/形状错必须拒（rc2）: {bad!r}", code, 2)
    try:
        parse_partition("0")
        pcode: object = "没报错"
    except SystemExit as exc:
        pcode = exc.code
    check("partition 0 必须拒（rc2）", pcode, 2)
    check("partition 形状解析", parse_partition("4"), 4)

    synth = [
        {"name": f"entry-{k}", "pkg": "p", "file": f"f{k % 7}.rs", "old": "a", "new": "b", "filter": ""}
        for k in range(40)
    ]
    shards = [select_shard(synth, i, 4) for i in range(1, 5)]
    union = [m["name"] for s in shards for m in s]
    check("分片并集=全表", sorted(union), sorted(m["name"] for m in synth))
    check("分片互不重叠", len(union), len(set(union)))
    check("分片计数和=全表", sum(len(s) for s in shards), len(synth))
    check("n=1 就是全表", select_shard(synth, 1, 1), synth)
    check("空表分片还是空", select_shard([], 1, 4), [])
    rebuilt = [dict(m) for m in reversed(synth)]
    check("分片不依赖登记顺序与对象身份（重建列表后同片归属不变）",
          [sorted(m["name"] for m in select_shard(rebuilt, i, 4)) for i in range(1, 5)],
          [sorted(m["name"] for m in s) for s in shards])
    check("分片键同时认 file 与 name",
          shard_key({"file": "a.rs", "name": "x"}) != shard_key({"file": "b.rs", "name": "x"}), True)
    rep = partition_report(synth, 4)
    check("划分报告：干净划分", (rep["covers_all"], rep["disjoint"], sum(rep["counts"])), (True, True, 40))

    sample = chr(10).join([
        "caught    claims: repair 不改重复 id  [t1 断言失败]",
        "CRASH-KILL 栈溢出  [崩溃红 0xc00000fd]",
        "HANG-KILL 死循环  [登记表声明]",
        "SURVIVED  存活者",
        "TIMEOUT   没结论  [x]",
        "NO-OUTPUT 空  [x]",
        "FALSE-KILL 编译红  [x]",
        "UNNAMED   未点名  [x]",
        "  TIMEOUT 没结论  [缩进汇总，不是判决]",
        "    - 死循环",
        "断言抓住 8，死循环抓住 1，崩溃抓住 1（共 10）",
    ])
    verdicts = verdicts_from_log(sample)
    check("判决行只认行首（缩进汇总不重计）", len(verdicts), 8)
    check("带空格/中文的名字读全", verdicts[0], ("claims: repair 不改重复 id", "caught"))
    check("八类判决都认得", sorted({k for _, k in verdicts}),
          sorted(["caught", "CRASH-KILL", "HANG-KILL", "SURVIVED", "TIMEOUT", "NO-OUTPUT", "FALSE-KILL", "UNNAMED"]))

    def fixture_entry(name: str) -> dict:
        return {"name": name, "pkg": "sansheng", "file": "no/such/file.rs", "old": "a", "new": "b", "filter": ""}

    entry_a = fixture_entry("shard-fixture-a")
    shard_a = shard_key(entry_a) % 4 + 1
    # 键退化成对象身份时 id%4 恒 0（地址对齐）全落片 1：夹具带兜底才构造得出来，让变异死在断言而非 StopIteration。
    candidates = [fixture_entry(f"shard-fixture-b{k}") for k in range(100)]
    entry_b = next((c for c in candidates if shard_key(c) % 4 + 1 != shard_a), candidates[0])
    shard_b = shard_key(entry_b) % 4 + 1

    saved = MUTANTS[:]
    with tempfile.TemporaryDirectory() as box:
        root = pathlib.Path(box)
        os.environ["MUTATION_CHECK_LOCK_DIR"] = str(root / "lock")
        try:
            MUTANTS.clear()
            MUTANTS.extend([entry_a, entry_b])

            def shard_rc(shard: tuple[int, int]) -> tuple[object, str]:
                buf = io.StringIO()
                try:
                    with contextlib.redirect_stdout(buf):
                        rc: object = main(["sansheng"], shard=shard)
                except Exception as exc:
                    rc = f"异常 {type(exc).__name__}"
                return rc, buf.getvalue()

            other = next(s for s in range(1, 5) if s not in (shard_a, shard_b))
            rc_empty, out_empty = shard_rc((other, 4))
            check("真空片放行（rc0）", rc_empty, 0)
            check("真空片要明说是真空集", "真空集" in out_empty, True)
            rc_run, out_run = shard_rc((shard_a, 4))
            check("选中片真的窄化（夹具走到 preflight）", rc_run, 1)
            check("选中片要大声声明部分跑", f"SHARD {shard_a}/4" in out_run, True)
            check("preflight 只查所选（1/1 而不是 2/2）", "PREFLIGHT FAILED（1/1 条）" in out_run, True)
            check("所选条目被点名", "shard-fixture-a" in out_run, True)
            check("别片条目不被 preflight", "shard-fixture-b" not in out_run, True)

            MUTANTS.clear()
            MUTANTS.extend(synth)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                dispatch_rc = main([], partition=4)
            check("main --partition 直连（干净划分放行）", dispatch_rc, 0)
            check("partition 模式打出 PARTITION 读数", "PARTITION 4 片" in buf.getvalue(), True)

            original_select = globals()["select_shard"]
            try:
                globals()["select_shard"] = lambda ms, i, n: original_select(ms, i, n)[:-1]
                buf = io.StringIO()
                with contextlib.redirect_stdout(buf):
                    drop_rc = partition_audit(4, None)
                check("聚合：漏片必须判红", drop_rc, 1)
                check("漏片要点名", "有漏片" in buf.getvalue(), True)
                globals()["select_shard"] = lambda ms, i, n: original_select(ms, i, n) + ([ms[0]] if i == 1 and ms else [])
                buf = io.StringIO()
                with contextlib.redirect_stdout(buf):
                    dup_rc = partition_audit(4, None)
                check("聚合：重复落片必须判红", dup_rc, 1)
                check("重复落片要点名", "重复" in buf.getvalue(), True)
            finally:
                globals()["select_shard"] = original_select

            log_dir = root / "logs"
            log_dir.mkdir()
            names = [m["name"] for m in synth]
            half = len(names) // 2
            caught_lines = [f"caught    {n}  [t 断言失败]" for n in names]

            def logs_rc(lines1: list[str], lines2: list[str]) -> tuple[object, str]:
                (log_dir / "shard-1.log").write_text(chr(10).join(lines1) + chr(10), encoding="utf-8")
                (log_dir / "shard-2.log").write_text(chr(10).join(lines2) + chr(10), encoding="utf-8")
                buf = io.StringIO()
                with contextlib.redirect_stdout(buf):
                    rc: object = partition_audit(4, log_dir)
                return rc, buf.getvalue()

            rc_full, out_full = logs_rc(caught_lines[:half], caught_lines[half:])
            check("聚合：全覆盖放行", rc_full, 0)
            check("聚合要打出 COVERAGE 读数", "COVERAGE 2 份日志" in out_full, True)
            rc_miss, out_miss = logs_rc(caught_lines[:half], caught_lines[half:-1])
            check("聚合：缺判决必须判红", rc_miss, 1)
            check("缺判决要点名到条目", "缺判决" in out_miss and names[-1] in out_miss, True)
            rc_unk, out_unk = logs_rc(caught_lines[:half], caught_lines[half:] + ["caught    ghost-entry  [x]"])
            check("聚合：不认识条目必须判红", rc_unk, 1)
            check("不认识条目要点名", "不认识条目" in out_unk and "ghost-entry" in out_unk, True)
            rc_dup, out_dup = logs_rc(caught_lines[:half] + [caught_lines[0]], caught_lines[half:])
            check("聚合：重复判决必须判红", rc_dup, 1)
            check("重复判决要点名", "重复判决" in out_dup, True)
            rc_bad, out_bad = logs_rc(caught_lines[:half], caught_lines[half:-1] + [f"SURVIVED  {names[-1]}"])
            check("聚合：非杀死判决必须判红", rc_bad, 1)
            check("非杀死要点名", "非杀死" in out_bad and names[-1] in out_bad, True)
            empty_dir = root / "empty"
            empty_dir.mkdir()
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc_nolog = partition_audit(4, empty_dir)
            check("聚合：读不到日志必须判红", rc_nolog, 1)
            check("无日志要点名 NO SHARD LOGS", "NO SHARD LOGS" in buf.getvalue(), True)

            MUTANTS.clear()
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc_empty_reg = partition_audit(4, None)
            check("聚合：空登记表同样判红", rc_empty_reg, 1)
        finally:
            MUTANTS.clear()
            MUTANTS.extend(saved)
            os.environ.pop("MUTATION_CHECK_LOCK_DIR", None)

    if failures:
        for line in failures:
            print(line)
        print(f"self-test: {total - len(failures)}/{total} passed")
        return 1
    print(f"self-test: {total}/{total} passed")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv[1:]:
        sys.exit(self_test())
    sys.exit(main(*parse_args(sys.argv[1:])))
