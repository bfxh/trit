#!/usr/bin/env python3
"""绿即合之前先问一句：这抹绿到底跑没跑。

用法：
  python scripts/green_audit.py pr <编号> [--repo o/r] [--jobs] [--json]
  python scripts/green_audit.py --self-test

为什么需要它（社区最高频的一类"假绿"）：
  必需检查只要**没跑**，GitHub 就有三种表现让它看起来无害：
    - `skipped`：workflow 的 `paths:` / `if:` 把它跳掉了；
    - 压根没上报：非触发事件（只在 push 跑的 job，PR 上永远不出现）；
    - `action_required` / `neutral` / `cancelled`：等着人点、或中途被杀。
  这几种在 `gh pr checks` 里颜色不一样，但**都不阻塞合并按钮**——于是"CI 绿了"
  这句话其实什么都没证明。`ci_integrity.py` 管的是配置层的对账（静态），
  这个管的是**某一次具体合并的证据**（动态），两头都要堵。

判定口径（fail-closed）：
  拿不到必需检查清单 ⇒ 红；某必需上下文压根没上报 ⇒ 红；上报了但不是 pass ⇒ 红；
  对应的 Actions run 一个 job 都没执行 ⇒ 红；批准review 落在别的 commit ⇒ 单列警告。
  任何 `gh api` 调用失败 ⇒ 红（绝不把"查不到"折成"没问题"）。
"""

from __future__ import annotations

import importlib.util
import io
import json
import pathlib
import re
import subprocess
import sys

RUN_ID_RE = re.compile(r"/actions/runs/(\d+)")
# GitHub 会把超过 100 字的 job 名截断后再进保护规则；前缀匹配只在这个长度上放开。
GH_NAME_CAP = 100
PASSING = {"PASS", "SUCCESS"}
NON_BLOCKING_BUT_NOT_PROOF = {"SKIPPED", "NEUTRAL", "CANCELLED", "ACTION_REQUIRED", "STALE"}


