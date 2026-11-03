#!/usr/bin/env python3
"""磁盘介质门：中型及以上项目的**构建/测试落盘禁止放在机械盘**（默认开，跑测试前就该判）。

**为什么默认开**（本机实测，2026-11-01）：
    盘 0  WDC WD40EZRZ-00GXCB0   HDD  SATA   3726 GB  ⇒ 挂 F:
    盘 1  XPG GAMMIX S50 PRO     SSD  NVMe    932 GB  ⇒ 挂 C: D: E:
  同一台机器上"哪个盘能跑构建"是**盘符级别**的差异；把 `target/` 或 TMP 指到 F: 时，
  编译与测试的墙钟会被随机写放大拖垮（本仓同族的既有事故：pagefile 落在机械盘直接导致整机卡顿）。
  这不是玄学，是一条能自动判的硬约束 ⇒ 进门禁，不靠人记。

**规模分类口径**（阈值锚真实读数，不是拍脑袋）：
    small  : 源行 < 5,000           —— 只提醒（单 crate 小工具，机械盘也痛不到哪去）
    medium : 源行 < 40,000          —— 判红（例：中等多 crate 工作区）
    large  : 源行 >= 40,000         —— 判红（实测对照：trit main 121,680 行 / 452 个 .rs；
                                      BSHSQ 113,012 行 / 583 个 .rs ⇒ 两边都是 large）
    源行 = 全仓 .rs 行数（跳 target/.git/__pycache__）

**检查哪些路径**（每一个都必须落在非旋转介质）：
    1. 仓库根 —— cargo 默认把 `target/` 写在这里；
    2. `CARGO_TARGET_DIR`（若设置）—— 构建产物实际落盘点；
    3. `TMPDIR` / `TMP` / `TEMP` —— 测试与工具建临时文件的地方；
    4. `--path <p>` 追加项（自己按项目情况加）。

**判红**：`ROTATIONAL`（落在机械盘）；`UNKNOWN` 且规模是 medium/large —— **按最严处理**
（测不出介质就当它可能是机械盘；确实测不出的环境用 `--allow-rotational <路径>` 显式放行，
并把这条放行写进 PR 说明，别让它悄悄长期生效）。

**已知边界（实测，别当万能）**：
    · 原生 Windows / 真物理盘：可信。本机两路都验过 —— `D:` → ssd(XPG GAMMIX S50 PRO)、
      `F:` → hdd(WDC WD40EZRZ) 且判红；
    · 虚拟化层会把固态报成机械盘：WSL2 自身根盘 `/dev/sdd` 实测 `rotational=1`，
      而它的 VHDX 就放在那块 NVMe 上 ⇒ 在 WSL 里跑本门会**假红**（偏保守，不是偏宽松），
      确实无解就 `--allow-rotational <路径>` 并在 PR 里写明；
    · CI runner 尚未取得证据 ⇒ 工作流里这一步先 `continue-on-error`（观测轮），
      等一次真实读数回来再改成阻塞。

用法：
    python scripts/disk_guard.py                      # 真检查
    python scripts/disk_guard.py --path E:/some/scratch
    python scripts/disk_guard.py --self-test          # 正例 + 逐条反证
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SMALL_LINES = 5_000
MEDIUM_LINES = 40_000
EXCLUDE_DIRS = {"target", ".git", "__pycache__", "node_modules", ".venv", "venv"}


def source_lines(root: Path) -> tuple[int, int]:
    """`(源行数, .rs 文件数)` —— 规模分类的唯一输入。"""
    total = 0
    count = 0
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in EXCLUDE_DIRS]
        for name in filenames:
            if name.endswith(".rs"):
                count += 1
                text = (Path(dirpath) / name).read_text(encoding="utf-8", errors="replace")
                total += len(text.splitlines())
    return total, count


def classify(lines: int) -> str:
    if lines < SMALL_LINES:
        return "small"
    if lines < MEDIUM_LINES:
        return "medium"
    return "large"


# --------------------------------------------------------------------------
# 介质探测：Windows 走 Get-Disk 的 MediaType，Linux 走 /sys/block/*/queue/rotational。
# 平台不支持 / 探不到 ⇒ "unknown"，由调用方按规模决定放行还是判红（**不静当 SSD**）。
# --------------------------------------------------------------------------
def read_evidence(mt: str, spinning: str, physical: str) -> tuple[str, str]:
    """把 Windows 三个信号合成结论 —— **纯函数，单独钉一把锁**。

    本机实况（2026-11-01 实测）：SATA 机械盘在 `Get-Disk.MediaType` 上是**空字符串**
    （只有 `Get-PhysicalDisk.MediaType` 报 HDD），而空集合走 `all()` 会**真空为真** ⇒
    只信 MediaType 的写法会把机械盘判成固态（**假绿**，这条就是那次踩坑）。
    合成规则：任一路说"机械/旋转"就是机械；必须有**正面**的固态证据才判固态；否则 unknown
    （unknown 由调用方按规模决定，绝不默认放行）。
    """
    evidence = f"MediaType={mt or '<空>'} Spinning={spinning or '<空>'} PhysicalDisk={physical or '<空>'}"
    spin = spinning.strip().lower()
    if "hdd" in physical.lower():
        return "hdd", evidence
    if spin == "true":
        return "hdd", evidence
    if "ssd" in physical.lower() or mt.strip().lower() == "ssd":
        return "ssd", evidence
    if mt.strip().lower() == "hdd":
        return "hdd", evidence
    return "unknown", evidence


def _media_windows(path: Path) -> tuple[str, str]:
    drive = path.drive[:1]
    if not drive:
        return "unknown", "取不到盘符"
    cmd = (
        "$ErrorActionPreference='SilentlyContinue';"
        f"$p = Get-Partition -DriveLetter '{drive}';"
        "$d = $p | Get-Disk;"
        "$n = if ($d) { $d.FriendlyName } else { '' };"
        "$pd = Get-PhysicalDisk | Where-Object { $_.FriendlyName -eq $n };"
        "$mt = if ($d) { ($d.MediaType | Sort-Object -Unique) -join '/' } else { '' };"
        "$sp = if ($null -ne $d -and $d.PSObject.Properties.Name -contains 'Spinning') {"
        "($d.Spinning | Sort-Object -Unique) -join '/' } else { '' };"
        "$pm = if ($pd) { ($pd.MediaType | Sort-Object -Unique) -join '/' } else { '' };"
        '"mt=$mt|sp=$sp|pm=$pm|n=$n"'
    )
    try:
        out = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command", cmd],
            capture_output=True,
            text=True,
            timeout=90,
        ).stdout.strip()
    except Exception as exc:  # noqa: BLE001 - 探测失败必须降级成 unknown，不能崩门
        return "unknown", f"powershell 调用失败：{exc}"
    if not out:
        return read_evidence("", "", "")
    # 按 `key=value` 取字段（不是按制表符切位：Windows 版本/属性缺失会让字段数变化，切位会串）
    fields = dict(part.split("=", 1) for part in out.strip().split("|") if "=" in part)
    media, evidence = read_evidence(fields.get("mt", ""), fields.get("sp", ""), fields.get("pm", ""))
    return media, f"{evidence} 盘={fields.get('n') or '?'}"


def _rotational_of(disk: str) -> str | None:
    """`/sys/block/<disk>/queue/rotational` —— 1=机械、0=固态、None=问不到。"""
    rot = Path(f"/sys/block/{disk}/queue/rotational")
    if not rot.exists():
        return None
    try:
        return rot.read_text().strip()
    except OSError:
        return None


def _disk_via_df(path: Path) -> str | None:
    """第二路：`df --output=source` 拿到挂载源，再折到父盘名（`/dev/sda1` -> `sda`）。

    WSL 的 `/mnt/e` 这类网络/透传挂载在这里也会失败（源是 `overlay`/`drvfs`），
    那就如实返回 None ⇒ 上层给 unknown ⇒ **按最严判红**，不猜成固态。
    """
    try:
        out = subprocess.run(
            ["df", "--output=source", "-P", str(path)],
            capture_output=True,
            text=True,
            timeout=20,
        ).stdout.strip().splitlines()
    except Exception:  # noqa: BLE001 - 探测失败降级，不崩门
        return None
    if len(out) < 2:
        return None
    source = out[-1].strip()
    if not source.startswith("/dev/"):
        return None
    name = source[len("/dev/") :]
    if _rotational_of(name) is not None:
        return name
    stripped = name.rstrip("0123456789")
    return stripped or None


def _media_linux(path: Path) -> tuple[str, str]:
    node = None
    try:
        st = path.stat()
        link = Path(f"/sys/dev/block/{os.major(st.st_dev)}:{os.minor(st.st_dev)}")
        if link.exists():
            node = link.resolve()
    except OSError:
        node = None
    disk = None
    if node is not None:
        disk = node.name
        # 分区（sda2 / nvme0n1p1）要回到父盘才问得到旋转性；dm-0/loop 没有 partition 文件，用本身。
        if (node / "partition").exists():
            disk = node.parent.name or disk.rstrip("0123456789")
        val = _rotational_of(disk)
    else:
        val = None
    if val is None:
        disk = _disk_via_df(path)
        val = _rotational_of(disk) if disk else None
    if val is None:
        return "unknown", f"/sys/dev/block 与 df 都问不到旋转性（{node or disk or '?'}）"
    return ("hdd", disk) if val == "1" else ("ssd", disk)


def media_of(path: Path) -> tuple[str, str]:
    """`(介质, 依据)`：介质 ∈ ssd / hdd / unknown。解析软链与 junction ⇒ 量真实落盘点。"""
    real = Path(os.path.realpath(str(path)))
    # 路径还没建（首次构建的 target/、新 TMP）⇒ 退到**最近的已存在祖先**，量同一块盘。
    while str(real) not in ("", ".") and not real.exists():
        real = real.parent
    if os.name == "nt":
        return _media_windows(real)
    if sys.platform.startswith("linux"):
        return _media_linux(real)
    return "unknown", f"平台 {sys.platform} 无介质探测"


def default_paths(root: Path) -> list[tuple[str, Path]]:
    """门禁默认要查的三项；缺项自动跳过（但仓库根一定在）。"""
    out: list[tuple[str, Path]] = [("repo", root)]
    for env in ("CARGO_TARGET_DIR", "TMPDIR", "TMP", "TEMP"):
        val = os.environ.get(env)
        if val:
            out.append((env, Path(val)))
    return out


def check(paths: list[tuple[str, Path]], size: str, allow: list[str]) -> tuple[list[str], list[str]]:
    """返回 `(判红, 提醒)`。`allow` = 显式放行的路径前缀（大小写/分隔符归一后比较）。"""
    failures: list[str] = []
    notes: list[str] = []

    def allowed(raw: str) -> bool:
        norm = os.path.normcase(os.path.normpath(raw))
        return any(norm == os.path.normcase(os.path.normpath(a)) for a in allow)

    for label, path in paths:
        media, detail = media_of(path)
        shown = str(path)
        if media == "hdd":
            if allowed(shown):
                notes.append(f"ALLOWED {label}={shown} 在机械盘（{detail}）—— 显式放行，PR 里要写")
            else:
                failures.append(f"ROTATIONAL: {label}={shown} 落在机械盘（{detail}）⇒ 挪到固态")
        elif media == "unknown":
            if size == "small":
                notes.append(f"UNKNOWN {label}={shown} 介质探不到（{detail}）—— small 只提醒")
            elif allowed(shown):
                notes.append(f"ALLOWED {label}={shown} 探不到但已放行 —— PR 里要写")
            else:
                failures.append(
                    f"UNKNOWN-MEDIA: {label}={shown} 探不到介质（{detail}）"
                    f"且项目规模={size} ⇒ 按最严判红；确实无解就 --allow-rotational"
                )
        else:
            notes.append(f"OK {label}={shown} 固态（{detail}）")
    return failures, notes


def self_test() -> int:
    """判红的两条都要有真会红的反证；探针用注入的假表，不依赖本机真接一块机械盘。"""
    failures: list[str] = []
    seen = [0]

    def case(name: str, got: bool, want: bool) -> None:
        seen[0] += 1
        good = got == want
        print(("PASS " if good else "FAIL ") + name)
        if not good:
            failures.append(name)

    fake = [Path("/tmp/x")]

    def probe_factory(table: dict[str, tuple[str, str]]):
        def media_of_override(path: Path) -> tuple[str, str]:
            return table[str(path)]

        return media_of_override

    def run(table: dict[str, tuple[str, str]], size: str, allow: list[str] | None = None) -> tuple[list[str], list[str]]:
        global media_of  # noqa: PLW0603 - 自测里替换探针，跑完立刻还原
        saved = media_of
        media_of = probe_factory(table)
        try:
            return check([("repo", fake[0])], size, allow or [])
        finally:
            media_of = saved

    # 1. 正例：固态 + large ⇒ 不判红
    f, _ = run({str(fake[0]): ("ssd", "NVMe XPG")}, "large")
    case("ssd never fails", not f, True)

    # 3. 反证：机械盘 + 任何规模都红
    f, _ = run({str(fake[0]): ("hdd", "WDC WD40EZRZ")}, "large")
    case("hdd fails at large", bool(f) and "ROTATIONAL" in f[0], True)
    f, _ = run({str(fake[0]): ("hdd", "WDC WD40EZRZ")}, "small")
    case("hdd fails even at small", bool(f), True)

    # 4. 反证：探不到介质时按规模分档（small 只提醒，medium/large 判红）
    f, _ = run({str(fake[0]): ("unknown", "无 powershell")}, "small")
    case("unknown is only a notice at small", not f, True)
    f, _ = run({str(fake[0]): ("unknown", "无 powershell")}, "medium")
    case("unknown fails at medium", bool(f) and "UNKNOWN-MEDIA" in f[0], True)

    # 5. 反证：显式放行把红变成提醒（逃生口要存在，也要留痕）
    f, n = run({str(fake[0]): ("hdd", "WDC WD40EZRZ")}, "large", [str(fake[0])])
    case("allow turns red into notice", not f and any("ALLOWED" in x for x in n), True)

    # 6. 规模分类的边界必须按实测阈值翻档
    case("small under 5k", classify(4_999) == "small", True)
    case("medium at 5k", classify(5_000) == "medium", True)
    case("large at 40k", classify(40_000) == "large", True)

    # 7. 纯函数锁：Windows 三路信号怎么合成（本机实况就是这里 —— `Get-Disk.MediaType` 对
    #    SATA 机械盘返回**空串**，只有 `Get-PhysicalDisk` 报 HDD；空集合走 `all()` 会真空为真）。
    def pin(name: str, args: tuple[str, str, str], want: str) -> None:
        seen[0] += 1
        got, _ = read_evidence(*args)
        good = got == want
        print(("PASS " if good else f"FAIL（得到 {got}） ") + name)
        if not good:
            failures.append(name)

    pin("MediaType 空 + Spinning=True -> hdd", ("", "True", ""), "hdd")
    pin("MediaType 空 + PhysicalDisk=HDD -> hdd", ("", "", "HDD"), "hdd")
    pin("两路信号都空 -> unknown（不许默认当固态）", ("", "", ""), "unknown")
    pin("Unspecified 且无正面固态证据 -> unknown", ("Unspecified", "False", ""), "unknown")
    pin("PhysicalDisk=SSD -> ssd", ("", "False", "SSD"), "ssd")
    pin("只有 MediaType 报 HDD（另两路都空）也要判机械", ("HDD", "", ""), "hdd")
    pin("Spinning=False 且 PhysicalDisk=Unspecified 不许当固态", ("", "False", "Unspecified"),
        "unknown")

    # 8. 真实探针必须能用（不然整条门是自娱自乐）：对本仓根跑一次，不许是 hdd
    real_media, real_detail = media_of(ROOT)
    case(f"real probe returns something ({real_media}/{real_detail})", real_media != "hdd", True)
    print(f"  本机实测：repo={ROOT} 介质={real_media} 依据={real_detail}")

    print("self-test: %d/%d passed" % (seen[0] - len(failures), seen[0]))
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    root = ROOT
    extra: list[tuple[str, Path]] = []
    allow: list[str] = []
    mode = "check"
    rest = argv[1:]
    while rest:
        arg = rest.pop(0)
        if arg == "--self-test":
            mode = "self-test"
        elif arg == "--root":
            root = Path(rest.pop(0))
        elif arg == "--path":
            extra.append(("extra", Path(rest.pop(0))))
        elif arg == "--allow-rotational":
            allow.append(rest.pop(0))
        elif arg == "--json":
            mode = "json"
        else:
            print(f"unknown argument: {arg}")
            return 2

    if mode == "self-test":
        return self_test()

    lines, count = source_lines(root)
    size = classify(lines)
    paths = default_paths(root) + extra
    failures, notes = check(paths, size, allow)

    if mode == "json":
        print(
            json.dumps(
                {
                    "root": str(root),
                    "source_lines": lines,
                    "rust_files": count,
                    "size": size,
                    "failures": failures,
                    "notes": notes,
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    else:
        print(
            "disk guard: size=%s（源行 %d / .rs %d 个，阈值 small<%d medium<%d）"
            % (size, lines, count, SMALL_LINES, MEDIUM_LINES)
        )
        for note in notes:
            print("NOTE " + note)
        for failure in failures:
            print("FAIL " + failure)
    if failures:
        print("=> 构建/测试落盘必须在固态盘：改 CARGO_TARGET_DIR / TMP 指到 SSD 盘再跑")
        return 1
    print("OK 全部检查项都在固态（或已显式放行）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
