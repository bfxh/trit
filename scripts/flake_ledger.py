#!/usr/bin/env python3
"""flake 的**债务面**：重跑掩盖 + 隔离台账必须带 owner/理由/到期/复验。

用法：
  python scripts/flake_ledger.py check [--repo o/r] [--limit 200] [--days 14] [--json]
  python scripts/flake_ledger.py ledger            # 只看台账（离线、不查 API）
  python scripts/flake_ledger.py --self-test

社区里"CI 偶发失败"这类痛，检测只是一半，另一半是**债务有没有人背**：
  - 同一个 head 失败过又"绿"了，但绿的那次是**重跑**（`run_attempt > 1`）——
    重跑掩盖等于没有 flake，没人记账；
  - 隔离（quarantine）条目没有 owner / 没有理由 / 没有到期日 / 早过期还在躺着——
    今天的"暂时跳过"就是明年的"没人记得为什么跳过"（这也是 `test_surface`
    拦裸 `#[ignore]` 的同一类事故）；
  - 窗口里再没抖过的项目没人摘 ⇒ 台账只会变胖。

判据（fail-closed）：
  拿不到 run 列表 ⇒ 红（"查不到"不等于"没有 flake"）；
  重跑才转绿的 workflow 没有台账覆盖 ⇒ 红；
  台账条目缺 owner/reason/expires、日期读不出来、已过期、重复登记 ⇒ 红；
  台账安静了很久 ⇒ 警告（该摘没人摘）。
"""

from __future__ import annotations

import datetime as dt
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_LEDGER = pathlib.Path(__file__).resolve().parent / "flake_quarantine.json"
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")


def _say(text: str) -> None:
    """中文 Windows 控制台是 cp936：判决文本里的 ⇒/箭头不能把闸门自己崩掉。"""
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    try:
        print(text)
    except UnicodeEncodeError:
        print(text.encode(encoding, "replace").decode(encoding, "replace"))


def parse_day(text) -> dt.date | None:
    if not isinstance(text, str) or not DATE_RE.match(text.strip()):
        return None
    try:
        year, month, day = (int(part) for part in text.strip().split("-"))
        return dt.date(year, month, day)
    except ValueError:
        return None


def load_ledger(path: pathlib.Path) -> tuple[list[dict], str]:
    """返回 (条目, 错误说明)。读不出来必须交出去，不许当"没有隔离"。"""
    if not path.is_file():
        return [], ""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        return [], f"台账不是合法 JSON：{exc}"
    if not isinstance(data, list):
        return [], "台账顶层必须是数组（每项一条隔离记录）"
    return data, ""


def run_stamp(run: dict) -> str:
    return str(run.get("name") or run.get("workflow_name") or run.get("id") or "?")


def masked_green(runs: list[dict]) -> list[dict]:
    """这抹绿是第 N 次尝试拿到的：`run_attempt > 1` 且 `conclusion == success`。

    两档（判红要准，不能把"抢不到 runner 后重跑"也算成掩盖）：
      - `definitive=True`：同一个 (head, workflow) 留有失败记录 ⇒ 先红后绿，确凿掩盖；
      - `definitive=False`：只有"attempt>1 才绿"这一条痕迹 ⇒ 警告。
        （GitHub 的 runs 列表在重跑后只保留最新结论，失败痕迹会被覆盖——
        真实教训：`bfxh/trit-immune` 那次 macOS 抢不到 runner 的重跑，
        第一版"必须同时看到红和绿"的规则**一条也没抓到**。）
    """
    grouped: dict[tuple[str, str], list[dict]] = {}
    for run in runs:
        key = (str(run.get("head_sha") or ""), run_stamp(run))
        grouped.setdefault(key, []).append(run)
    found: list[dict] = []
    for (sha, name), attempts in sorted(grouped.items()):
        failed = [r for r in attempts
                  if str(r.get("conclusion") or "").lower() in {"failure", "timed_out", "cancelled"}]
        green_late = [r for r in attempts
                      if str(r.get("conclusion") or "").lower() == "success"
                      and int(r.get("run_attempt") or 1) > 1]
        if not green_late:
            continue
        found.append({"sha": sha, "name": name,
                      "attempt": max(int(r.get("run_attempt") or 1) for r in green_late),
                      "red_first": sorted({str(r.get("conclusion")) for r in failed}),
                      "definitive": bool(failed)})
    return found