def load_ci_integrity():
    """复用矩阵 job 名的模板匹配，避免两份实现对不上账。"""
    path = pathlib.Path(__file__).resolve().parent / "ci_integrity.py"
    spec = importlib.util.spec_from_file_location("trit_ci_integrity", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    sys.modules["trit_ci_integrity"] = module
    return module


def run(argv: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                          encoding="utf-8", errors="replace")


def api(path: str) -> tuple[bool, object]:
    """返回 (拿到数据, 内容)。拿不到就红，不许当成空集。"""
    result = run(["gh", "api", path])
    if result.returncode != 0:
        return False, result.stdout.strip()
    try:
        return True, json.loads(result.stdout or "null")
    except json.JSONDecodeError:
        return False, "不是 JSON：" + result.stdout[:200]


def context_matches(reported: str, required: str, ci) -> bool:
    if reported == required:
        return True
    if "${{" in required and ci.name_pattern(required).fullmatch(reported):
        return True
    # 截断名：规则里存的是被砍到 100 字的前缀
    if len(required) == GH_NAME_CAP and reported.startswith(required):
        return True
    return False


def find_reported(required: str, rollup: list[dict], ci) -> dict | None:
    for entry in rollup:
        if context_matches(entry.get("name") or entry.get("context") or "", required, ci):
            return entry
    return None


def run_id_of(url: str) -> str:
    match = RUN_ID_RE.search(url or "")
    return match.group(1) if match else ""


BLOCKING = {"无必需检查清单", "必需检查没上报", "必需检查没真跑过", "取不到 job 列表", "run 里 0 个 job"}


def audit(required: list[str], rollup: list[dict], jobs: dict[str, list[dict]],
          approvals: list[dict], head_sha: str, deep: bool) -> dict:
    """纯函数：数据进、结论出。CLI 只负责取数，判定全在这里（这样才测得动）。

    两档：BLOCKING 里任何一种都说明这次合并"没被验过"⇒ 红；其余（非必需的 skipped、
    批准后又有推送）算警告。把噪音判成拦截，工具第二天就被人绕过。
    """
    ci = load_ci_integrity()
    findings: list[dict] = []

    def add(kind: str, detail: str) -> None:
        findings.append({"kind": kind, "detail": detail, "blocking": kind in BLOCKING})

    if not required:
        add("无必需检查清单",
            "拿不到分支保护的必需检查（规则可能来自 ruleset，或 token 权限不足）——"
            "无法证明这次合并被检查过，判红。")
    by_name: dict[str, dict] = {}
    for entry in rollup:
        by_name[entry.get("name") or entry.get("context") or ""] = entry
    for context in required:
        hit = find_reported(context, rollup, ci)
        if hit is None:
            add("必需检查没上报", f"{context}：这次 head 上根本没有它"
                "（多半是 `on:`/`paths:`/`if:` 没覆盖 PR），但它挡不住合并按钮。")
            continue
        conclusion = (hit.get("conclusion") or hit.get("status") or "").upper()
        if conclusion not in PASSING:
            add("必需检查没真跑过",
                f"{context}：{conclusion or '状态为空'}——不是 pass，绿按钮不代表验过。")
            continue
        if deep:
            rid = run_id_of(hit.get("detailsUrl") or hit.get("details_url") or "")
            if rid:
                entries = jobs.get(rid)
                if entries is None:
                    add("取不到 job 列表", f"{context}：run {rid} 的 job 清单没拿到，"
                        "无法证明真执行过。")
                elif not entries:
                    add("run 里 0 个 job", f"{context}：run {rid} 一个 job 都没执行"
                        " ⇒ 成功状态是空壳。")
    for entry in by_name.values():
        conclusion = (entry.get("conclusion") or entry.get("status") or "").upper()
        if conclusion == "SKIPPED" and entry["name"] not in required:
            add("被跳过的检查（非必需）",
                f"{entry['name']}：skipped；如果以后把它加进必需检查，PR 上永远不会跑。")
    for review in approvals:
        commit_id = review.get("commit_id") or ""
        if commit_id and head_sha and commit_id != head_sha:
            add("批准的不是最终树",
                f"{review.get('state', '?')} 落在 {commit_id[:12]}，head 是 {head_sha[:12]}"
                "；批准之后又推过东西（除非开了 dismiss-stale，审批覆盖的是旧代码）。")
    return {"tool": "green-audit", "findings": findings,
            "red": any(finding["blocking"] for finding in findings),
            "blocking": sum(1 for finding in findings if finding["blocking"]),
            "required": len(required), "reported": len(rollup)}


def collect(repo: str, number: str, deep: bool) -> tuple[dict | None, str]:
    ok, pr = api(f"repos/{repo}/pulls/{number}")
    if not ok:
        return None, f"取不到 PR：{pr}"
    head_sha = (pr.get("head") or {}).get("sha") or ""
    base = (pr.get("base") or {}).get("ref") or ""
    ok, raw_required = api(f"repos/{repo}/branches/{base}/protection/required_status_checks")
    required = list(raw_required.get("contexts") or []) if ok else []
    if not ok:
        required = []
    ok, checks = api(f"repos/{repo}/commits/{head_sha}/check-runs?per_page=100")
    if not ok:
        return None, f"取不到 check-runs：{checks}"
    runs = body_list(checks)
    if not runs and isinstance(checks, dict) and checks.get("total_count"):
        return None, f"check-runs 返回形状不认识（total_count={checks['total_count']} 但没解析出条目）"
    rollup: list[dict] = []
    for item in runs:
        rollup.append({"name": item.get("name") or "",
                       "conclusion": item.get("conclusion") or item.get("status") or "",
                       "status": item.get("status") or "",
                       "detailsUrl": item.get("html_url") or ""})
    ok, statuses = api(f"repos/{repo}/commits/{head_sha}/status")
    if ok:
        for state in body_list(statuses):
            rollup.append({"name": state.get("context") or "", "conclusion": state.get("state") or "",
                           "status": state.get("state") or "", "detailsUrl": state.get("target_url") or ""})
    jobs: dict[str, list[dict]] = {}
    if deep:
        for entry in rollup:
            rid = run_id_of(entry.get("detailsUrl") or "")
            if rid and rid not in jobs:
                ok, body = api(f"repos/{repo}/actions/runs/{rid}/jobs?per_page=100")
                # 拿不到/形状不认识都记 None，让 audit 报"取不到"——不许静默当没有 job
                jobs[rid] = parse_jobs(body) if ok else None
    ok, reviews = api(f"repos/{repo}/pulls/{number}/reviews?per_page=100")
    approvals = []
    if ok:
        for review in body_list(reviews):
            if (review.get("state") or "").upper() in {"APPROVED", "CHANGES_REQUESTED"}:
                approvals.append(review)
    return {"required": required, "rollup": rollup, "jobs": jobs, "approvals": approvals,
            "head_sha": head_sha}, ""


def parse_jobs(body) -> list | None:
    """Actions 的 run→job 列表：形状不对一律返回 None（= 取不到，判红），
    绝不返回 []——空列表在这里的意思是"这个 run 真的一个 job 都没执行"。

    （第一版就是踩在这上面：把键名写成 workflow_jobs，13 条必需检查全被报成空壳。）
    """
    if isinstance(body, dict) and isinstance(body.get("jobs"), list):
        return body["jobs"]
    return None


def body_list(payload) -> list[dict]:
    """gh api 翻页时可能直接给 list，也可能给 {"items": [...]} / {"reviews": [...]}。"""
    if isinstance(payload, list):
        return payload
    if isinstance(payload, dict):
        for key in ("reviews", "items", "check_runs", "statuses"):
            if isinstance(payload.get(key), list):
                return payload[key]
    return []


def _say(text: str) -> None:
    """中文 Windows 控制台是 cp936：判决文本里有 ⇒ 这类字符时 bare print 会抛
    UnicodeEncodeError——把一次本来有效的审计崩掉，比不审计更糟。"""
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    try:
        print(text)
    except UnicodeEncodeError:
        print(text.encode(encoding, "replace").decode(encoding, "replace"))


def emit(verdict: dict) -> None:
    """人读输出（--json 之外的那条路）。单独成函数，才能被自测用假控制台钉住。"""
    warnings = len(verdict["findings"]) - verdict["blocking"]
    _say(f"必需 {verdict['required']} 条｜上报 {verdict['reported']} 条｜拦 {verdict['blocking']} 条"
         f"｜警告 {warnings} 条")
    for finding in verdict["findings"]:
        _say(f"  [{'拦' if finding['blocking'] else '警'}|{finding['kind']}] {finding['detail']}")
    _say("结论：" + ("红（必需检查里有没跑/跑空的）" if verdict["red"]
                   else "绿（每条必需检查都有执行证据）"))


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    if argv[0] != "pr":
        print(f"未知命令: {argv[0]}\n\n{__doc__}")
        return 2
    number = argv[1] if len(argv) > 1 else ""
    if not re.fullmatch(r"\d+", number or ""):
        print("用法：green_audit.py pr <PR 编号>（编号必须是数字）")
        return 2
    repo = ""
    for index, arg in enumerate(argv):
        if arg == "--repo" and index + 1 < len(argv):
            repo = argv[index + 1]
    if not repo:
        outcome = run(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"])
        repo = outcome.stdout.strip() if outcome.returncode == 0 else ""
    if not repo:
        print("不知道仓库（--repo o/r 或先在仓里跑）")
        return 1
    data, error = collect(repo, number, "--jobs" in argv or "--deep" in argv)
    if data is None:
        print(f"红：{error}")
        return 1
    verdict = audit(data["required"], data["rollup"], data["jobs"], data["approvals"],
                    data["head_sha"], bool(data["jobs"]))
    if "--json" in argv:
        print(json.dumps(verdict, ensure_ascii=False, indent=1))
    else:
        emit(verdict)
    return 1 if verdict["red"] else 0


def self_test() -> int:
    """每条判红都要有反证；正例必须真跑（rc=0 不算证据）。"""
    total, failures = 0, []
    jobs = {"9": [{"id": 1}], "0": []}

    def check(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    good = [{"name": "build (ubuntu-latest)", "conclusion": "success", "detailsUrl": "x/actions/runs/9/job/1"}]
    # 正例：必需检查都在，都 pass，run 里有 job
    verdict = audit(["build (ubuntu-latest)"], good, jobs, [], "abc", True)
    check("证据齐全时判绿", verdict["red"], False)

    # 反证 1：skipped 的必需检查
    skipped = [{"name": "build (ubuntu-latest)", "conclusion": "skipped", "detailsUrl": "x/actions/runs/9/job/1"}]
    check("skipped 的必需检查判红",
          audit(["build (ubuntu-latest)"], skipped, jobs, [], "abc", True)["red"], True)

    # 反证 2：必需检查压根没上报
    check("没上报的必需检查判红",
          audit(["deploy"], good, jobs, [], "abc", False)["red"], True)

    # 反证 3：必需清单取不到（空集不许当"没要求"）
    check("空必需清单判红", audit([], good, {}, [], "abc", False)["red"], True)

    # 反证 4：run 里 0 个 job —— 状态是空壳
    hollow = [{"name": "build (ubuntu-latest)", "conclusion": "success", "detailsUrl": "x/actions/runs/0/job/1"}]
    check("0 job 的 run 判红", audit(["build (ubuntu-latest)"], hollow, jobs, [], "abc", True)["red"], True)

    # 反证 5：job 清单没取到 ≠ 通过，也 ≠ "0 个 job"（两类要分开点名）
    found = audit(["build (ubuntu-latest)"], hollow, {}, [], "abc", True)["findings"]
    check("取不到 job 清单判红", any(f["blocking"] for f in found), True)
    check("取不到 job 清单点的是\"取不到\"（不许混成 0 job）",
          any(f["kind"] == "取不到 job 列表" for f in found), True)

    # 反证 6：neutral / action_required / cancelled 都不算证据
    for state in ("neutral", "action_required", "cancelled", "stale"):
        entry = [{"name": "build (ubuntu-latest)", "conclusion": state, "detailsUrl": "x/actions/runs/9/job/1"}]
        check(f"{state} 不算通过", audit(["build (ubuntu-latest)"], entry, jobs, [], "abc", True)["red"], True)

    # 反证 7：批准落在旧 commit（警告，不拦——否则每个 PR 都红，工具会被绕过）
    stale_approval = [{"state": "APPROVED", "commit_id": "old"}]
    verdict = audit(["build (ubuntu-latest)"], good, jobs, stale_approval, "abc", True)
    check("批准的不是最终树被点出",
          any(f["kind"] == "批准的不是最终树" for f in verdict["findings"]), True)
    check("批准后推送只警告不判红", verdict["red"], False)

    # 反证 8：矩阵模板写进必需检查也要能对上
    templated = [{"name": "fmt + clippy + test (windows-latest)", "conclusion": "success", "detailsUrl": ""}]
    check("矩阵模板能匹配",
          audit(["fmt + clippy + test (${{ matrix.os }})"], templated, {}, [], "abc", False)["red"], False)

    # 反证 9：100 字截断名按前缀匹配，但不许用短名前缀蒙混
    long_req = "a" * GH_NAME_CAP
    check("截断名按前缀算匹配",
          audit([long_req], [{"name": long_req + " (x)", "conclusion": "success", "detailsUrl": ""}], {},
                [], "abc", False)["red"], False)
    check("没被截断的短名不许前缀蒙混",
          audit(["build"], [{"name": "build-extra", "conclusion": "success", "detailsUrl": ""}], {},
                [], "abc", False)["red"], True)

    # 反证 10：非必需的 skipped 也要单列（今天是可选，明天加进必需就永远不跑）
    extra = good + [{"name": "nightly", "conclusion": "skipped", "detailsUrl": ""}]
    verdict = audit(["build (ubuntu-latest)"], extra, jobs, [], "abc", True)
    check("非必需 skipped 单列",
          any(f["kind"] == "被跳过的检查（非必需）" for f in verdict["findings"]), True)
    check("非必需 skipped 不判红", verdict["red"], False)

    # 反证 11：body_list 认三种返回形状
    check("body_list 认裸 list", len(body_list([{"a": 1}])), 1)
    check("body_list 认 items", len(body_list({"items": [{"a": 1}]})), 1)
    check("body_list 认不出的形状给空", body_list({"weird": 1}), [])

    # 反证 11b：jobs 列表解析——真·0 job 和"取不到"必须分开（第一版混了就是一片假红）
    check("parse_jobs 认 jobs 键", parse_jobs({"jobs": [{"id": 1}]}), [{"id": 1}])
    check("parse_jobs 真空列表算 0 job", parse_jobs({"jobs": []}), [])
    check("parse_jobs 不认错误的键名（workflow_jobs→取不到）", parse_jobs({"workflow_jobs": [{"id": 1}]}), None)
    check("parse_jobs 非 dict 算取不到", parse_jobs("boom"), None)

    # 反证 12：中文 Windows 控制台（cp936）不许把一次有效审计崩掉——
    # 判决文本里有 ⇒（GBK 放不下），bare print 会抛 UnicodeEncodeError。
    arrow_verdict = audit(["build (ubuntu-latest)"], hollow, jobs, stale_approval, "abc", True)
    check("夹具里确实有一条带 ⇒ 的判决",
          any("⇒" in f["detail"] for f in arrow_verdict["findings"]), True)
    probe = io.TextIOWrapper(io.BytesIO(), encoding="cp936", errors="strict", newline="\n")
    saved_stdout, sys.stdout = sys.stdout, probe
    try:
        emit(arrow_verdict)
        emit_rc = 0
    except UnicodeEncodeError:
        emit_rc = -1
    finally:
        sys.stdout = saved_stdout
    check("cp936 控制台下打印判决不崩", emit_rc, 0)

    if failures:
        for line in failures:
            print(line)
        print(f"self-test: {total - len(failures)}/{total} passed")
        return 1
    print(f"self-test: {total}/{total} passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
