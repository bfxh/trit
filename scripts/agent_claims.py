#!/usr/bin/env python3
"""多智能体撞车提醒：谁在动哪个东西，双方都要被点名。

用法：
  python scripts/agent_claims.py claim --who <会话名> --repo <o/r> --paths <a/src b/src> \
      [--note "在做什么"] [--ttl 90]
  python scripts/agent_claims.py release --who <会话名> [--id <编号>]
  python scripts/agent_claims.py list
  python scripts/agent_claims.py check          # 有重叠就红，并把**双方**都点名
  python scripts/agent_claims.py --self-test

为什么要有这个（2026-11-03 实测的形状）：
  另一个智能体（ZCode 的 zodER）在 `~/.zcode/workspace/draft-*/trit-immune`
  做 TLS + 诊断解析扩展，6 个文件改了没提交；我这边同时在 `D:/KF/trit-immune`
  （同一个 crate 在单体仓里的另一份）推治理门。**两个仓目录同名、crate 同名、
  文件同名**，但两边都不知道对方在动它——直到人来提醒。
  交接账本 LEDGER 是"事后记账"，挡不住"同时动手"。

口径：
  - 声明（claim）= 我现在在动哪个仓的哪些路径，带 TTL；收工必须 release；
  - 两个**在效期内**的声明路径相交（或同一个 repo+分支）⇒ 判红，双方会话名都印出来；
  - 名单文件读不出/时间戳坏掉 ⇒ 判红（"不知道有没有人"不等于"没人"）；
  - 过期未收的声明单列为警告（可能那个会话已经死了，也可能它还在跑只是忘了续）。
"""

from __future__ import annotations

import json
import pathlib
import re
import sys
import time

DEFAULT_CLAIMS = pathlib.Path(r"D:\KF\_handoff\CLAIMS.md")
HEADER = "# 活动声明（多智能体重叠提醒）\n\n机器读写：`scripts/agent_claims.py`。人可读，收工请 release。\n"
LINE = re.compile(r"^\|\s*(?P<id>\w+)\s*\|\s*(?P<who>[^|]*?)\s*\|\s*(?P<repo>[^|]*?)\s*\|"
                  r"\s*(?P<paths>[^|]*?)\s*\|\s*(?P<ts>\d+)\s*\|\s*(?P<ttl>\d+)\s*\|\s*(?P<note>[^|]*?)\s*\|")
SEP = "| id | who | repo | paths | claimed_at | ttl_min | note |"


def _say(text: str) -> None:
    """中文 Windows 控制台是 cp936：判决文本里的 ⇒ 这类字符会把闸门自己崩掉
    （同一个 bug 今天犯了三次，所以这条从第一行就按降级打印写）。"""
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    try:
        print(text)
    except UnicodeEncodeError:
        print(text.encode(encoding, "replace").decode(encoding, "replace"))


def now() -> int:
    return int(time.time())


def parse_claims(text: str) -> tuple[list[dict], list[str]]:
    """返回 (声明, 解析不了的行)。坏行必须交出去，不许静默丢掉。"""
    claims, broken = [], []
    for raw in text.splitlines():
        line = raw.strip()
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        # 表头行与分隔行不是声明：它们曾被当成坏行，把一次本来干净的检查判成红
        if not cells or cells[0] in ("", "id") or all(set(cell) <= set("-: ") for cell in cells):
            continue
        match = LINE.match(line)
        if not match:
            broken.append(line[:120])
            continue
        fields = match.groupdict()
        claims.append({"id": fields["id"], "who": fields["who"], "repo": fields["repo"],
                       "paths": [p for p in fields["paths"].split(",") if p.strip()],
                       "ts": int(fields["ts"]), "ttl": int(fields["ttl"]),
                       "note": fields["note"]})
    return claims, broken


def live(claim: dict, at: int) -> bool:
    return claim["ttl"] <= 0 or at - claim["ts"] <= claim["ttl"] * 60


