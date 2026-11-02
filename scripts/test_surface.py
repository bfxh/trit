#!/usr/bin/env python3
"""测试面体检：绿不等于跑了东西。

用法：
  python scripts/test_surface.py            # 体检，红了就退出非零
  python scripts/test_surface.py --update   # 收紧棘轮（只许把数字记小=测试变少时拒绝）
  python scripts/test_surface.py --json
  python scripts/test_surface.py --self-test

钉的是社区里三类"假绿"（本仓也真踩过前两类的坑）：
  1. **空壳测试**：`#[test] fn x() {}` 或函数体只剩注释——永远绿，永远不判任何东西；
  2. **无理由的忽略**：裸 `#[ignore]` / `.skip()` / `skipIf(...)` 不带原因——
     今天的"暂时跳过"就是明年的"没人记得为什么跳过"；
  3. **测试变少没人拦**：删测试和删产品代码是两回事，删掉的测试不会让任何门变红。
  外加对本仓很关键的一条：脚本声明了 `def self_test(` 但里面一条断言都没有 ⇒
  变异门拿它当判据就是空转（`has_own_logic` 只看到函数名，看不到它其实什么都没查）。

棘轮（和 module_budget 同口径）：台账里的数字只能变小（测试变多了要登记），
变大 = 有测试被删/被跳过 ⇒ 红；`--update` 拒绝增长。
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = pathlib.Path(__file__).resolve().parent / "test_surface_baseline.json"

# 一条断言长什么样（有这些才算"这个测试会判东西"）。
# 注意 `assert` 不带词边界：proptest 的 prop_assert_eq! 也必须算，否则一整类属性测试被误判。
ASSERT_ANY = re.compile(
    r"(assert|expect\(|unwrap\(|panic!|unreachable!|ensure|"
    r"must_pass|must_fail|check\(|EXPECT|raise AssertionError|test -|\[\[ )"
)
RUST_TEST = re.compile(r"#\[test\]")
RUST_IGNORE_BARE = re.compile(r"^[ \t]*#\[ignore\][ \t]*$", re.MULTILINE)
PY_SKIP_NOREASON = re.compile(r"@pytest\.mark\.skip\(\s*\)|\.skip\(\s*\)|@unittest\.skip\(\s*\)")
BASH_SELFTEST = re.compile(r"self_test\s*\(\s*\)")
PY_SELFTEST = re.compile(r"def self_test\(")


def blank_strings(text: str) -> str:
    """把字符串/注释 token 涂白（长度与换行不变），只留代码骨架。

    扫描器不能扫到自己的文档：本工具第一版就在 docstring 里的 `.skip()` 上把自己判红了。
    """
    import io
    import tokenize

    try:
        tokens = list(tokenize.generate_tokens(io.StringIO(text).readline))
    except (tokenize.TokenError, IndentationError, SyntaxError):
        return text  # 解析不动就当全是代码：宁可多报，不许漏报
    offsets, running = [], 0
    for line in text.splitlines(keepends=True):
        offsets.append(running)
        running += len(line)
    chars = list(text)

    def pos(row: int, col: int) -> int | None:
        if 1 <= row <= len(offsets):
            return offsets[row - 1] + col
        return None

    for token in tokens:
        if token.type not in (tokenize.STRING, tokenize.COMMENT):
            continue
        start = pos(token.start[0], token.start[1])
        end = pos(token.end[0], token.end[1])
        if start is None or end is None or end <= start:
            continue
        for index in range(start, min(end, len(chars))):
            if chars[index] not in "\n\r":
                chars[index] = " "
    return "".join(chars)



def strip_comments(text: str) -> str:
    """去掉行注释与 //块注释，用来判断"函数体是不是只剩注释"。"""
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r"#[ \t][^\n]*", "", text)  # python/bash 行注释（不误伤 #[test]）
    return text


