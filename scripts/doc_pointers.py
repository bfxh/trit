#!/usr/bin/env python3
"""文档行号指针对账门：`` `文件.rs:91` `` 这类引用必须**现在**就指得到、指得对。

动机（反"我以为做完了"的另一种形态）：仓里大量用 `文件.rs:行` 当复算指针（判据、读数出处、
"这行改成什么"）。代码一动，行号就漂；漂了之后那句话仍然是**通顺的**，没人会发现。
本门逐条按行号去读文件：

1. 路径解析：带 `/` 的按仓根相对路径；只给文件名的在全仓唯一匹配（多命中 ⇒ 拦"歧义"，
   零命中 ⇒ 拦"指不到"）。
2. 行号范围：`1 ≤ 行号 ≤ 该文件行数`，越界就报出真实行数。
3. 符号核对：指针同一句里如果还反引号了一个标识符（如 `` `decompose` ``），
   要求该行的 ±2 行内真的出现它 —— 这是"行号还在、内容已经不是那件事"的唯一可机器判形式。

读不到清单/扫不到任何指针 ⇒ 判红（失明不等于对上了）。

用法：
  python scripts/doc_pointers.py            # 对账
  python scripts/doc_pointers.py --show     # 打印解析结果（哪条指到哪个文件的哪一行）
  python scripts/doc_pointers.py --json
  python scripts/doc_pointers.py --self-test
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = pathlib.Path(__file__).resolve().parent / "doc_pointers_baseline.json"
POINTER = re.compile(r"`([\w./\-]+\.(?:rs|py|toml|json|md|sh|wgsl)|[\w\-]+\.(?:rs|py|toml|json)):(\d+)`")
# 同句里的标识符：够长、不含路径分隔、看起来像代码符号
TOKEN = re.compile(r"`([A-Za-z_][A-Za-z0-9_]{4,})`")
# 这些词太泛，拿它们核对行号只会误报
STOPWORDS = {"None", "Some", "true", "false", "pub", "fn", "let", "self", "crate", "struct", "use", "mod"}

SKIP_DIRS = {"target", ".git", "node_modules", "dist", "build"}


def scanned(root: pathlib.Path) -> list[pathlib.Path]:
    files = [root / "README.md", root / "CHANGELOG.md"]
    files += sorted((root / "docs").glob("*.md"))
    return [f for f in files if f.exists()]


def index_names(root: pathlib.Path) -> dict[str, list[pathlib.Path]]:
    """文件名 → 全仓该名字的文件（用于解析只写文件名的指针）。"""
    out: dict[str, list[pathlib.Path]] = {}
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if path.suffix not in {".rs", ".py", ".toml", ".json", ".md", ".sh", ".wgsl"}:
            continue
        out.setdefault(path.name, []).append(path)
    return out


def resolve(raw: str, root: pathlib.Path, by_name: dict[str, list[pathlib.Path]]) -> list[pathlib.Path]:
    if "/" in raw:
        direct = root / raw
        return [direct] if direct.exists() else []
    return by_name.get(raw, [])


def sentence_of(text: str, at: int) -> str:
    """指针所在的**子句**（按 。；，、与换行切）。

    只用子句是有原因的：整句会把同一句话里别处的标识符也算进核对范围，
    于是"`Cargo.toml:9` 是 `publish = false`……`MatNet`……"会因为远处的 `MatNet` 误报指歪。
    """
    seps = ("。", "；", "，", "、", "\n", ";", ",")
    start = 0
    for sep in seps:
        start = max(start, text.rfind(sep, 0, at) + 1)
    end = len(text)
    for sep in seps:
        i = text.find(sep, at)
        if i != -1:
            end = min(end, i + 1)
    return text[start:end]


def nearby_tokens(text: str, m: re.Match) -> set[str]:
    """说明"这行写的是什么"的标识符：指针**后面 40 字符**内的，加上紧贴指针前面的 12 字符。

    不扫整句也不扫前 40 字符：仓里的写法是 "`文件.rs:行` 的 `符号`"、"`文件.rs:行` 是 `值`"，
    说明跟在指针后面；把整句都算上就会被同句别处的名词误伤（实测被 `MatNet` 误报过一次）。
    """
    after = text[m.end() : m.end() + 40]
    before = text[max(0, m.start() - 12) : m.start()]
    found = {t for t in TOKEN.findall(after) if t not in STOPWORDS}
    found |= {t for t in TOKEN.findall(before) if t not in STOPWORDS}
    return found


def violations(text: str, root: pathlib.Path, by_name: dict[str, list[pathlib.Path]], doc: pathlib.Path) -> list[str]:
    problems: list[str] = []
    for m in POINTER.finditer(text):
        raw, line = m.group(1), int(m.group(2))
        hits = resolve(raw, root, by_name)
        where = f"{doc.name} 的 `{raw}:{line}`"
        if not hits:
            problems.append(f"指不到：{where} —— 全仓没有这个文件（改名/删文件后指针没跟着改）")
            continue
        if len(hits) > 1:
            listing = "、".join(p.relative_to(root).as_posix() for p in hits[:4])
            problems.append(f"歧义：{where} —— 同名文件有 {len(hits)} 个（{listing}），请写成带目录的路径")
            continue
        target = hits[0]
        lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
        if not 1 <= line <= len(lines):
            problems.append(
                f"越界：{where} —— {target.relative_to(root).as_posix()} 只有 {len(lines)} 行"
            )
            continue
        # 指针附近的标识符核对（只看紧邻的，避免一句话里别处的符号误伤）
        tokens = nearby_tokens(text, m)
        window = "\n".join(lines[max(0, line - 3) : line + 2])
        for token in sorted(tokens):
            if token not in window:
                problems.append(
                    f"指歪：{where} —— 指针旁边还写着 `{token}`，但 "
                    f"{target.relative_to(root).as_posix()} 的第 {line} 行上下 2 行里没有它（行号漂了）"
                )
    return problems


def show(text: str, root: pathlib.Path, by_name: dict[str, list[pathlib.Path]], doc: pathlib.Path) -> list[str]:
    out: list[str] = []
    for m in POINTER.finditer(text):
        raw, line = m.group(1), int(m.group(2))
        hits = resolve(raw, root, by_name)
        if len(hits) != 1:
            out.append(f"{doc.name}: `{raw}:{line}` -> 解析到 {len(hits)} 个")
            continue
        lines = hits[0].read_text(encoding="utf-8", errors="replace").splitlines()
        shown = lines[line - 1].strip()[:60] if 1 <= line <= len(lines) else "(越界)"
        out.append(f"{doc.name}: `{raw}:{line}` -> {hits[0].relative_to(root).as_posix()} | {shown}")
    return out


def compare(found: list[str], baseline: dict[str, int]) -> list[str]:
    """棘轮：按文档计数，只许变少；新出现的文档必须从 0 开始。"""
    counts: dict[str, int] = {}
    for item in found:
        doc = item.split("：", 1)[-1].split(" 的 ", 1)[0]
        counts[doc] = counts.get(doc, 0) + 1
    problems = []
    for doc, n in sorted(counts.items()):
        allowed = baseline.get(doc, 0)
        if n > allowed:
            problems.append(
                f"{doc} 有 {n} 条坏指针，基线允许 {allowed} —— 把指针改对，或（仅存量）跑 --update 收紧"
            )
    return problems


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--show", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    files = scanned(ROOT)
    if not files:
        print("FAIL 找不到任何被扫文档（失明不等于对上了）")
        return 1
    by_name = index_names(ROOT)
    found: list[str] = []
    total = 0
    for doc in files:
        text = doc.read_text(encoding="utf-8", errors="replace")
        total += len(POINTER.findall(text))
        if args.show:
            for line in show(text, ROOT, by_name, doc):
                print(line)
            continue
        found += violations(text, ROOT, by_name, doc)
    if args.show:
        return 0
    baseline = (
        json.loads(BASELINE.read_text(encoding="utf-8")) if BASELINE.exists() else {}
    )
    if args.update:
        counts: dict[str, int] = {}
        for item in found:
            doc = item.split("：", 1)[-1].split(" 的 ", 1)[0]
            counts[doc] = counts.get(doc, 0) + 1
        grew = [f"{d} {counts[d]}>{baseline.get(d, 0)}" for d in counts if counts[d] > baseline.get(d, 0)]
        if grew and baseline:
            print("REFUSED 坏指针变多了，--update 不接受：" + "；".join(grew))
            return 1
        BASELINE.write_text(json.dumps(counts, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"台账已写：{len(counts)} 个文档共 {sum(counts.values())} 条坏指针 → doc_pointers_baseline.json")
        return 0
    print(f"文档指针对账：扫 {len(files)} 个文档，共 {total} 条带行号的指针")
    if not total:
        print("FAIL 一条指针都没抓到 ⇒ 扫描口径漂了（不是「文档变干净了」）")
        return 1
    hard = found + compare(found, baseline) if baseline else found
    for item in found:
        print("  拦：" + item)
    if args.json:
        print(json.dumps({"pointers": total, "problems": hard}, ensure_ascii=False))
    print(("FAIL 拦 %d 条" % len(hard)) if hard else "OK 每条行号指针都指得到、指得对")
    return 1 if hard else 0


def self_test() -> int:
    import tempfile

    root = pathlib.Path(tempfile.mkdtemp(prefix="doc-pointers-"))
    (root / "src").mkdir(parents=True)
    (root / "docs").mkdir()
    (root / "src" / "thing.rs").write_text(
        "fn alpha() {}\nfn decompose() {}\nfn gamma() {}\nfn delta() {}\nfn epsilon() {}\n", encoding="utf-8"
    )
    (root / "src" / "dup.rs").write_text("fn x() {}\n", encoding="utf-8")
    (root / "other").mkdir()
    (root / "other" / "dup.rs").write_text("fn y() {}\n", encoding="utf-8")
    doc = root / "docs" / "D.md"
    by_name = index_names(root)
    ok = True

    def expect(label: str, bad: list[str], want: str | None) -> None:
        nonlocal ok
        good = (not bad) if want is None else any(want in b for b in bad)
        print(("ok   " if good else "FAIL ") + label)
        ok = ok and good

    good = "第 2 行是 `src/thing.rs:2` 里的 `decompose`，这条应该通过。\n"
    doc.write_text(good, encoding="utf-8")
    expect("正例：路径 + 行号 + 同句标识符都对", violations(good, root, by_name, doc), None)

    off = "同一句引用 `src/thing.rs:99` 的 `decompose`。\n"
    doc.write_text(off, encoding="utf-8")
    expect("反证 1：行号越界", violations(off, root, by_name, doc), "越界")

    gone = "引用 `src/missing.rs:3` 里的 `decompose`。\n"
    doc.write_text(gone, encoding="utf-8")
    expect("反证 2：文件指不到", violations(gone, root, by_name, doc), "指不到")

    amb = "引用 `dup.rs:1` 里的 `decompose`。\n"
    doc.write_text(amb, encoding="utf-8")
    expect("反证 3：同名文件歧义", violations(amb, root, by_name, doc), "歧义")

    drifted = "引用 `src/thing.rs:5` 里的 `decompose`。\n"
    doc.write_text(drifted, encoding="utf-8")
    expect(
        "反证 4：行号漂了（同句标识符不在 ±2 行内）", violations(drifted, root, by_name, doc), "指歪"
    )

    no_signal = "引用 `src/thing.rs:5`（同句没有标识符可核）。\n"
    doc.write_text(no_signal, encoding="utf-8")
    expect("反证 5：没有可核对的符号时不编造判据", violations(no_signal, root, by_name, doc), None)

    far = "引用 `src/thing.rs:5` 是 `epsilon`；这一句后面还顺便提到别的事情与背景说明，例如 `decompose`。\n"
    doc.write_text(far, encoding="utf-8")
    expect(
        "反证 6：标识符离得远就不参与核对（按整句核会误伤）", violations(far, root, by_name, doc), None
    )

    ratchet = ["指歪：docs/D.md 的 `a.rs:1` x", "指歪：docs/D.md 的 `a.rs:2` y"]
    expect(
        "反证 7：坏指针变多时棘轮必须拦（基线 1，实测 2）",
        compare(ratchet, {"docs/D.md": 1}),
        "基线允许 1",
    )
    expect(
        "反证 7b：变少是允许的（基线 2，实测 1）",
        compare(["指歪：docs/D.md 的 `a.rs:1` x"], {"docs/D.md": 2}),
        None,
    )

    print("自测结论：" + ("绿" if ok else "红"))
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