def ledger_problems(ledger: list[dict], today: dt.date) -> list[dict]:
    out: list[dict] = []
    seen: set[str] = set()
    for entry in ledger:
        test = str(entry.get("test") or "").strip()
        if not test:
            out.append({"kind": "隔离项没写是哪个测试", "detail": json.dumps(entry, ensure_ascii=False)[:120]})
            continue
        if test in seen:
            out.append({"kind": "同一个测试重复登记", "detail": test})
        seen.add(test)
        for field, label in (("owner", "没人背"), ("reason", "没写为什么跳过"),
                             ("added", "没写哪天加的"), ("expires", "没写到期日")):
            if not str(entry.get(field) or "").strip():
                out.append({"kind": f"隔离项缺 {field}", "detail": f"{test}：{label}"})
        expires = parse_day(entry.get("expires"))
        if entry.get("expires") and expires is None:
            out.append({"kind": "到期日读不出来", "detail": f"{test}：expires={entry.get('expires')!r} 不是 YYYY-MM-DD"})
        if expires and expires < today:
            out.append({"kind": "隔离已过期还在躺着", "detail":
                        f"{test}：{expires} 到期（owner={entry.get('owner') or '?'}）——"
                        "要么复验后摘掉，要么改期并写清为什么还要隔"})
    return out


def covered(name: str, ledger: list[dict]) -> bool:
    """台账要能覆盖这个 workflow/job 名（子串匹配，允许登记到更细的测试名）。"""
    target = name.lower()
    return any(target in str(entry.get("test") or "").lower()
               or str(entry.get("test") or "").lower() in target for entry in ledger)


def cross_check(masked: list[dict], ledger: list[dict]) -> list[dict]:
    out = []
    for hit in masked:
        if covered(hit["name"], ledger):
            continue          # 已经有人背这条：不再重复报
        trace = ("/".join(hit["red_first"])) if hit["red_first"] else "无失败痕迹（重跑覆盖了）"
        if hit["definitive"]:
            out.append({"kind": "重跑掩盖，且没人记账", "blocking": True, "detail":
                        f"{hit['name']} @ {hit['sha'][:12]}：先 {trace} 再在 attempt={hit['attempt']} 转绿"
                        " ⇒ 这是 flake，不是「环境抖一下」。进 flake_quarantine.json"
                        "（owner/reason/added/expires）或修它"})
        else:
            out.append({"kind": "绿是重跑拿到的（无失败痕迹）", "blocking": False, "detail":
                        f"{hit['name']} @ {hit['sha'][:12]}：attempt={hit['attempt']} 才绿，{trace}"
                        " —— 多半是 runner 没抢到或手动重跑；复核一下，要长期就这样就记账让它闭嘴"})
    return out


def quiet_entries(ledger: list[dict], seen_names: list[str], today: dt.date) -> list[dict]:
    """窗口里没再抖过的隔离项 ⇒ 警告该摘掉（不判红：窗口太短就红会让人直接删台账）。"""
    out = []
    lowered = [n.lower() for n in seen_names]
    for entry in ledger:
        test = str(entry.get("test") or "").lower().strip()
        if not test:
            continue
        if not any(test in name or name in test for name in lowered):
            out.append({"kind": "隔离项窗口内没再抖", "detail":
                        f"{entry.get('test')}（owner={entry.get('owner') or '?'}）"
                        "——到期前该复验并摘掉，台账只会越积越胖"})
    return out


def verdict(runs: list[dict] | None, ledger: list[dict], today: dt.date,
            ledger_error: str = "") -> dict:
    findings: list[dict] = []

    def add(kind: str, detail: str, blocking: bool = True) -> None:
        findings.append({"kind": kind, "detail": detail, "blocking": blocking})

    if ledger_error:
        add("台账读不出来", ledger_error)
    if runs is None:
        add("拿不到 run 列表", "API 没成功就是没有证据，不能折成「没有 flake」")
        return {"tool": "flake-ledger", "findings": findings, "blocking": 1,
                "warnings": 0, "red": True, "runs": 0, "masked": 0}
    masked = masked_green(runs)
    findings.extend(cross_check(masked, ledger))
    findings.extend(ledger_problems(ledger, today))
    warnings = quiet_entries(ledger, [hit["name"] for hit in masked], today)
    findings.extend({**warning, "blocking": False} for warning in warnings)
    for finding in findings:
        finding.setdefault("blocking", True)
    return {"tool": "flake-ledger", "findings": findings,
            "blocking": sum(1 for f in findings if f["blocking"]),
            "warnings": sum(1 for f in findings if not f["blocking"]),
            "red": any(finding["blocking"] for finding in findings),
            "runs": len(runs), "masked": len(masked), "ledger": len(ledger)}