def body_of(lines: list[str], start: int) -> str:
    """从 start 行往下，取第一个 `{` 与配对 `}` **之间**的内容。

    签名不算进体内：算进去的话 `fn hollow() { // 注释 }` 会被当成"有内容"，
    空壳检测就废了（第一版正是这样漏的）。
    """
    text = "\n".join(lines[start: start + 60])
    begin = text.find("{")
    if begin < 0:
        return ""
    depth = 0
    for index in range(begin, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[begin + 1: index]
    return text[begin + 1:]


def rust_findings(path: pathlib.Path, text: str) -> tuple[list[dict], int]:
    findings: list[dict] = []
    lines = text.splitlines()
    count = 0
    for index, line in enumerate(lines):
        if not RUST_TEST.search(line):
            continue
        count += 1
        body = strip_comments(body_of(lines, index + 1))
        # should_panic 通常写在 #[test] 之后，所以要看前后两边的属性行
        attrs = "\n".join(lines[max(0, index - 2): index] + lines[index + 1: index + 4])
        if not body.strip():
            findings.append({"kind": "空壳测试", "file": str(path), "line": index + 1,
                             "detail": "#[test] 的函数体是空的（只有括号或只有注释）——永远绿"})
        elif not ASSERT_ANY.search(body) and "should_panic" not in attrs:
            findings.append({"kind": "无断言测试", "file": str(path), "line": index + 1,
                             "blocking": False,
                             "detail": "#[test] 体内没有任何 assert/expect/panic 痕迹"})
    for match in RUST_IGNORE_BARE.finditer(text):
        findings.append({"kind": "无理由的 ignore", "file": str(path),
                         "line": text.count("\n", 0, match.start()) + 1,
                         "detail": "裸 #[ignore]：必须写 #[ignore = \"为什么\"]，否则没人知道能不能删"})
    return findings, count


def script_findings(path: pathlib.Path, text: str) -> tuple[list[dict], int]:
    findings: list[dict] = []
    is_python = path.suffix == ".py"
    code = blank_strings(text) if is_python else text
    count = len(re.findall(r"def test_\w+", code)) if is_python else \
        len(re.findall(r"must_pass|must_fail", code))
    selftest = (PY_SELFTEST if is_python else BASH_SELFTEST).search(code)
    if selftest:
        # 声明了自测就必须真有断言：否则变异门拿它当判据 = 空转
        tail = code[selftest.start():]
        if not ASSERT_ANY.search(strip_comments(tail)):
            findings.append({"kind": "空转自测", "file": str(path),
                             "line": text.count("\n", 0, selftest.start()) + 1,
                             "detail": "有 self_test() 但里面没有任何 check/assert 类调用——"
                                       "门会把它当判据，实际什么都没判"})
        count += 1
    if is_python:
        for match in PY_SKIP_NOREASON.finditer(code):
            findings.append({"kind": "无理由的 skip", "file": str(path),
                             "line": text.count("\n", 0, match.start()) + 1,
                             "detail": "skip() 没写 reason：一年后没人知道该不该恢复"})
    return findings, count


def units(root: pathlib.Path) -> dict[str, dict]:
    """每个 crate / 每个脚本一个计数单位。"""
    result: dict[str, dict] = {}
    for path in sorted(root.glob("crates/*/src/**/*.rs")) + sorted(root.glob("crates/*/tests/*.rs")):
        rel = path.relative_to(root).as_posix()
        crate = pathlib.PurePosixPath(rel).parts[1]
        entry = result.setdefault(f"crate/{crate}", {"tests": 0, "findings": []})
        findings, count = rust_findings(path, path.read_text(encoding="utf-8", errors="replace"))
        entry["tests"] += count
        entry["findings"].extend(findings)
    for pattern in ("scripts/*.py", "scripts/*.sh"):
        for path in sorted(root.glob(pattern)):
            findings, count = script_findings(path, path.read_text(encoding="utf-8", errors="replace"))
            entry = result.setdefault(f"script/{path.name}", {"tests": 0, "findings": []})
            entry["tests"] += count
            entry["findings"].extend(findings)
    return result


def ratchet(units: dict[str, dict], baseline: dict) -> list[dict]:
    findings: list[dict] = []
    for name, entry in sorted(units.items()):
        if name not in baseline:
            findings.append({"kind": "未登记的计数单位", "file": name,
                             "detail": f"新增单位 {entry['tests']} 条测试没进台账——"
                                       "先 --update 登记，之后只许变多不许变少"})
        elif entry["tests"] < baseline[name]:
            findings.append({"kind": "测试变少", "file": name,
                             "detail": f"{entry['tests']} < 台账 {baseline[name]}："
                                        f"少了 {baseline[name] - entry['tests']} 条，删测试不会让任何门变红，所以在这里拦"})
    for name in sorted(set(baseline) - set(units)):
        findings.append({"kind": "台账里的死单位", "file": name,
                         "detail": "台账登记了但现在扫不到——单位被删/改名了，先确认测试没跟着丢再 --update"})
    return findings


def scan(root: pathlib.Path, baseline_path: pathlib.Path) -> dict:
    baseline: dict = {}
    if baseline_path.is_file():
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    found = units(pathlib.Path(root))
    findings: list[dict] = []
    for entry in found.values():
        findings.extend(entry["findings"])
    findings.extend(ratchet(found, baseline))
    for finding in findings:
        finding.setdefault("blocking", True)
    return {
        "tool": "test-surface",
        "units": {name: entry["tests"] for name, entry in sorted(found.items())},
        "findings": findings,
        "blocking": sum(1 for f in findings if f["blocking"]),
        "warnings": sum(1 for f in findings if not f["blocking"]),
        "red": any(finding["blocking"] for finding in findings),
    }


def update_baseline(root: pathlib.Path, baseline_path: pathlib.Path) -> int:
    found = units(pathlib.Path(root))
    old = json.loads(baseline_path.read_text(encoding="utf-8")) if baseline_path.is_file() else {}
    grew = [f"{name} {old[name]} -> {entry['tests']}（测试变少了，--update 不接受）"
            for name, entry in sorted(found.items()) if name in old and entry["tests"] < old[name]]
    if grew:
        for line in grew:
            print(f"REFUSED {line}")
        print("--update 只收紧台账（变少=删测试，必须先把断言补回来）")
        return 1
    payload = {name: entry["tests"] for name, entry in sorted(found.items())}
    baseline_path.write_text(json.dumps(payload, ensure_ascii=False, indent=1, sort_keys=True) + "\n",
                             encoding="utf-8")
    print(f"台账已写：{len(payload)} 个单位，{sum(payload.values())} 条测试 → {baseline_path.name}")
    return 0


def self_test() -> int:
    """每条规则都要有一个真的会红的反证；绿例必须真跑。"""
    import subprocess
    import tempfile

    script = str(pathlib.Path(__file__).resolve())
    total, failures = 0, []

    def run(root: pathlib.Path, extra: list[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, script, "--root", str(root), "--baseline",
                               str(root / "bl.json"), *extra],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                              encoding="utf-8", errors="replace")

    def check(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "scripts").mkdir()

        # 正例：有断言、有理由、已登记 ⇒ 绿
        (root / "scripts" / "good.py").write_text(
            "def self_test():\n    check(\"x\", 1, 1)\n    return 0\n", encoding="utf-8")
        update_baseline(root, root / "bl.json")
        check("干净夹具判绿", run(root, []) .returncode, 0)

        # 反证 1：空壳 #[test]
        cdir = root / "crates" / "demo" / "src"
        cdir.mkdir(parents=True)
        (cdir / "lib.rs").write_text("#[test]\nfn hollow() {\n    // 还没写\n}\n", encoding="utf-8")
        update_baseline(root, root / "bl.json")
        out = run(root, []).stdout
        check("空壳测试被抓", "空壳测试" in out, True)
        check("有拦项时 rc=1", run(root, []).returncode, 1)

        # 反证 1b：断言在 helper 里的测试只警告，不拦（判红要准，不能把噪音算成拦截）
        (cdir / "lib.rs").write_text("#[test]\nfn via_helper() { exhaustive_all(2); }\n", encoding="utf-8")
        update_baseline(root, root / "bl.json")
        out = run(root, []).stdout
        check("无断言痕迹只给警告", "无断言测试" in out, True)
        check("警告不判红", run(root, []).returncode, 0)

        # 反证 2：有内容的测试不误报为空壳
        (cdir / "lib.rs").write_text("#[test]\nfn real() { assert_eq!(1, 1); }\n", encoding="utf-8")
        update_baseline(root, root / "bl.json")
        check("真测试不算空壳", "空壳测试" in run(root, []).stdout, False)

        # 反证 3：裸 #[ignore] 必须写理由
        (cdir / "lib.rs").write_text("#[test]\n#[ignore]\nfn parked() { assert!(true); }\n",
                                     encoding="utf-8")
        check("裸 ignore 被抓", "无理由的 ignore" in run(root, []).stdout, True)
        (cdir / "lib.rs").write_text('#[test]\n#[ignore = "等 #123 修完"]\nfn parked() { assert!(true); }\n',
                                     encoding="utf-8")
        check("带理由的 ignore 放行", "无理由的 ignore" in run(root, []).stdout, False)

        # 反证 4：skip() 不带 reason
        (root / "scripts" / "t.py").write_text(
            "import unittest\n\nclass T(unittest.TestCase):\n    def test_a(self):\n"
            "        self.skip()\n", encoding="utf-8")
        update_baseline(root, root / "bl.json")
        check("无理由 skip 被抓", "无理由的 skip" in run(root, []).stdout, True)

        # 反证 5：self_test 里一条断言都没有 = 门拿它当判据是空转
        (root / "scripts" / "t.py").write_text("def self_test():\n    return 0\n", encoding="utf-8")
        update_baseline(root, root / "bl.json")
        check("空转自测被抓", "空转自测" in run(root, []).stdout, True)

        # 反证 6：删掉一条测试 ⇒ 棘轮判红，且 --update 拒绝
        (root / "scripts" / "good.py").write_text("def other():\n    return 1\n", encoding="utf-8")
        out = run(root, []).stdout
        check("测试变少被抓", "测试变少" in out, True)
        check("--update 拒绝增长", run(root, ["--update"]).returncode, 1)

        # 反证 7：台账里留了已消失的单位
        (root / "scripts" / "good.py").unlink()
        check("死台账被抓", "台账里的死单位" in run(root, []).stdout, True)

        # 反证 8：新增单位没登记 ⇒ 红（不许悄悄加一块没人数的测试面）
        (root / "scripts" / "new.py").write_text(
            "def self_test():\n    check(\"y\", 1, 1)\n    return 0\n", encoding="utf-8")
        check("未登记单位被抓", "未登记的计数单位" in run(root, []).stdout, True)

        # 反证 9：扫描器不许扫到自己的文档（第一版就在 docstring 的 .skip() 上判红）
        (root / "scripts" / "doc.py").write_text(
            '\"\"\"用法里提到 self.skip() 与 def self_test( 只是说明。\"\"\"\n\n'
            'def self_test():\n    check("z", 1, 1)\n    return 0\n', encoding="utf-8")
        units_now = units(root)
        check("docstring 里的 skip() 不算命中",
              any(f["kind"] == "无理由的 skip" for f in units_now["script/doc.py"]["findings"]), False)

        # 反证 10：proptest 的 prop_assert_eq! 必须算断言（否则整类属性测试被误判）
        (cdir / "lib.rs").write_text("#[test]\nfn prop() { prop_assert_eq!(1, 1); }\n", encoding="utf-8")
        check("prop_assert 算断言",
              any(f["kind"] == "无断言测试" for f in
                  rust_findings(cdir / "lib.rs", (cdir / "lib.rs").read_text(encoding="utf-8"))[0]), False)

        # 反证 11：#[should_panic] 的测试不需要自己写 assert
        (cdir / "lib.rs").write_text("#[test]\n#[should_panic(expected = \"x\")]\nfn boom() { run(0); }\n",
                                     encoding="utf-8")
        check("should_panic 不算无断言",
              any(f["kind"] == "无断言测试" for f in
                  rust_findings(cdir / "lib.rs", (cdir / "lib.rs").read_text(encoding="utf-8"))[0]), False)

    print(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root, baseline = ROOT, BASELINE
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1])
    if "--baseline" in argv:
        baseline = pathlib.Path(argv[argv.index("--baseline") + 1])
    if "--update" in argv:
        return update_baseline(root, baseline)
    result = scan(root, baseline)
    if "--json" in argv:
        print(json.dumps(result, ensure_ascii=False, indent=1))
    else:
        print(f"单位 {len(result['units'])} 个｜测试 {sum(result['units'].values())} 条｜"
              f"拦 {result['blocking']} 条｜警告 {result['warnings']} 条")
        for finding in result["findings"]:
            print(f"  [{'拦' if finding['blocking'] else '警'}|{finding['kind']}] "
                  f"{finding['file']}:{finding.get('line', '')} {finding['detail']}")
        print("结论：" + ("红" if result["red"] else "绿（每条测试都真的会判东西）"))
    return 1 if result["red"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