def overlaps(a: list[str], b: list[str]) -> bool:
    """路径前缀相交算重叠（`src` 覆盖 `src/tls.rs`）。空路径集 = 整个仓 ⇒ 与谁都重叠。"""
    if not a or not b:
        return True
    for x in a:
        for y in b:
            if x == y or x.startswith(y.rstrip("/") + "/") or y.startswith(x.rstrip("/") + "/"):
                return True
    return False


def conflicts(claims: list[dict], at: int) -> tuple[list[str], list[str]]:
    """(拦路:重叠的双方点名, 警告:过期未收 / 跨仓同名路径)。

    拦：**同一个仓**里两个在效声明 ⇒ 一定撞（不管路径写没写、写的是不是同一批）。
    不同仓 ⇒ 不拦。两份代码在两个仓里各有一份（单体仓的 `crates/x/src/*` 与独立仓的 `src/*`）
    路径名相交是常态，判红就是把闸门自己变成假红源头；但这种情况要**出声**，所以给警告。
    """
    red, warn = [], []
    active = [c for c in claims if live(c, at)]
    for index, left in enumerate(active):
        for right in active[index + 1:]:
            same_repo = bool(left["repo"].strip()) and left["repo"].strip() == right["repo"].strip()
            if same_repo:
                red.append(f"{left['who']} ↔ {right['who']}"
                           f"｜同一个仓 {left['repo']}"
                           f"｜路径 {left['paths'] or ['(整个仓)']} × {right['paths'] or ['(整个仓)']}"
                           f"｜{left['note'] or '-'} / {right['note'] or '-'}")
            elif overlaps(left["paths"], right["paths"]):
                warn.append(f"{left['who']}@{left['repo']} 与 {right['who']}@{right['repo']} "
                            f"路径名相交（{left['paths']} × {right['paths']}）"
                            "——不同仓不拦，但如果这是同一份代码的两份副本，先对齐")
    for claim in claims:
        if not live(claim, at):
            stale_min = (at - claim["ts"]) // 60 - claim["ttl"]
            warn.append(f"{claim['who']}｜{claim['repo']}｜过期 {stale_min} 分钟未收"
                        f"（那个会话可能已经死了，也可能还在跑忘续期）")
    return red, warn


def check(path: pathlib.Path) -> dict:
    if not path.is_file():
        # 文件不存在 = 没人登记过，但**也可能是这套还没被采用**；
        # 这不是"确认没人"，所以只报一条警告，不当成绿。
        return {"tool": "agent-claims", "claims": [], "red": [],
                "warn": ["没有任何声明文件：新会话开工前请先 claim（这条不判红，但等于没闸门）"],
                "broken": [], "green": True}
    claims, broken = parse_claims(path.read_text(encoding="utf-8"))
    red, warn = conflicts(claims, now())
    red += [f"名单里有解析不了的行（不知道是谁在动什么 ⇒ 判红）：{line}" for line in broken]
    return {"tool": "agent-claims", "claims": claims, "red": red, "warn": warn,
            "broken": broken, "green": not red}


def render(result: dict) -> None:
    say = _say
    say(f"声明 {len(result['claims'])} 条｜拦 {len(result['red'])} 条｜警 {len(result['warn'])} 条")
    for line in result["red"]:
        say(f"  [拦] {line}")
    for line in result["warn"]:
        say(f"  [警] {line}")
    for claim in result["claims"]:
        say(f"  - {claim['who']} @ {claim['repo']}：{claim['paths'] or '(整个仓)'}"
            f"（{claim['note'] or '无备注'}）")
    say("结论：" + ("绿（没有两个在效期的声明指向同一个东西）" if result["green"]
                  else "红（有智能体在动同一个东西，双方都要停一下对齐）"))


def read_claims(path: pathlib.Path) -> list[dict]:
    return parse_claims(path.read_text(encoding="utf-8"))[0] if path.is_file() else []


