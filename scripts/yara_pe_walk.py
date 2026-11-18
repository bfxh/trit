#!/usr/bin/env python3
"""pe 对象树遍历对拍：官方 `pe` 模块在每个样本上吐出的**整棵对象树**，逐个
叶子生成一条规则，再让官方与 trit-re 都跑一遍。

yara_diff.py 对拍的是"手写规则 × 样本"；这里反过来——规则由官方的输出
派生，所以字段有多少就测多少，不靠人想得到：

  - 每个标量叶子        `pe.<路径> == <官方读数>`
  - 每个字典键          `pe.version_info["<键>"] == "<值>"`
  - 每个数组的越界下标  `not defined pe.<数组>[N].<字段>`（N、N + 1000000、-1）
  - 同形结构里别的实例有、这个实例没有的字段  `not defined pe.<路径>.<字段>`
  - 模块函数            `pe.imports(...)` / `pe.exports(...)` / `pe.section_index(...)` /
                        `pe.rva_to_offset(...)` / `pe.is_dll()` 等，期望值由对象树算出

用法：
    pip install yara-python==4.5.4
    python -I scripts/yara_pe_walk.py --trit <trit-re> --samples <PE 样本目录> [--verbose]
    python -I scripts/yara_pe_walk.py --self-test      # 只测规则生成，不需要 yara-python

序号导入名（`sigscan/expr/ordnames.rs` 那两张表）的逐序号复核：
    python -I scripts/yara_pe_walk.py --write-ordinal-pes <空目录>
    python -I scripts/yara_pe_walk.py --trit <trit-re> --samples <该目录> --cap 20000
造出来的是"从某个 DLL 按序号导入 lo..hi"的合成 PE（每个 8000 个序号，覆盖
1..65535；DLL 名含大小写 / 前缀 / 不带扩展名的对照组）。

逐条分类（退出码：有 DIFF = 1）：
  OK        两边都有裁决且一致
  ABSTAIN   官方有裁决、trit-re 报 UNKNOWN（子集外字段记名弃权——不算失败，
            但每一类都要在 docs/YARA-EXPR.md 登记）
  GAP       trit-re 报 UNSUPPORTED（官方能编译而本实现拒绝——同上）
  DIFF      两边都有裁决但不一致，或 trit-re 没有给出这条规则（真差异——要修）

大数组（导出表几千项）按确定性等距抽样，每个数组最多 `--cap` 项；同一批
样本两次运行生成的规则逐字节相同。
"""

from __future__ import annotations

import argparse
import collections
import re
import subprocess
import sys
import tempfile
from pathlib import Path

# 不遍历的子树：`signatures`（Authenticode，本实现整族记名弃权，另行登记）、
# `module`（yara-python 自己加的模块名）。
SKIP = {"signatures", "module"}
SAMPLE_EXTS = {".exe", ".dll", ".sys", ".pyd", ".bin", ".sample"}


def lit(data: bytes) -> str:
    """字节串 → YARA 字符串字面量（全部写成 \\xNN，不怕引号与非 ASCII）。"""
    return '"' + "".join(f"\\x{byte:02x}" for byte in data) + '"'


def num(value: int) -> str:
    """整数 → YARA 表达式（负数写成减法：字面量没有负号）。"""
    return str(value) if value >= 0 else f"(0 - {-value})"


def shape(path: str) -> str:
    """路径的"形状"：下标抹掉，`pe.sections[3].name` → `pe.sections[].name`。"""
    return re.sub(r"\[[^\]]*\]", "[]", path)


