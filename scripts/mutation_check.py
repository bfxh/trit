#!/usr/bin/env python3
"""变异门：对关键规则做单点文本变异，跑对应回归；测试必须变红。

用法：python scripts/mutation_check.py [--pkg <目标>]...
目标 = crate 名，或带自测面的脚本路径（`scripts/*.py`、`crates/*/scripts/*.sh`）。
每个变异都会在 finally 里**按字节**恢复原文件；退出码 1 表示有 mutant 存活。

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
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]

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


def run_tests(pkg: str, test_filter: str, runner: str = "cargo") -> subprocess.CompletedProcess[str]:
    if runner in ("python", "bash"):
        argv = [sys.executable if runner == "python" else "bash", pkg, "--self-test"]
    else:
        argv = ["cargo", "test", "-q", "-p", pkg, test_filter, "--locked"]
    return subprocess.run(
        argv,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


# 显式豁免：只有"整个 crate 不含自有逻辑"这种**可以被机器复核**的理由才收在这里。
# 复核在 has_own_logic()：一旦该包出现了 fn/impl，豁免自动失效并判红（防止理由烂掉）。
EXEMPT: dict[str, str] = {
    "sansheng": "三值工具箱的伞包：lib.rs 只有 pub use 转发与文档 doctest，无自有规则",
    "scripts/gen_docs_charts.py": "出图脚本，无 --self-test 且非承重判定；不参与发布门禁",
    # 这两个是薄壳（5 / 19 行），没有可钉的判定；一旦有人给它们补 self_test()，豁免自动过期。
    "scripts/bench.sh": "基准薄壳（5 行）：只是把参数转给 cargo bench，无可变异判定",
    "scripts/publish.sh": "发布薄壳（19 行）：版本校验后转给 cargo publish，无可变异判定",
    "crates/trit-immune/scripts/sync-upstream.sh": "上游同步脚本：一键同步本就不成立（补丁表达不了删除），无 --self-test 面",
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


def select(pkgs: list[str]) -> list[dict]:
    if not pkgs:
        return MUTANTS
    return [mutant for mutant in MUTANTS if mutant["pkg"] in pkgs]


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
    """把一次变异运行的结果判成四类：caught / false_kill / crash / unnamed。

    rc==0 的"存活"由调用方先判，不在这里。
    返回 (kind, detail)：caught 的 detail 是点名，crash 的 detail 是崩溃提示。
    这一层是门最容易被改坏的地方（编译红当抓住、把崩溃当断言各踩过一次），
    所以抽成纯函数，由 `--self-test` 用合成输出逐类钉住。
    """
    syntax = RUNNERS[runner]["syntax"]
    # 只认工具链自己打的诊断：断言行的例名里可能出现 "SyntaxError" 这类字样，
    # 把断言文本当成编译失败就会把"真抓住"误判成假杀（门自测第一版就这么翻车）。
    diag = "\n".join(line for line in out.splitlines() if not line.startswith("FAIL "))
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
            lambda: next((l for l in out.splitlines() if l.startswith("FAIL ")), None),
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


def main(pkgs: list[str]) -> int:
    # Windows 控制台默认可能是 GBK；变异名里有中国字与 Ł，统一按 UTF-8 输出。
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    mutants = select(pkgs)
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
    problems = preflight(mutants)
    if problems:
        print(f"PREFLIGHT FAILED（{len(problems)}/{len(mutants)} 条）:")
        for problem in problems:
            print(f"  {problem}")
        return 1

    baseline: dict[tuple[str, str], bool] = {}
    survived = 0
    false_kills: list[str] = []
    crash_kills: list[str] = []
    unnamed: list[str] = []
    for mutant in mutants:
        key = (mutant["pkg"], mutant["filter"], runner_of(mutant))
        pkg, filt, runner = key
        if key not in baseline:
            result = run_tests(pkg, filt, runner)
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
        path.write_text(
            original.replace(mutant["old"], mutant["new"]),
            encoding="utf-8",
            newline="",
        )
        try:
            result = run_tests(pkg, filt, runner)
        finally:
            # 恢复按字节，保证工作树回到原样（换行风格也不许被门顺手改掉）
            path.write_bytes(original_bytes)
            assert path.read_bytes() == original_bytes, "恢复失败"

        if result.returncode == 0:
            survived += 1
            print(f"SURVIVED  {mutant['name']}")
            continue
        out = result.stdout
        runner = runner_of(mutant)
        kind, detail = classify(out, result.returncode, runner)
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
            print(f"caught    {mutant['name']}  [(未点名测试)]")
            continue
        print(f"caught    {mutant['name']}  [{detail}]")

    killed = len(mutants) - survived - len(false_kills) - len(crash_kills)
    print(
        f"\n断言抓住 {killed}，崩溃抓住 {len(crash_kills)}，编译红假杀 {len(false_kills)}，"
        f"存活 {survived}（共 {len(mutants)}）"
    )
    for name in false_kills:
        print(f"  FALSE-KILL {name}")
    if crash_kills:
        print("  CRASH-KILL（证明守卫移除会炸，但不是断言钉住）：")
        for name in crash_kills:
            print(f"    - {name}")
    if unnamed:
        print(f"  未点名（既无断言也无崩溃签名）: {', '.join(unnamed)}")
    if survived or false_kills:
        print("⇒ 门判红：假杀的条目要么改成语义变异（编得过、只改行为），要么删除。")
        return 1
    return 0


def parse_args(argv: list[str]) -> list[str]:
    pkgs: list[str] = []
    rest = list(argv)
    while rest:
        arg = rest.pop(0)
        if arg == "--pkg" and rest:
            pkgs.append(rest.pop(0))
        elif arg.startswith("--pkg="):
            pkgs.append(arg.split("=", 1)[1])
        else:
            raise SystemExit(f"未知参数: {arg}")
    return pkgs


def self_test() -> int:
    """门自身判定链的自测：合成输出逐类判定 + 临时夹具验 preflight/has_own_logic 边界。

    这不是"再跑一遍变异"：不碰 cargo，毫秒级，专钉门里最容易改坏的那几行
    （编译红当抓住、崩溃当断言、脚本落到 cargo、空集放行，这四类都真踩过）。
    失败打印 `FAIL <例名>`（与门脚本族同口径），全过打印 `self-test: N/N passed`。
    """
    total = 0
    failures: list[str] = []

    def check(label: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(f"FAIL {label}: 期望 {expected!r}，实际 {actual!r}")

    # 一、四类裁决：编译红不算锁、崩溃不算断言
    check("cargo 编译红→false_kill", classify("error[E0308]: mismatched types", 1, "cargo")[0], "false_kill")
    check("cargo 栈溢出→crash", classify("thread 'x' has overflowed its stack", 134, "cargo")[0], "crash")
    check("cargo 0xc00000fd→crash", classify("boom", 0xC00000FD, "cargo")[0], "crash")
    check(
        "cargo 点名测试→caught",
        classify("---- t1 stdout ----\nthread 't1' panicked", 101, "cargo"),
        ("caught", "t1 断言失败"),
    )
    check("cargo 无痕迹→unnamed", classify("nothing useful", 1, "cargo")[0], "unnamed")
    check("python SyntaxError→false_kill", classify("SyntaxError: invalid syntax", 1, "python")[0], "false_kill")
    check(
        "python FAIL 点名→caught",
        # 真脚本里 FAIL 行与 `self-test:` 计数行是两行，门会把计数拼在后面
        classify("FAIL red: stale baseline\nself-test: 9/10 passed", 1, "python"),
        ("caught", "FAIL red: stale baseline ｜ self-test: 9/10 passed"),
    )
    check("python 裸 traceback→crash", classify("Traceback (most recent call last):\nKeyError: 1", 1, "python")[0], "crash")
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
    sys.exit(main(parse_args(sys.argv[1:])))