def write_claims(path: pathlib.Path, claims: list[dict]) -> None:
    lines = [HEADER, "", SEP, "| --- | --- | --- | --- | --- | --- | --- |"]
    for claim in claims:
        cells = [claim["id"], claim["who"], claim["repo"], ",".join(claim["paths"]),
                 str(claim["ts"]), str(claim["ttl"]), claim["note"]]
        # 字段里出现 | 会截断解析 ⇒ 写侧就换掉，别把名单写坏
        lines.append("| " + " | ".join(str(cell).replace("|", "/") for cell in cells) + " |")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def cmd_claim(path: pathlib.Path, argv: list[str]) -> int:
    def value(flag: str, default: str = "") -> str:
        return argv[argv.index(flag) + 1] if flag in argv and argv.index(flag) + 1 < len(argv) else default

    def values(flag: str) -> list[str]:
        """`--paths a b c` 取到下一个选项为止——只取一个值等于把大半张声明面丢掉。"""
        if flag not in argv:
            return []
        out, index = [], argv.index(flag) + 1
        while index < len(argv) and not argv[index].startswith("--"):
            out.append(argv[index])
            index += 1
        return out

    who = value("--who")
    repo = value("--repo")
    paths = [p for p in values("--paths") if p]
    if not who or not repo:
        _say("claim 需要 --who <会话名> 与 --repo <o/r>（谁在动哪个仓，缺一项就没法提醒对方）")
        return 1
    claims = read_claims(path)
    # 一个会话可以同时守几个仓；只有"同仓 + 同路径"才是重复声明
    dupes = [c for c in claims if c["who"] == who and c["repo"] == repo
             and overlaps([x.rstrip("/") for x in c["paths"]], [x.rstrip("/") for x in paths])]
    if dupes:
        _say(f"{who} 已在 {repo} 声明过这些路径：{dupes[0]['paths'] or '(整个仓)'}"
             "（要换范围先 release）")
        return 1
    ttl = int(value("--ttl", "90"))
    if ttl <= 0:
        _say("ttl 必须是正数分钟（不许用 0 表示永久占用，那样别人永远解不开）")
        return 1
    claims.append({"id": f"c{len(claims) + 1}", "who": who, "repo": repo, "paths": paths,
                   "ts": now(), "ttl": ttl, "note": value("--note")})
    write_claims(path, claims)
    _say(f"已登记：{who} @ {repo} {paths or '(整个仓)'}（{ttl} 分钟）")
    result = check(path)
    mine = [line for line in result["red"] if who in line]
    if mine:
        _say("提醒：这条与别人的在效声明重叠 ⇒ 双方都要对齐后再动手（下面已点名双方）")
        for line in mine:
            _say(f"  - {line}")
        return 1
    return 0


def cmd_release(path: pathlib.Path, argv: list[str]) -> int:
    who = argv[argv.index("--who") + 1] if "--who" in argv else ""
    cid = argv[argv.index("--id") + 1] if "--id" in argv else ""
    claims = read_claims(path)
    # 两个条件同时给时必须同时满足：or 会把我这条 `--who X --id c1` 变成"把 X 全清掉"
    keep = [c for c in claims
            if not ((not who or c["who"] == who) and (not cid or c["id"] == cid))]
    if len(keep) == len(claims):
        _say(f"没找到要释放的声明（who={who or '-'} id={cid or '-'}）")
        return 1
    write_claims(path, keep)
    _say(f"已释放 {len(claims) - len(keep)} 条")
    return 0