def picks(length: int, cap: int) -> list[int]:
    """数组抽样：不超过 cap 就全取；否则头 cap/2 项 + 尾 4 项 + 中间等距 cap/2 项。"""
    if length <= cap:
        return list(range(length))
    half = cap // 2
    middle = [half + (length - half - 4) * step // half for step in range(half)]
    return sorted(set(range(half)) | set(middle) | set(range(length - 4, length)))


class Walk:
    """一批样本共用的遍历状态：见过的字段集合与叶子形状（跨样本取并集）。"""

    def __init__(self, cap: int = 24) -> None:
        self.cap = cap
        self.fields: dict[str, set[str]] = collections.defaultdict(set)
        self.leaves: set[str] = set()

    def learn(self, node, path: str = "pe") -> None:
        """第一遍：只记字段名与叶子形状（供 `not defined` 类规则用）。"""
        if isinstance(node, dict):
            if path.endswith("version_info"):
                return
            self.fields[shape(path)].update(key for key in node if key not in SKIP)
            for key, value in node.items():
                if key not in SKIP and not is_constant(key):
                    self.learn(value, f"{path}.{key}")
        elif isinstance(node, list):
            for index in picks(len(node), self.cap):
                self.learn(node[index], f"{path}[{index}]")
        elif isinstance(node, (bool, int, bytes)):
            self.leaves.add(shape(path))

    def values(self, node, path: str = "pe") -> list[str]:
        """每个叶子一条 `路径 == 官方读数`，每个数组三条越界规则。"""
        out: list[str] = []
        if isinstance(node, dict):
            if path.endswith("version_info"):
                for key, value in node.items():
                    raw = key.encode("latin-1", "replace") if isinstance(key, str) else key
                    out.append(f"{path}[{lit(raw)}] == {lit(value)}")
                out.append(f'not defined {path}["__no_such_key__"]')
                return out
            for key, value in node.items():
                if key not in SKIP and not is_constant(key):
                    out.extend(self.values(value, f"{path}.{key}"))
        elif isinstance(node, list):
            for index in picks(len(node), self.cap):
                out.extend(self.values(node[index], f"{path}[{index}]"))
            leaf = self.first_leaf(f"{path}[0]")
            if leaf:
                for index in (str(len(node)), str(len(node) + 1000000), "0 - 1"):
                    out.append(f"not defined {path}[{index}].{leaf}")
        elif isinstance(node, bool):
            out.append(f"{path} == {int(node)}")
        elif isinstance(node, int):
            out.append(f"{path} == {num(node)}")
        elif isinstance(node, bytes):
            out.append(f"{path} == {lit(node)}")
        elif not isinstance(node, float):  # 浮点字段：本实现不收浮点，跳过
            raise SystemExit(f"对象树里出现了没见过的类型 {type(node)}：{path}")
        return out

    def first_leaf(self, path: str) -> str | None:
        names = sorted(self.fields.get(shape(path), ()))
        return next((name for name in names if shape(f"{path}.{name}") in self.leaves), None)

    def missing(self, node, path: str = "pe") -> list[str]:
        """同形结构里别的实例有、这个实例没有的标量字段：官方是未定义。"""
        out: list[str] = []
        if isinstance(node, dict) and not path.endswith("version_info"):
            for name in sorted(self.fields.get(shape(path), set()) - set(node)):
                if shape(f"{path}.{name}") in self.leaves:
                    out.append(f"not defined {path}.{name}")
            for key, value in node.items():
                if key not in SKIP:
                    out.extend(self.missing(value, f"{path}.{key}"))
        elif isinstance(node, list):
            for index, value in enumerate(node[:6]):
                out.extend(self.missing(value, f"{path}[{index}]"))
        return out


def is_constant(key: str) -> bool:
    """`pe.MACHINE_AMD64` 之类的模块常量（对象树里也有，值与样本无关）。"""
    return key.isupper() or (key[0].isupper() and "_" in key)


def functions(tree: dict) -> list[str]:
    """模块函数：期望值由对象树算出，写成"应当命中"的条件。"""
    out: list[str] = []
    if "characteristics" in tree:
        out.append(f"pe.is_dll() == {tree['characteristics'] & 0x2000}")
    if "opthdr_magic" in tree:
        wide = tree["opthdr_magic"] == 0x20B
        out += [f"pe.is_64bit() == {int(wide)}", f"pe.is_32bit() == {int(not wide)}"]
    for index, section in enumerate(tree.get("sections", [])[:8]):
        name = section.get("name")
        if name is not None and b"\0" not in name:
            out.append(f"pe.section_index({lit(name)}) <= {index}")
        if "raw_data_offset" in section and section.get("raw_data_size"):
            out.append(f"pe.section_index({section['raw_data_offset']}) <= {index}")
            rva = section.get("virtual_address")
            if rva is not None:
                undefined = f"not defined pe.rva_to_offset({rva})"
                out.append(f"pe.rva_to_offset({rva}) >= 0 or {undefined}")
                out.append(f"pe.rva_to_offset({rva} + 1) == pe.rva_to_offset({rva}) + 1 or {undefined}")
    out += [
        'not defined pe.section_index("\\x01nope")',
        "not defined pe.rva_to_offset(0x7fffffff)",
        "not defined pe.rva_to_offset(0 - 1)",
        "pe.rva_to_offset(0) == 0 or not defined pe.rva_to_offset(0)",
    ]
    if "entry_point_raw" in tree and "entry_point" in tree:
        out.append("pe.rva_to_offset(pe.entry_point_raw) == pe.entry_point")
    for library in tree.get("import_details", [])[:6]:
        dll = library["library_name"]
        if b"\0" in dll:
            continue
        count = library["number_of_functions"]
        out += [
            f"pe.imports({lit(dll)}) == {count}",
            f"pe.imports({lit(dll.upper())}) == {count}",
            f"pe.imports(pe.IMPORT_STANDARD, {lit(dll)}) == {count}",
        ]
        for function in library["functions"][:3]:
            if "name" in function:
                name = function["name"]
                out += [
                    f"pe.imports({lit(dll)}, {lit(name)})",
                    f"pe.imports({lit(dll.lower())}, {lit(name.swapcase())})",
                    f"pe.imports(pe.IMPORT_STANDARD, {lit(dll)}, {lit(name)})",
                    f"not pe.imports({lit(dll)}, {lit(name + b'_x')})",
                    f"pe.import_rva({lit(dll)}, {lit(name)}) > 0",
                ]
            if "ordinal" in function:
                out.append(f"pe.imports({lit(dll)}, {function['ordinal']})")
    out += ['pe.imports("no_such.dll") == 0', 'not pe.imports("no_such.dll", "f")']
    out.append('not defined pe.import_rva("no_such.dll", "f")')
    exports = tree.get("export_details", [])
    for index in sorted(set(range(min(4, len(exports)))) | ({len(exports) - 1} if exports else set())):
        export = exports[index]
        if "name" in export:
            name = export["name"]
            out += [
                f"pe.exports({lit(name)})",
                f"pe.exports_index({lit(name)}) <= {index}",
                f"pe.exports({lit(name.swapcase())})",
            ]
        if "ordinal" in export:
            out += [f"pe.exports({export['ordinal']})", f"pe.exports_index({export['ordinal']}) <= {index}"]
    out += [
        'not pe.exports("no_such_export_xyz")',
        'not defined pe.exports_index("no_such_export_xyz")',
        "not pe.exports(0)",
        "not pe.exports(1000000)",
        "for all i in (0..pe.number_of_sections - 1) : (defined pe.sections[i].name)",
        "not defined pe.sections[pe.number_of_sections].name",
        "for all s in pe.sections : (s.virtual_address >= 0)",
        "for all d in pe.data_directories : (d.size >= 0)",
        "for all imp in pe.import_details : (imp.number_of_functions >= 0"
        " and for all f in imp.functions : (defined f.rva)) or pe.number_of_imports == 0",
        "for all k, v in pe.version_info : (pe.version_info[k] == v) or pe.number_of_version_infos == 0",
        "for all r in pe.resources : (r.length >= 0) or pe.number_of_resources == 0",
        "for all e in pe.export_details : (e.ordinal > 0) or pe.number_of_exports == 0",
    ]
    return out


ORDINAL_DLLS = ["WS2_32.dll", "wsock32.DLL", "OLEAUT32.dll", "oleaut32.dll.mui", "ws2_32", "KERNEL32.dll"]


def ordinal_pe(dll: bytes, ordinals: list[int]) -> bytes:
    """最小 PE32+：一个节，里面只有一张"从 dll 按序号导入"的导入表。"""
    import struct

    count = len(ordinals)
    rva, raw = 0x1000, 0x200
    name_at = 40  # 两个导入描述符（第二个全零收尾）之后
    ilt_at = (name_at + len(dll) + 1 + 7) & ~7
    iat_at = ilt_at + 8 * (count + 1)
    size = iat_at + 8 * (count + 1)
    section = bytearray(size)
    struct.pack_into("<IIIII", section, 0, rva + ilt_at, 0, 0, rva + name_at, rva + iat_at)
    section[name_at : name_at + len(dll)] = dll
    for index, ordinal in enumerate(ordinals):
        thunk = (1 << 63) | ordinal
        struct.pack_into("<Q", section, ilt_at + 8 * index, thunk)
        struct.pack_into("<Q", section, iat_at + 8 * index, thunk)
    raw_size = (size + 0x1FF) & ~0x1FF
    virtual_size = (size + 0xFFF) & ~0xFFF
    header = bytearray(0x200)
    header[0:2] = b"MZ"
    struct.pack_into("<I", header, 0x3C, 0x40)
    header[0x40:0x44] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", header, 0x44, 0x8664, 1, 0, 0, 0, 0xF0, 0x2022)
    opt = 0x58
    struct.pack_into("<HBBIIIII", header, opt, 0x20B, 14, 0, 0, raw_size, 0, 0, 0x1000)
    struct.pack_into("<QII", header, opt + 24, 0x180000000, 0x1000, 0x200)
    struct.pack_into("<HHHHHHIIIIHH", header, opt + 40, 6, 0, 0, 0, 6, 0, 0, 0x1000 + virtual_size, 0x200, 0, 2, 0x160)
    struct.pack_into("<QQQQII", header, opt + 72, 0x100000, 0x1000, 0x100000, 0x1000, 0, 16)
    struct.pack_into("<II", header, opt + 112 + 8, rva, 40)  # 数据目录 1 = 导入表
    table = opt + 0xF0
    header[table : table + 6] = b".rdata"
    struct.pack_into("<IIIIIIHHI", header, table + 8, size, rva, raw_size, raw, 0, 0, 0, 0, 0x40000040)
    return bytes(header) + bytes(section) + bytes(raw_size - size)


def write_ordinal_pes(root: Path) -> int:
    root.mkdir(parents=True, exist_ok=True)
    written = 0
    for dll in ORDINAL_DLLS:
        for low in range(1, 65536, 8000):
            high = min(low + 7999, 65535)
            stem = dll.replace(".", "_")
            path = root / f"ord_{stem}_{low:05}_{high:05}.dll"
            path.write_bytes(ordinal_pe(dll.encode("ascii"), list(range(low, high + 1))))
            written += 1
    print(f"写出 {written} 个合成 PE → {root}")
    return 0


def official_tree(sample: Path) -> dict:
    import yara

    tree: dict = {}

    def grab(data):
        tree.update(data)
        return yara.CALLBACK_CONTINUE

    yara.compile(source='import "pe" rule x { condition: true }').match(str(sample), modules_callback=grab)
    return tree


def run_official(source: str, sample: Path) -> set[str]:
    import yara

    return {match.rule for match in yara.compile(source=source).match(str(sample))}


def run_trit(cli: Path, source: str, sample: Path) -> tuple[dict[str, str], str]:
    with tempfile.TemporaryDirectory() as scratch:
        rules = Path(scratch) / "walk.yar"
        rules.write_text(source, encoding="utf-8")
        proc = subprocess.run(
            [str(cli), "sigscan", "--rules", str(rules), str(sample)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    verdicts = {}
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2:
            verdicts[parts[1]] = parts[0]
    return verdicts, proc.stderr.strip()[-200:]


def family(condition: str) -> str:
    """条件问的是哪个顶层字段 / 函数（汇总弃权分布用）：`pe.<第一段>[.<第二段>]`。"""
    found = re.search(r"pe\.([a-z_0-9]+)(\.[a-z_]+)?", condition)
    if not found:
        return condition[:40]
    head = found.group(1)
    return f"pe.{head}{found.group(2)}" if head == "rich_signature" and found.group(2) else f"pe.{head}"


def classify(official_hit: bool, trit: str | None) -> str:
    if trit == "UNKNOWN":
        return "ABSTAIN"
    if trit == "UNSUPPORTED":
        return "GAP"
    if trit in ("HIT", "MISS") and (trit == "HIT") == official_hit:
        return "OK"
    return "DIFF"


def iter_samples(root: Path):
    if root.is_file():
        yield root
        return
    for path in sorted(root.rglob("*")):
        if path.is_file() and path.suffix.lower() in SAMPLE_EXTS:
            yield path


def self_test() -> int:
    failures, checks = [], []

    def check(name, got, want):
        checks.append(name)
        if got != want:
            failures.append(f"{name}: {got!r} != {want!r}")

    check("字面量全转义", lit(b'a"\xff'), '"\\x61\\x22\\xff"')
    check("负数写成减法", (num(5), num(-3)), ("5", "(0 - 3)"))
    check("形状抹下标", shape('pe.a[12].b["k"].c'), "pe.a[].b[].c")
    check("小数组全取", picks(5, 24), [0, 1, 2, 3, 4])
    big = picks(1000, 24)
    check("大数组封顶", len(big) <= 24 + 4, True)
    check("大数组含首尾", (big[0], big[-1]), (0, 999))
    check("抽样确定", picks(1000, 24), big)
    check("常量名识别", [is_constant(k) for k in ("DLL", "MACHINE_AMD64", "machine", "Rich")], [True, True, False, False])

    one = {"machine": 0x8664, "is_pe": True, "DLL": 0x2000, "module": "pe",
           "sections": [{"name": b".text", "raw_data_size": 512}, {"name": b".rsrc"}],
           "version_info": {"K": b"v"}, "signatures": [{"issuer": b"x"}], "dll_name": b"a.dll"}
    two = {"machine": 0x14C, "is_pe": True, "sections": [], "version_info": {}}
    walk = Walk()
    walk.learn(one)
    walk.learn(two)
    got = walk.values(one)
    check("标量叶子", "pe.machine == 34404" in got and "pe.is_pe == 1" in got, True)
    check("字符串叶子", f"pe.sections[0].name == {lit(b'.text')}" in got, True)
    check("字典键", f"pe.version_info[{lit(b'K')}] == {lit(b'v')}" in got, True)
    check("字典缺键", 'not defined pe.version_info["__no_such_key__"]' in got, True)
    check("越界三条", [c for c in got if c.startswith("not defined pe.sections[")],
          ["not defined pe.sections[2].name", "not defined pe.sections[1000002].name",
           "not defined pe.sections[0 - 1].name"])
    check("常量与跳过子树不出规则", any("DLL" in c or "signatures" in c or "module" in c for c in got), False)
    check("缺字段", walk.missing(one), ["not defined pe.sections[1].raw_data_size"])
    check("另一实例缺的顶层字段", walk.missing(two), ["not defined pe.dll_name"])
    check("空数组也出越界规则", "not defined pe.sections[0].name" in walk.values(two), True)
    calls = functions({"characteristics": 0x2022, "opthdr_magic": 0x20B,
                       "import_details": [{"library_name": b"K.dll", "number_of_functions": 1,
                                           "functions": [{"name": b"Fn", "rva": 8}]}],
                       "export_details": [{"name": b"E", "ordinal": 1}]})
    check("is_dll 是掩码值", "pe.is_dll() == 8192" in calls, True)
    check("导入名大小写互换", f"pe.imports({lit(b'k.dll')}, {lit(b'fN')})" in calls, True)
    check("导出序号", "pe.exports(1)" in calls and "pe.exports_index(1) <= 0" in calls, True)
    check("分类", [classify(True, "HIT"), classify(False, "MISS"), classify(True, "MISS"),
                 classify(False, "HIT"), classify(True, "UNKNOWN"), classify(False, "UNSUPPORTED"),
                 classify(True, None)], ["OK", "OK", "DIFF", "DIFF", "ABSTAIN", "GAP", "DIFF"])
    check("字段族", [family(c) for c in ("not defined pe.delayed_import_details[3].library_name",
                                       "pe.rich_signature.version_data == 1", "pe.is_signed == 0",
                                       'pe.imports("a", "b")', "for all s in pe.sections : (true)")],
          ["pe.delayed_import_details", "pe.rich_signature.version_data", "pe.is_signed", "pe.imports",
           "pe.sections"])
    built = ordinal_pe(b"WS2_32.dll", [1, 115, 500])
    check("合成 PE 头", (built[:2], built[0x40:0x44], len(built) % 0x200), (b"MZ", b"PE\0\0", 0))
    check("合成 PE 的序号 thunk", built.count((1 << 63 | 115).to_bytes(8, "little")), 2)
    check("合成 PE 的 DLL 名", built[0x200 + 40 : 0x200 + 51], b"WS2_32.dll\0")
    for failure in failures:
        print(f"SELF-TEST FAIL {failure}")
    print(f"self-test：{len(checks) - len(failures)}/{len(checks)} 通过")
    return 1 if failures else 0


def main() -> int:
    if "--self-test" in sys.argv[1:]:
        return self_test()
    if "--write-ordinal-pes" in sys.argv[1:]:
        return write_ordinal_pes(Path(sys.argv[sys.argv.index("--write-ordinal-pes") + 1]))
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--trit", required=True, type=Path, help="trit-re 可执行文件路径")
    parser.add_argument("--samples", required=True, type=Path, help="PE 样本文件或目录（递归）")
    parser.add_argument("--cap", type=int, default=24, help="每个数组最多遍历多少项（默认 24）")
    parser.add_argument("--verbose", action="store_true", help="逐类列出 ABSTAIN / GAP 的前两条")
    args = parser.parse_args()
    try:
        import yara
    except ImportError:
        print("需要 yara-python：pip install yara-python==4.5.4", file=sys.stderr)
        return 2
    samples = list(iter_samples(args.samples))
    if not samples:
        print("没有样本可扫", file=sys.stderr)
        return 2
    walk = Walk(args.cap)
    trees = {sample: official_tree(sample) for sample in samples}
    for tree in trees.values():
        walk.learn(tree)
    total: collections.Counter = collections.Counter()
    families: collections.Counter = collections.Counter()
    for sample, tree in trees.items():
        conditions = walk.values(tree) + walk.missing(tree) + functions(tree)
        source = 'import "pe"\n' + "\n".join(
            f"rule r{index} {{ condition: {condition} }}" for index, condition in enumerate(conditions)
        )
        hits = run_official(source, sample)
        verdicts, stderr = run_trit(args.trit, source, sample)
        tally: collections.Counter = collections.Counter()
        shown: collections.Counter = collections.Counter()
        for index, condition in enumerate(conditions):
            name = f"r{index}"
            kind = classify(name in hits, verdicts.get(name))
            tally[kind] += 1
            if name not in hits:
                tally["官方不命中"] += 1  # 生成的条件按理都该命中；不命中的单列出来看
            key = (kind, shape(condition.split(" == ")[0])[:60])
            shown[key] += 1
            if kind != "OK":
                families[(kind, family(condition))] += 1
            if kind == "DIFF" or (args.verbose and kind != "OK" and shown[key] <= 2):
                got = verdicts.get(name, f"（无输出：{stderr}）")
                side = "HIT" if name in hits else "miss"
                print(f"  {kind} {sample.name}: official={side} trit={got} | {condition[:160]}")
        total += tally
        print(f"{sample.name}: " + " ".join(f"{kind}={count}" for kind, count in sorted(tally.items())))
    print(
        f"\n遍历汇总（yara-python {yara.__version__} / YARA {yara.YARA_VERSION}）：样本 {len(samples)}  "
        + "  ".join(f"{kind} {total[kind]}" for kind in ("OK", "ABSTAIN", "GAP", "DIFF", "官方不命中"))
    )
    for (kind, name), count in sorted(families.items(), key=lambda item: (-item[1], item[0])):
        print(f"  {kind:8} {count:6}  {name}")
    return 1 if total["DIFF"] else 0


if __name__ == "__main__":
    sys.exit(main())
