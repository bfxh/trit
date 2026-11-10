#!/usr/bin/env python3
"""多智能体撞车提醒：谁在动哪个东西，双方都要被点名。

用法：
  python scripts/agent_claims.py claim --who <会话名> --repo <o/r> --paths <a/src b/src> \
      [--note "在做什么"] [--ttl 90]
  python scripts/agent_claims.py release --who <宿主:会话id> [--id <编号>]
  python scripts/agent_claims.py renew --who <宿主:会话id> [--ttl ≤240]   # 长任务心跳
  python scripts/agent_claims.py list
  python scripts/agent_claims.py check          # 有重叠就红，并把**双方**都点名
  python scripts/agent_claims.py gate --who <会话名> --paths <a b> [--json]
      # 三态判定（与库里的 ClaimStore::gate 同一张表）：True=我登记过且全覆盖，
      # False=别人在效期里撞我，Unknown=没登记过（能动，但没人知道）
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
TTL_CAP_MIN = 240
HOSTS = ("qoder", "zcode", "codex", "yan", "eac", "human")
WHO = re.compile(r"^(?P<host>[a-z]+):(?P<id>[a-z0-9][a-z0-9._-]{0,63})$")
_W = "qoder|zcode|codex|yan|eac"
_WHO_HOST_PAREN = re.compile(rf"^(?P<host>{_W})\s*\(\s*(?:sess(?:ion)?[ :]*)?(?P<id>[^)]+?)\s*\)$",
                             re.IGNORECASE)
_WHO_HOST_DASH = re.compile(rf"^(?P<host>{_W})[-_ ](?P<id>.+)$", re.IGNORECASE)
_WHO_ID_PAREN = re.compile(rf"^(?P<id>[^\s(]+?)\s*\(\s*(?P<host>{_W})\s*\)$", re.IGNORECASE)
HEADER = "# 活动声明（多智能体重叠提醒）\n\n机器读写：`scripts/agent_claims.py`。人可读，收工请 release。\n"
LINE = re.compile(r"^\|\s*(?P<id>\w+)\s*\|\s*(?P<who>[^|]*?)\s*\|\s*(?P<repo>[^|]*?)\s*\|"
                  r"\s*(?P<paths>[^|]*?)\s*\|\s*(?P<ts>\d+)\s*\|\s*(?P<ttl>\d+)\s*\|\s*(?P<note>[^|]*?)\s*\|"
                  r"(?:\s*(?P<renewed>\d+)\s*\|)?$")
SEP = "| id | who | repo | paths | claimed_at | ttl_min | note | renewed |"


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


def slug(text: str) -> str:
    return re.sub(r"[^a-z0-9._-]+", "-", text.strip().lower()).strip("-") or "unknown"


def canonical_who(raw: str) -> str:
    """把历史写法归一成 `宿主:会话id`；归不了返回空串（调用方决定警告还是拒绝）。

    实测三种写法描述同一个人：`Qoder(sess dc34ac09)` / `qoder-56607d26` / `zodER(ZCode)`。
    `release --who` 是精确匹配，不归一就会出现"收不掉自己的声明"。
    """
    text = (raw or "").strip()
    if not text:
        return ""
    direct = WHO.match(text.lower())
    if direct:
        return f"{direct.group('host')}:{direct.group('id')}" if direct.group("host") in HOSTS else ""
    for pattern in (_WHO_HOST_PAREN, _WHO_HOST_DASH, _WHO_ID_PAREN):
        match = pattern.match(text)
        if match:
            host = match.group("host").lower()
            return f"{host}:{slug(match.group('id'))}"
    return ""


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
                       "note": fields["note"], "renewed": int(fields.get("renewed") or 0)})
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


# 共享表在单体仓里跟着库走；镜像仓（repo-tools）没有 crates/，就把同一份表放在 fixtures/。
# 两处都读不到才算失败——不是跳过。
PARITY_TABLES = (
    pathlib.Path(__file__).resolve().parents[1]
    / "crates" / "trit-orch" / "tests" / "fixtures" / "claim_gate_cases.json",
    pathlib.Path(__file__).resolve().parents[1] / "fixtures" / "claim_gate_cases.json",
)
INF = 2**63 - 1


def normalize_path(raw: str) -> str:
    """与库里 `normalize_path` 同口径：反斜杠转正斜杠、丢空段与 `.`、去掉首尾斜杠。

    不归一就会漏判同一个文件（`src\\.` 与 `src` 是一种写法）；`..`、盘符、绝对路径直接不收，
    因为声明的键必须是仓内相对路径，否则两边的"同一个路径"没有共同定义。
    """
    path = (raw or "").strip().replace("\\", "/").strip("/")
    if not path or ":" in path:
        return ""
    parts = [part for part in path.split("/") if part not in ("", ".")]
    if not parts or any(part == ".." for part in parts):
        return ""
    return "/".join(parts)


def covers(scope: str, path: str) -> bool:
    """只向下覆盖：`src` 覆盖 `src/a.rs`，`src/a.rs` 不覆盖 `src`。"""
    return scope == path or path.startswith(scope + "/")


def paths_overlap(left: str, right: str) -> bool:
    return covers(left, right) or covers(right, left)


def expires_at_of(claim: dict) -> int:
    """台账行是 `ts + ttl_min`，共享表是绝对 `expires_at`；两边统一到后者比较。

    `ttl <= 0` 在这套 CLI 里表示"永不过期"（收工必须 release），库里没有这种声明，
    所以对拍表里不会出现，这里只把它折成极大值。
    """
    if "expires_at" in claim:
        return int(claim["expires_at"])
    ttl = int(claim.get("ttl", 0))
    return INF if ttl <= 0 else int(claim["ts"]) + ttl * 60


def owner_of_claim(claim: dict) -> str:
    return (claim.get("who") if claim.get("who") is not None else claim.get("owner") or "").strip()


def gate(claims: list[dict], who: str, paths: list[str], at: int) -> str:
    """三态判定：`true` / `unknown` / `false`，与 `ClaimStore::gate` 逐字对齐。

    先扫别人再扫自己：撞了就是 `false`，不许用自己那条把它洗成 `true`。
    全覆盖才算 `true`；其余（没登记、只覆盖一半、别人的已过期）都是 `unknown`。

    **`who` 在这里按原文比较**（与库的 `ClaimStore::gate` 同一口径，共享表也是这么写的）；
    "三种写法是同一个人"那件事在账本边界上做（`canonical_who`，写入与 release/renew 时）。
    两处混在一起，共享表就会跟着 Python 侧的归一规则漂。
    """
    want = (who or "").strip()
    scoped = [normalize_path(p) for p in paths]
    if not want or not scoped or any(not p for p in scoped):
        return "unknown"
    live_claims = [(owner_of_claim(claim), expires_at_of(claim),
                    [normalize_path(x) for x in claim.get("paths", []) if normalize_path(x)])
                   for claim in claims]
    live_claims = [(owner, expires, scopes) for owner, expires, scopes in live_claims if expires > at]

    for path in scoped:
        for owner, _expires, scopes in live_claims:
            if owner != want and any(paths_overlap(scope, path) for scope in scopes):
                return "false"
    for path in scoped:
        held = any(owner == want and any(covers(scope, path) for scope in scopes)
                   for owner, _expires, scopes in live_claims)
        if not held:
            return "unknown"
    return "true"


def parity_cases() -> tuple[list[dict], str]:
    """读共享表。读不到不"跳过"——没有表就等于两侧口径没人对过，必须当成判决失败。"""
    table = next((cand for cand in PARITY_TABLES if cand.is_file()), None)
    if table is None:
        return [], f"共享表读不到（找过 {len(PARITY_TABLES)} 个位置：{', '.join(str(c) for c in PARITY_TABLES)}）"
    try:
        data = json.loads(table.read_text(encoding="utf-8"))
    except ValueError as exc:
        return [], f"共享表不是合法 JSON：{exc}"
    if data.get("spec") != "bfxh/trit claim-gate parity v1":
        return [], f"共享表版本对不上：{data.get('spec')!r}"
    return data.get("cases", []), ""


def _flag(argv: list[str], flag: str) -> str:
    """单值取参：取不到就空串（`--who` 后面没东西时不许拿 IndexError 当判决）。"""
    following = argv[1:]  # 位置 0 是 flag 自己
    return following[argv.index(flag)] if flag in argv and argv.index(flag) < len(following) else ""


def _flag_list(argv: list[str], flag: str) -> list[str]:
    """多值取参：`--paths a b c` 收到下一个 `--` 为止。"""
    if flag not in argv:
        return []
    rest = argv[argv.index(flag) + 1:]
    end = next((i for i, item in enumerate(rest) if item.startswith("--")), len(rest))
    return rest[:end]


def cmd_gate(path: pathlib.Path, argv: list[str]) -> int:
    who = _flag(argv, "--who")
    paths = _flag_list(argv, "--paths")
    claims = read_claims(path)
    verdict = gate(claims, who, paths, now())
    if "--json" in argv:
        print(json.dumps({"tool": "agent-claims-gate", "who": who, "paths": paths,
                          "verdict": verdict, "claims": len(claims)}, ensure_ascii=False))
    else:
        label = {"true": "True（我登记过且全覆盖）", "false": "False（别人在效期里撞你）",
                 "unknown": "Unknown（没登记过；能动，但先 claim 一声）"}[verdict]
        _say(f"gate {who or '(无名)'} @ {paths or ['(没给路径)']} ⇒ {label}")
    return 1 if verdict == "false" else 0


def crate_of_repo(repo: str) -> str:
    """仓名尾段：`bfxh/trit-immune` ⇒ `trit-immune`。"""
    return (repo or "").rstrip("/").split("/")[-1]


def crate_relatives(paths: list[str]) -> dict[str, list[str]]:
    """把 `crates/<crate>/<rest>` 归一成 {crate: [rest]}，其余路径忽略。"""
    out: dict[str, list[str]] = {}
    for path in paths:
        parts = path.strip().strip("/").split("/")
        if len(parts) >= 3 and parts[0] == "crates" and parts[1]:
            out.setdefault(parts[1], []).append("/".join(parts[2:]))
    return out


def crate_twin(left: dict, right: dict) -> str:
    """跨仓的「同一份代码两个落点」；不相交返回空串。

    实测漏网形状（2026-11-03）：一方声明 `bfxh/trit-immune` 的 `src/bin/trit.rs`，
    另一方声明 `bfxh/trit` 的 `crates/trit-immune/src` ⇒ 仓名不同、字面路径也不相交，
    旧口径一声不吭，而两边改的是同一只文件的**两份副本**（一边搬测试、一边加围栏，
    两边都从 719 行那份出发）。
    """
    left_repo = left["repo"].strip()
    right_repo = right["repo"].strip()
    if not left_repo or not right_repo or left_repo == right_repo:
        return ""
    right_crate = crate_of_repo(right_repo)
    left_crates = crate_relatives(left["paths"])
    if right_crate in left_crates and overlaps(left_crates[right_crate], right["paths"]):
        return right_crate
    left_crate = crate_of_repo(left_repo)
    right_crates = crate_relatives(right["paths"])
    if left_crate in right_crates and overlaps(right_crates[left_crate], left["paths"]):
        return left_crate
    return ""

def conflicts(claims: list[dict], at: int) -> tuple[list[str], list[str]]:
    """(拦路:重叠的双方点名, 警告:过期未收 / 跨仓同名路径)。

    拦：**同一个仓 + 路径相交**（含"整仓声明"：空路径集与谁都重叠）⇒ 一定撞，双方点名。
    警：同一个仓但路径不相交 ⇒ 不拦——两个智能体各改各的文件是常态，一律判红等于把闸门自己
    变成噪音源头（第二天就有人绕过它）；但**照样点名双方**，共享工作树/分支这件事得有人记着。
    不同仓但**同一份代码的两个落点**（独立仓 `x` 对上单体仓 `crates/x/…`）⇒ 警，并点出 crate 名；
    不同仓、字面路径名相交 ⇒ 也只警告：单体仓的 `crates/x/src/*` 与独立仓的 `src/*` 就是这个形状。
    """
    red, warn = [], []
    active = [c for c in claims if live(c, at)]
    for index, left in enumerate(active):
        for right in active[index + 1:]:
            same_repo = bool(left["repo"].strip()) and left["repo"].strip() == right["repo"].strip()
            if same_repo and overlaps(left["paths"], right["paths"]):
                red.append(f"{left['who']} ↔ {right['who']}"
                           f"｜同一个仓 {left['repo']} 且路径相交"
                           f"｜路径 {left['paths'] or ['(整个仓)']} × {right['paths'] or ['(整个仓)']}"
                           f"｜{left['note'] or '-'} / {right['note'] or '-'}")
            elif same_repo:
                warn.append(f"{left['who']} ↔ {right['who']}｜同一个仓 {left['repo']} 但路径不相交"
                            f"（{left['paths'] or ['(整个仓)']} × {right['paths'] or ['(整个仓)']}）"
                            "——不拦，但工作树/分支是共享的：切分支、改台账前先对齐，别把对方的 HEAD 带走")
            elif crate_twin(left, right):
                warn.append(f"{left['who']}@{left['repo']} 与 {right['who']}@{right['repo']} 改的是"
                            f"「同一份代码的两个落点」（crate {crate_twin(left, right)}：一边是独立仓、"
                            f"另一边是单体仓里的 crates/{crate_twin(left, right)}/）——不拦，但这是两份副本："
                            "一边搬文件、一边改原件，sync 时必撞；动手前互相报一声路径形状")
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
    seen: dict[str, list[str]] = {}
    for claim in claims:
        seen.setdefault(claim["id"], []).append(claim["who"])
    for cid, owners in seen.items():
        if len(owners) > 1:
            red.append(f"同名声明 id={cid} 出现 {len(owners)} 次（{', '.join(owners)}）⇒ "
                       "按 id 收声明会收错人；库里的 ClaimStore 对同名 id 直接报错，两侧不能不一致")
    for claim in claims:
        fixed = canonical_who(claim["who"])
        if not fixed:
            warn.append(f"{claim['id']} 的 who={claim['who']!r} 归不出「宿主:会话id」；"
                        f"release --who 精确匹配会漏（可用 {'|'.join(HOSTS)}:id）")
        elif fixed != claim["who"]:
            warn.append(f"{claim['id']} 的 who={claim['who']!r} 建议写成 {fixed!r}")
        if claim["ttl"] > TTL_CAP_MIN:
            warn.append(f"{claim['id']} 的 ttl={claim['ttl']} 分钟超过上限 {TTL_CAP_MIN}；"
                        "长任务请用 renew 续期，别一次性占到底")
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
                 str(claim["ts"]), str(claim["ttl"]), claim["note"],
                 str(claim.get("renewed", 0))]
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

    raw_who = value("--who")
    who = canonical_who(raw_who)
    repo = value("--repo")
    paths = [p for p in values("--paths") if p]
    if not raw_who or not repo:
        _say("claim 需要 --who <会话名> 与 --repo <o/r>（谁在动哪个仓，缺一项就没法提醒对方）")
        return 1
    if not who:
        _say(f"FAIL --who {raw_who!r} 归不出「宿主:会话id」（宿主限定 {', '.join(HOSTS)}）。"
             "新声明一律写规范形，否则 release --who 精确匹配收不掉自己的声明")
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
    if ttl > TTL_CAP_MIN:
        _say(f"FAIL ttl={ttl} 超过上限 {TTL_CAP_MIN} 分钟：长任务用 `renew` 续期，"
             "一次占到底会让过期声明长期占位（实测有一本账里 3/4 条过期未收）")
        return 1
    taken = {c["id"] for c in claims}
    serial = 1
    while f"c{serial}" in taken:
        serial += 1
    claims.append({"id": f"c{serial}", "who": who, "repo": repo, "paths": paths,
                   "ts": now(), "ttl": ttl, "note": value("--note")})
    write_claims(path, claims)
    _say(f"已登记：{who} @ {repo} {paths or '(整个仓)'}（{ttl} 分钟；上限 {TTL_CAP_MIN}，"
         "超时请 renew）")
    result = check(path)
    mine = [line for line in result["red"] if who in line]
    if mine:
        _say("提醒：这条与别人的在效声明重叠 ⇒ 双方都要对齐后再动手（下面已点名双方）")
        for line in mine:
            _say(f"  - {line}")
        return 1
    return 0


def cmd_renew(path: pathlib.Path, argv: list[str]) -> int:
    """续期：把 claimed_at 推到现在并可改 ttl。长任务的心跳口。

    不续期就只能靠 ttl 一次占到底，而一次占到底在实测里出过两种坏账：
    过期未收长期占位（3/4 条）与 TTL 与工作时长严重不匹配（1461 分钟 vs 240）。
    """
    def value(flag: str, default: str = "") -> str:
        return argv[argv.index(flag) + 1] if flag in argv and argv.index(flag) + 1 < len(argv) else default

    raw_who = value("--who")
    ident = value("--id")
    who = canonical_who(raw_who)
    if not who and not ident:
        _say("renew 需要 --who <宿主:会话id> 或 --id <编号>")
        return 1
    claims = read_claims(path)
    targets = [c for c in claims
               if (ident and c["id"] == ident)
               or (who and canonical_who(c["who"]) == who)]
    if not targets:
        _say(f"没找到要续期的声明（who={raw_who or '-'} id={ident or '-'}）")
        return 1
    ttl_raw = value("--ttl")
    ttl = int(ttl_raw) if ttl_raw else 0
    if ttl and ttl > TTL_CAP_MIN:
        _say(f"FAIL ttl={ttl} 超过上限 {TTL_CAP_MIN} 分钟")
        return 1
    at = now()
    for claim in targets:
        claim["ts"] = at
        claim["ttl"] = ttl or min(claim["ttl"], TTL_CAP_MIN)
        claim["renewed"] = claim.get("renewed", 0) + 1
    write_claims(path, claims)
    _say(f"已续期 {len(targets)} 条：{', '.join(c['id'] for c in targets)}"
         f"（ttl={targets[0]['ttl']} 分钟，renewed={targets[0]['renewed']}）")
    return 0


def cmd_release(path: pathlib.Path, argv: list[str]) -> int:
    who = argv[argv.index("--who") + 1] if "--who" in argv else ""
    cid = argv[argv.index("--id") + 1] if "--id" in argv else ""
    # 归一再比：三种历史写法描述的是同一个人，精确匹配会让你收不掉自己的声明
    who_key = canonical_who(who)
    claims = read_claims(path)
    # 两个条件同时给时必须同时满足：or 会把我这条 `--who X --id c1` 变成"把 X 全清掉"
    keep = [c for c in claims
            if not ((not who or (canonical_who(c["who"]) or c["who"]) == (who_key or who))
                    and (not cid or c["id"] == cid))]
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
    a = {"id": "c1", "who": "qoder:s1", "repo": "bfxh/trit", "paths": ["crates/trit-immune/src"],
         "ts": base, "ttl": 90, "note": "治理门"}
    b = {"id": "c2", "who": "zcode:zoder", "repo": "bfxh/trit-immune",
         "paths": ["src/tls.rs", "src/diag.rs"], "ts": base, "ttl": 90, "note": "TLS"}
    same = {"id": "c3", "who": "zcode:zoder", "repo": "bfxh/trit", "paths": ["scripts"],
            "ts": base, "ttl": 90, "note": "同仓不同路径"}

    # 正例：路径不相交 ⇒ 绿（注意 repo 不同但 paths 名字像，也不该因为字符串像就判红）
    red, warn = conflicts([a, b], base)
    check_at("路径不相交时判绿", red, [])
    check_at("跨仓同一份代码的两个落点 ⇒ 警并点名 crate",
             [w for w in warn if "两个落点" in w and "trit-immune" in w] != [], True)
    check_at("反向也算（左边才是独立仓）",
             crate_twin({**b, "who": "z"}, {**a, "who": "q"}), "trit-immune")
    check_at("同一个仓不报两个落点（那是拦/警的活）",
             crate_twin(a, {**b, "repo": "bfxh/trit"}), "")
    check_at("同一个仓即使形状像两个落点也不报（拦/警两档不许串）",
             crate_twin({"repo": "bfxh/trit-immune", "paths": ["crates/trit-immune/src"], "who": "x"},
                        {"repo": "bfxh/trit-immune", "paths": ["src/bin/trit.rs"], "who": "y"}), "")
    check_at("不相干的 crate 不报（独立仓 trit-re vs 单体仓 crates/trit-net）",
             crate_twin({"repo": "bfxh/trit", "paths": ["crates/trit-net/src"], "who": "x"},
                        {"repo": "bfxh/trit-re", "paths": ["src/pe.rs"], "who": "y"}), "")
    check_at("整仓声明也算两个落点（paths 为空 = 全部）",
             crate_twin(a, {"repo": "bfxh/trit-immune", "paths": [], "who": "z"}), "trit-immune")
    check_at("crate_relatives 只认 crates/<名>/ 形状",
             crate_relatives(["crates/trit-net/src", "scripts/x.py", "crates"]),
             {"trit-net": ["src"]})
    check_at("仓名尾段即 crate 名", crate_of_repo("bfxh/trit-immune/"), "trit-immune")
    twin_a = {"id": "t1", "who": "Q", "repo": "bfxh/trit", "paths": ["crates/trit-circuit/src"],
              "ts": base, "ttl": 90, "note": "拆文件"}
    twin_b = {"id": "t2", "who": "Z", "repo": "bfxh/trit-circuit", "paths": ["src/sdf.rs"],
              "ts": base, "ttl": 90, "note": "独立仓改原件"}
    red, warn = conflicts([twin_a, twin_b], base)
    check_at("两个落点只警不拦（不同仓不该拦，但必须留痕）",
             (red, [w for w in warn if "两个落点" in w] != []), ([], True))
    # 反证 1：同一个仓 + 路径前缀相交（src 覆盖 src/tls.rs）⇒ 红，双方都在文案里
    red, _ = conflicts([a, {**b, "repo": "bfxh/trit",
                             "paths": ["crates/trit-immune/src/tls.rs"]}], base)
    check_at("前缀相交被判出", len(red), 1)
    check_at("双方都被点名", "qoder:s1 ↔ zcode:zoder" in red[0], True)
    # 反证 1b：不同仓、路径名相交 ⇒ 不拦，但必须出声（同一份代码的两个副本就是这个形状）
    red, warn = conflicts([a, {**b, "paths": ["crates/trit-immune/src/tls.rs"]}], base)
    check_at("跨仓同名路径不判红", red, [])
    check_at("跨仓同名路径要警告", len(warn), 1)
    # 反证 2：同一个 repo + 分支，路径不同也算撞（今天真实形状：两份同名 crate）
    red, warn = conflicts([a, same], base)
    check_at("同仓但路径不相交 ⇒ 不拦（否则闸门自己变噪音源头）", red, [])
    check_at("同仓路径不相交也要点名双方（共享工作树的账）",
             (len(warn), "qoder:s1 ↔ zcode:zoder" in warn[0], "HEAD" in warn[0]), (1, True, True))
    # 反证 3：空路径集 = 整个仓，与同仓的任何声明都重叠
    red, _ = conflicts([a, {**b, "repo": "bfxh/trit", "paths": []}], base)
    check_at("整仓声明与任何人在建的都算重叠", len(red), 1)
    red, warn = conflicts([a, {**b, "paths": []}], base)
    check_at("整仓声明跨仓时也是警告不是拦", (red, len(warn)), ([], 1))
    # 反证 4：过期的不算撞，但要单列成警告（不能悄悄消失）
    red, warn = conflicts([a, {**b, "ts": base - 60 * 600}], base)
    check_at("过期声明不再拦路", red, [])
    check_at("过期未收要警告（必须点名过期这条，不能被别的警告顶数）",
             (len(warn), "过期" in warn[0]), (1, True))
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
        check_at("claim 写进去了", cmd_claim(path, ["claim", "--who", "qoder:s1", "--repo", "o/r",
                                                    "--paths", "src/a", "src/b", "src/c/d.rs",
                                                    "--ttl", "30", "--note", "治理门"]), 0)
        check_at("读回来是 1 条", len(read_claims(path)), 1)
        check_at("--paths 允许多个值（只取一个等于把声明面丢掉）",
                 read_claims(path)[0]["paths"], ["src/a", "src/b", "src/c/d.rs"])
        check_at("备注存得下来（对方要知道你在干什么）", read_claims(path)[0]["note"], "治理门")
        # 反证 3b：自己写的名单自己必须能读干净（表头/分隔行被当坏行时，
        # 一次本来没冲突的检查会直接判红——这类"工具自己造假红"今天犯过一次）
        check_at("自己写的名单没有坏行", check(path)["broken"], [])
        check_at("同一会话重复 claim 被拒", cmd_claim(path, ["claim", "--who", "qoder:s1",
                                                             "--repo", "o/r"]), 1)
        check_at("同一会话可以另开一个仓的声明（一人守多仓是常态）",
                 cmd_claim(path, ["claim", "--who", "qoder:s1", "--repo", "other/r",
                                  "--paths", "src/a"]), 0)
        check_at("ttl=0 当永久占用被拒",
                 cmd_claim(path, ["claim", "--who", "eac:x", "--repo", "o/r", "--ttl", "0"]), 1)
        check_at("另一会话同仓但路径不相交 ⇒ 登记成功（只提醒不拦）",
                 cmd_claim(path, ["claim", "--who", "zcode:zoder", "--repo", "o/r",
                                  "--paths", "src/z"]), 0)
        check_at("另一会话同仓且路径相交 ⇒ 拦并点名双方",
                 cmd_claim(path, ["claim", "--who", "qoder:s3", "--repo", "o/r",
                                  "--paths", "src/a/inner"]), 1)
        check_at("release 能收掉", cmd_release(path, ["--who", "qoder:s1"]), 0)
        check_at("判红那条也是登记过的（等双方对齐后各自 release）", len(read_claims(path)), 2)
        check_at("第二条也收得掉", cmd_release(path, ["--who", "zcode:zoder"]), 0)
        check_at("第三条也收得掉", cmd_release(path, ["--who", "qoder:s3"]), 0)
        check_at("收完确实空了", read_claims(path), [])
        # gate 子命令：三态判定 + 退出码（0=能动，1=别人占着）。用单独一本台账，别搅后面的用例
        gate_ledger = pathlib.Path(tmp) / "GATE.md"
        check_at("gate：台账空 ⇒ unknown 且放行（没登记不等于不能动）",
                 cmd_gate(gate_ledger, ["--who", "qoder:s1", "--paths", "src/a"]), 0)
        cmd_claim(gate_ledger, ["claim", "--who", "qoder:s1", "--repo", "o/r",
                               "--paths", "src/a", "src/b"])
        check_at("gate：自己登记过 ⇒ true（目录覆盖其下文件）",
                 cmd_gate(gate_ledger, ["--who", "qoder:s1", "--paths", "src/a/deep.rs"]), 0)
        check_at("gate：--paths 的多个值都收（只取一个就把声明面丢掉）",
                 cmd_gate(gate_ledger, ["--who", "qoder:s1", "--paths", "src/b"]), 0)
        cmd_claim(gate_ledger, ["claim", "--who", "zcode:zoder", "--repo", "o/r", "--paths", "src/c"])
        check_at("gate：别人占着 ⇒ 拦（退 1）",
                 cmd_gate(gate_ledger, ["--who", "qoder:s1", "--paths", "src/c"]), 1)
        check_at("gate：没登记的第三方也算撞（unknown 不是给别人的通行证）",
                 cmd_gate(gate_ledger, ["--who", "codex:newbie", "--paths", "src/c"]), 1)
        cmd_release(gate_ledger, ["--who", "zcode:zoder"])
        check_at("gate：对方 release 之后不再拦",
                 cmd_gate(gate_ledger, ["--who", "codex:newbie", "--paths", "src/c"]), 0)
        # —— 账本硬化：who 归一 / TTL 上限 / id 唯一 / renew ——
        hard = pathlib.Path(tmp) / "HARD.md"
        check_at("三种历史写法国一成同一个键",
                 {canonical_who("Qoder(sess dc34ac09)"), canonical_who("qoder-dc34ac09"),
                  canonical_who("qoder:dc34ac09")}, {"qoder:dc34ac09"})
        check_at("前缀写法的第三种方向也认（zodER(ZCode)）",
                 canonical_who("zodER(ZCode)"), "zcode:zoder")
        check_at("归不出的写法返回空串（交给调用方拒绝）",
                 (canonical_who("随便"), canonical_who("")), ("", ""))
        check_at("claim 用旧写法也存成规范形",
                 (cmd_claim(hard, ["claim", "--who", "qoder-abcdef12", "--repo", "o/r",
                                   "--paths", "src/x"]),
                  read_claims(hard)[0]["who"]), (0, "qoder:abcdef12"))
        check_at("归不出的 who 直接拒收，且不写进账",
                 (cmd_claim(hard, ["claim", "--who", "随便", "--repo", "o/r"]),
                  len(read_claims(hard))), (1, 1))
        check_at("ttl 超过上限被拒（长任务走 renew）",
                 cmd_claim(hard, ["claim", "--who", "qoder:abcdef12", "--repo", "o/r2",
                                  "--paths", "src/y", "--ttl", str(TTL_CAP_MIN + 60)]), 1)
        before = read_claims(hard)[0]["ts"]
        check_at("renew 推进时间并计数",
                 (cmd_renew(hard, ["--who", "qoder:abcdef12"]),
                  read_claims(hard)[0]["renewed"] >= 1), (0, True))
        check_at("renew 用旧写法也能续规范形的行",
                 cmd_renew(hard, ["--who", "qoder-abcdef12"]), 0)
        check_at("renew 找不到就报错",
                 cmd_renew(hard, ["--who", "zcode:nobody"]), 1)
        check_at("renew 的 ttl 也受上限约束",
                 cmd_renew(hard, ["--id", read_claims(hard)[0]["id"],
                                 "--ttl", str(TTL_CAP_MIN + 1)]), 1)
        check_at("release 用旧写法能收掉规范形的行",
                 cmd_release(hard, ["--who", "qoder-abcdef12"]), 0)
        check_at("收完之后确实是空的", read_claims(hard), [])

        dup = pathlib.Path(tmp) / "DUP.md"
        dup_rows = ["| c1 | qoder:a | o/r | src/x | 1000000000 | 90 | one | 0 |",
                    "| c1 | zcode:b | o/r | src/y | 1000000000 | 90 | two | 0 |"]
        dup.write_text(chr(10).join([HEADER.rstrip(), "", SEP,
                                    "| --- | --- | --- | --- | --- | --- | --- |", *dup_rows])
                       + chr(10), encoding="utf-8")
        res = check(dup)
        check_at("同名 id 判红（按 id 收声明会收错人）",
                 any("同名声明" in line for line in res["red"]), True)

        legacy = pathlib.Path(tmp) / "LEG.md"
        leg_rows = ["| c1 | Qoder(sess dc34ac09) | o/r | src/x | 1000000000 | 600 | three | 0 |"]
        legacy.write_text(chr(10).join([HEADER.rstrip(), "", SEP,
                                       "| --- | --- | --- | --- | --- | --- | --- |", *leg_rows])
                          + chr(10), encoding="utf-8")
        res = check(legacy)
        check_at("旧 who 给警告并给出照抄值",
                 any("qoder:dc34ac09" in line for line in res["warn"]), True)
        check_at("历史行 ttl 超上限也只警告（不追溯判红）",
                 any("超过上限" in line for line in res["warn"]), True)
        check_at("这些警告不改绿/红结论", res["green"], True)

        check_at("含 | 的备注不会把名单写坏",
                 cmd_claim(path, ["claim", "--who", "qoder:s3", "--repo", "third/r",
                                  "--paths", "docs", "--note", "在合并 A|B 两条"]), 0)
        check_at("备注里的 | 被换掉（否则解析被截断）", read_claims(path)[0]["note"], "在合并 A/B 两条")
        check_at("转义后仍只有一条", len(read_claims(path)), 1)
        check_at("带 | 的名单也没有坏行", check(path)["broken"], [])
        check_at("没东西可释放时报错不静默", cmd_release(path, ["--who", "qoder:s1"]), 1)
        cmd_claim(path, ["claim", "--who", "yan:multi", "--repo", "m/r", "--paths", "x"])
        before = len(read_claims(path))
        check_at("release 的 --who 与 --id 是且不是或（id 不匹配就一条都不许删）",
                 (cmd_release(path, ["--who", "yan:multi", "--id", "不存在"]), len(read_claims(path))),
                 (1, before))
        multi = [c for c in read_claims(path) if canonical_who(c["who"]) == "yan:multi"]
        check_at("两个条件都对上才删",
                 cmd_release(path, ["--who", "yan:multi", "--id", multi[0]["id"]]), 0)
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

    cases, table_error = parity_cases()
    if table_error:
        check_at("认领闸门共享表可读", table_error, "")
    else:
        checked = {"true": 0, "false": 0, "unknown": 0}
        for case in cases:
            query = case["query"]
            got = gate(case["claims"], query["owner"], query["paths"], query["now"])
            checked[case["expect"]] = checked.get(case["expect"], 0) + 1
            check_at(f"共享表 {case['name']}", got, case["expect"])
        check_at("共享表三态都覆盖到", all(v > 0 for v in checked.values()), True)

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
    if command == "renew":
        return cmd_renew(path, argv[1:])
    if command == "gate":
        return cmd_gate(path, argv[1:])
    if command in ("list", "check", ""):
        result = check(path)
        render(result)
        return 0 if result["green"] else 1
    _say(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