def self_test() -> int:
    """每条判红都要有真的会红的反证；绿例必须真跑。"""
    import io
    import tempfile

    total, failures = 0, []

    def check_at(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            _say(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    base = now()
    a = {"id": "c1", "who": "Qoder", "repo": "bfxh/trit", "paths": ["crates/trit-immune/src"],
         "ts": base, "ttl": 90, "note": "治理门"}
    b = {"id": "c2", "who": "zodER", "repo": "bfxh/trit-immune",
         "paths": ["src/tls.rs", "src/diag.rs"], "ts": base, "ttl": 90, "note": "TLS"}
    same = {"id": "c3", "who": "zodER", "repo": "bfxh/trit", "paths": ["scripts"],
            "ts": base, "ttl": 90, "note": "同仓不同路径"}

    # 正例：路径不相交 ⇒ 绿（注意 repo 不同但 paths 名字像，也不该因为字符串像就判红）
    red, warn = conflicts([a, b], base)
    check_at("路径不相交时判绿", red, [])
    # 反证 1：同一个仓 + 路径前缀相交（src 覆盖 src/tls.rs）⇒ 红，双方都在文案里
    red, _ = conflicts([a, {**b, "repo": "bfxh/trit",
                             "paths": ["crates/trit-immune/src/tls.rs"]}], base)
    check_at("前缀相交被判出", len(red), 1)
    check_at("双方都被点名", "Qoder ↔ zodER" in red[0], True)
    # 反证 1b：不同仓、路径名相交 ⇒ 不拦，但必须出声（同一份代码的两个副本就是这个形状）
    red, warn = conflicts([a, {**b, "paths": ["crates/trit-immune/src/tls.rs"]}], base)
    check_at("跨仓同名路径不判红", red, [])
    check_at("跨仓同名路径要警告", len(warn), 1)
    # 反证 2：同一个 repo + 分支，路径不同也算撞（今天真实形状：两份同名 crate）
    red, _ = conflicts([a, same], base)
    check_at("同仓不同路径也拦", len(red), 1)
    # 反证 3：空路径集 = 整个仓，与同仓的任何声明都重叠
    red, _ = conflicts([a, {**b, "repo": "bfxh/trit", "paths": []}], base)
    check_at("整仓声明与任何人在建的都算重叠", len(red), 1)
    red, warn = conflicts([a, {**b, "paths": []}], base)
    check_at("整仓声明跨仓时也是警告不是拦", (red, len(warn)), ([], 1))
    # 反证 4：过期的不算撞，但要单列成警告（不能悄悄消失）
    red, warn = conflicts([a, {**b, "ts": base - 60 * 600}], base)
    check_at("过期声明不再拦路", red, [])
    check_at("过期未收要警告", len(warn), 1)
    # 反证 5：坏行必须判红（不知道是谁在动什么 ≠ 没人动）
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "CLAIMS.md"
        path.write_text(HEADER + "\n" + SEP + "\n| --- | --- | --- | --- | --- | --- | --- |\n"
                        "| cX | 谁? |  |  | 不是数字 | 90 |  |\n", encoding="utf-8")
        result = check(path)
        check_at("坏行不判绿", result["green"], False)
        check_at("坏行有点名", any("解析不了" in line for line in result["red"]), True)
        # 反证 6：claim/release 往返（真写文件）
        path.unlink()
        check_at("claim 需要 who+repo",
                 cmd_claim(path, ["claim", "--repo", "o/r"]), 1)
        check_at("claim 写进去了", cmd_claim(path, ["claim", "--who", "Qoder", "--repo", "o/r",
                                                    "--paths", "src/a", "src/b", "src/c/d.rs",
                                                    "--ttl", "30", "--note", "治理门"]), 0)
        check_at("读回来是 1 条", len(read_claims(path)), 1)
        check_at("--paths 允许多个值（只取一个等于把声明面丢掉）",
                 read_claims(path)[0]["paths"], ["src/a", "src/b", "src/c/d.rs"])
        check_at("备注存得下来（对方要知道你在干什么）", read_claims(path)[0]["note"], "治理门")
        # 反证 3b：自己写的名单自己必须能读干净（表头/分隔行被当坏行时，
        # 一次本来没冲突的检查会直接判红——这类"工具自己造假红"今天犯过一次）
        check_at("自己写的名单没有坏行", check(path)["broken"], [])
        check_at("同一会话重复 claim 被拒", cmd_claim(path, ["claim", "--who", "Qoder",
                                                             "--repo", "o/r"]), 1)
        check_at("同一会话可以另开一个仓的声明（一人守多仓是常态）",
                 cmd_claim(path, ["claim", "--who", "Qoder", "--repo", "other/r",
                                  "--paths", "src/a"]), 0)
        check_at("ttl=0 当永久占用被拒",
                 cmd_claim(path, ["claim", "--who", "X", "--repo", "o/r", "--ttl", "0"]), 1)
        check_at("另一会话同仓 ⇒ 拦并点名双方",
                 cmd_claim(path, ["claim", "--who", "zodER", "--repo", "o/r",
                                  "--paths", "src/b"]), 1)
        check_at("release 能收掉", cmd_release(path, ["--who", "Qoder"]), 0)
        check_at("冲突那条也已登记（等双方对齐后各自 release）", len(read_claims(path)), 1)
        check_at("第二条也收得掉", cmd_release(path, ["--who", "zodER"]), 0)
        check_at("收完确实空了", read_claims(path), [])
        check_at("含 | 的备注不会把名单写坏",
                 cmd_claim(path, ["claim", "--who", "Third", "--repo", "third/r",
                                  "--paths", "docs", "--note", "在合并 A|B 两条"]), 0)
        check_at("备注里的 | 被换掉（否则解析被截断）", read_claims(path)[0]["note"], "在合并 A/B 两条")
        check_at("转义后仍只有一条", len(read_claims(path)), 1)
        check_at("带 | 的名单也没有坏行", check(path)["broken"], [])
        check_at("没东西可释放时报错不静默", cmd_release(path, ["--who", "Qoder"]), 1)
        cmd_claim(path, ["claim", "--who", "Multi", "--repo", "m/r", "--paths", "x"])
        before = len(read_claims(path))
        check_at("release 的 --who 与 --id 是且不是或（id 不匹配就一条都不许删）",
                 (cmd_release(path, ["--who", "Multi", "--id", "不存在"]), len(read_claims(path))),
                 (1, before))
        multi = [c for c in read_claims(path) if c["who"] == "Multi"]
        check_at("两个条件都对上才删",
                 cmd_release(path, ["--who", "Multi", "--id", multi[0]["id"]]), 0)
        # 反证 7：名单文件不存在 ≠ 没人，只是闸门还没启用（不判红，但必须说）
        absent = check(path.parent / "没有这个文件.md")
        check_at("没有声明文件时不判红但要说",
                 (absent["green"], len(absent["warn"]) >= 1), (True, True))
        # 反证 8：cp936 控制台下不许把闸门自己崩掉。
        # 注意打印的判决里**必须真有一个 GBK 放不下的字符**，否则这条夹具是空转的
        # （第一版拿一份干净名单去打印，变异把降级去掉也不崩 ⇒ 门报 SURVIVED）。
        arrow_path = path.parent / "坏名单.md"
        arrow_path.write_text(HEADER + "\n" + SEP + "\n| --- | --- | --- | --- | --- | --- | --- |\n"
                              "| x | 谁? |  |  | 不是数字 | 90 |  |\n", encoding="utf-8")
        probe_verdict = check(arrow_path)
        check_at("夹具里确实有一条带 ⇒ 的判决",
                 any("⇒" in line for line in probe_verdict["red"]), True)
        probe = io.TextIOWrapper(io.BytesIO(), encoding="cp936", errors="strict", newline="\n")
        saved, sys.stdout = sys.stdout, probe
        try:
            render(probe_verdict)
            rc = 0
        except UnicodeEncodeError:
            rc = -1
        finally:
            sys.stdout = saved
        check_at("cp936 控制台下打印判决不崩", rc, 0)

    _say(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    path = DEFAULT_CLAIMS
    if "--claims" in argv:
        path = pathlib.Path(argv[argv.index("--claims") + 1])
    command = argv[0] if argv else ""
    if command == "claim":
        return cmd_claim(path, argv[1:])
    if command == "release":
        return cmd_release(path, argv[1:])
    if command in ("list", "check", ""):
        result = check(path)
        render(result)
        return 0 if result["green"] else 1
    _say(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