def api(path: str) -> tuple[bool, object]:
    result = subprocess.run(["gh", "api", path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, encoding="utf-8", errors="replace")
    if result.returncode != 0:
        return False, result.stdout.strip()[:200]
    try:
        return True, json.loads(result.stdout or "null")
    except json.JSONDecodeError:
        return False, "不是 JSON：" + result.stdout[:160]


def fetch_runs(repo: str, limit: int) -> tuple[list[dict] | None, str]:
    runs: list[dict] = []
    page = 1
    while len(runs) < limit:
        ok, body = api(f"repos/{repo}/actions/runs?per_page=100&page={page}")
        if not ok:
            return None, str(body)
        chunk = body.get("workflow_runs") if isinstance(body, dict) else None
        if chunk is None:
            return None, f"run 列表形状不认识（第 {page} 页）"
        runs.extend(chunk)
        if not chunk or len(chunk) < 100:
            break
        page += 1
    return runs[:limit], ""


def render(result: dict) -> None:
    _say(f"run {result['runs']} 次｜重跑掩盖 {result.get('masked', 0)} 处｜"
         f"隔离 {result.get('ledger', 0)} 条｜拦 {result['blocking']} 条｜警 {result['warnings']} 条")
    for finding in result["findings"]:
        _say(f"  [{'拦' if finding['blocking'] else '警'}|{finding['kind']}] {finding['detail']}")
    _say("结论：" + ("红（有 flake 没被记账，或台账本身不合规）" if result["red"]
                  else "绿（没有靠重跑转绿的记录，台账每条都有主有期）"))


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()

    def value(flag: str, default: str = "") -> str:
        return argv[argv.index(flag) + 1] if flag in argv and argv.index(flag) + 1 < len(argv) else default

    ledger_path = pathlib.Path(value("--ledger", str(DEFAULT_LEDGER)))
    ledger, ledger_error = load_ledger(ledger_path)
    today = dt.date.today()
    command = argv[0] if argv and not argv[0].startswith("--") else "check"
    if command == "ledger":
        result = verdict([], ledger, today, ledger_error)      # 空列表=不看 run，只审台账
    else:
        repo = value("--repo") or subprocess.run(
            ["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            encoding="utf-8", errors="replace").stdout.strip()
        if not repo:
            _say("不知道仓库（--repo o/r 或先在仓里跑）")
            return 1
        limit = int(value("--limit", "200") or 200)
        runs, why = fetch_runs(repo, limit)
        if runs is not None and value("--days"):
            days = int(value("--days"))
            cutoff = dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)
            runs = [r for r in runs if _fresh(r, cutoff)]
        result = verdict(runs, ledger, today, ledger_error)
    if "--json" in argv:
        print(json.dumps(result, ensure_ascii=False, indent=1))
    else:
        render(result)
    return 1 if result["red"] else 0


def _fresh(run: dict, cutoff) -> bool:
    created = str(run.get("created_at") or "")
    try:
        return dt.datetime.fromisoformat(created.replace("Z", "+00:00")) >= cutoff
    except ValueError:
        return True      # 时间读不出来就当在看，宁可多查也不放过


def self_test() -> int:
    """每条判红都要有真的会红的反证；绿例必须真跑。"""
    total, failures = 0, []

    def check(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            _say(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    today = dt.date(2026, 11, 3)
    red = {"head_sha": "a" * 40, "name": "test (macos-latest)", "conclusion": "failure", "run_attempt": 1}
    green_late = {"head_sha": "a" * 40, "name": "test (macos-latest)", "conclusion": "success",
                  "run_attempt": 2}
    clean_pair = [{"head_sha": "b" * 40, "name": "fmt", "conclusion": "success", "run_attempt": 1},
                  {"head_sha": "b" * 40, "name": "fmt", "conclusion": "success", "run_attempt": 1}]

    # 正例：没有先红后绿的组合，台账齐备 ⇒ 绿
    good_ledger = [{"test": "test (macos-latest) 的 tls 往返", "owner": "Qoder",
                    "reason": "GitHub macOS runner 抢不到，偶发无输出", "added": "2026-11-01",
                    "expires": "2026-11-30"}]
    result = verdict(clean_pair, good_ledger, today)
    check("无掩盖且台账合规判绿", result["red"], False)
    check("正例真扫到了 run", result["runs"], 2)

    # 反证 1：先红后绿（attempt>1）就是掩盖
    masked = masked_green([red, green_late])
    check("重跑掩盖被抓", len(masked), 1)
    check("掩盖项没记账 ⇒ 拦", verdict([red, green_late], [], today)["red"], True)
    check("记账之后不拦（owner/到期齐备）", verdict([red, green_late], good_ledger, today)["red"], False)

    # 反证 2：只看到"attempt>1 才绿"、没有失败痕迹 ⇒ 警告（不是拦），但也不能装看不见
    lone_rerun = [{"head_sha": "c" * 40, "name": "test (macos-latest)", "conclusion": "success",
                   "run_attempt": 3}]
    result = verdict(lone_rerun, [], today)
    check("无失败痕迹的重跑不判红", result["red"], False)
    check("但必须点出「绿是重跑拿到的」",
          any(f["kind"] == "绿是重跑拿到的（无失败痕迹）" for f in result["findings"]), True)
    check("干净一次性绿不该有任何掩盖痕迹",
          len(masked_green([{"head_sha": "d" * 40, "name": "fmt", "conclusion": "success",
                             "run_attempt": 1}])), 0)

    # 反证 1b：先 timed_out 再绿同样算掩盖（只认 "failure" 会漏掉超时/取消）
    timeout_pair = [{"head_sha": "e" * 40, "name": "test (windows-latest)",
                     "conclusion": "timed_out", "run_attempt": 1},
                    {"head_sha": "e" * 40, "name": "test (windows-latest)",
                     "conclusion": "success", "run_attempt": 2}]
    hits = masked_green(timeout_pair)
    check("超时后重跑转绿也算掩盖", [h["name"] for h in hits], ["test (windows-latest)"])
    check("而且按确凿掩盖判红（不是只警告）",
          any(f["kind"] == "重跑掩盖，且没人记账" for f in verdict(timeout_pair, [], today)["findings"]), True)
    check("日期读不出来不能编出一个日期", parse_day("2026-13-45"), None)
    check("时间戳读不出来的 run 不许被丢掉（宁可多查）",
          _fresh({"created_at": "不是时间"}, dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc)), True)

    # 反证 3：拿不到 run 列表 ⇒ 红
    result = verdict(None, [], today)
    check("查不到 run 判红", (result["red"], result["runs"]), (True, 0))    # 反证 4：台账缺字段 / 过期 / 日期坏 / 重复 / 没名字
    bad = [
        {"test": "t1", "reason": "抖", "added": "2026-11-01", "expires": "2026-11-30"},      # 缺 owner
        {"test": "t2", "owner": "o", "added": "2026-11-01", "expires": "2026-10-01"},        # 缺 reason + 过期
        {"test": "t3", "owner": "o", "reason": "r", "added": "2026-11-01", "expires": "30/11/2026"},
        {"test": "t1", "owner": "o", "reason": "r", "added": "2026-11-01", "expires": "2026-11-30"},
        {"test": "", "owner": "o", "reason": "r", "added": "2026-11-01", "expires": "2026-11-30"},
    ]
    kinds = {finding["kind"] for finding in verdict([], bad, today)["findings"]}
    check("缺 owner 被抓", "隔离项缺 owner" in kinds, True)
    check("缺 reason 被抓", "隔离项缺 reason" in kinds, True)
    check("过期被抓", "隔离已过期还在躺着" in kinds, True)
    check("日期格式坏被抓", "到期日读不出来" in kinds, True)
    check("重复登记被抓", "同一个测试重复登记" in kinds, True)
    check("没写测试名的被抓", "隔离项没写是哪个测试" in kinds, True)

    # 反证 5：台账 JSON 坏 ⇒ 判红，不许当成"没有隔离"
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "flake_quarantine.json"
        path.write_text("{这不是 JSON", encoding="utf-8")
        entries, error = load_ledger(path)
        check("坏 JSON 要带说明", (entries, error != ""), ([], True))
        check("坏 JSON 判红", verdict([], entries, today, error)["red"], True)
        path.write_text('{"不是": "数组"}', encoding="utf-8")
        check("顶层不是数组也要判红", load_ledger(path)[1] != "", True)
        check("台账不存在不算错（空台账另说）", load_ledger(pathlib.Path(tmp) / "没有.md")[1], "")

    # 反证 6：窗口内没再抖的隔离项 ⇒ 警告，不拦
    verdict_quiet = verdict(clean_pair, [{"test": "早就修好的 job", "owner": "o", "reason": "r",
                                          "added": "2026-10-01", "expires": "2026-12-01"}], today)
    check("安静项只警告不拦",
          (verdict_quiet["red"], any(f["kind"] == "隔离项窗口内没再抖" for f in verdict_quiet["findings"])),
          (False, True))

    # 反证 7：cp936 控制台下不许把闸门自己崩掉（夹具必须真带一个 GBK 放不下的字符）
    arrow = verdict([red, green_late], [], today)
    check("夹具判决里真带 ⇒", any("⇒" in f["detail"] for f in arrow["findings"]), True)
    import io
    probe = io.TextIOWrapper(io.BytesIO(), encoding="cp936", errors="strict", newline="\n")
    saved, sys.stdout = sys.stdout, probe
    try:
        render(arrow)
        rc = 0
    except UnicodeEncodeError:
        rc = -1
    finally:
        sys.stdout = saved
    check("cp936 控制台下打印判决不崩", rc, 0)

    _say(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
