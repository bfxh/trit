#!/usr/bin/env python3
"""CI 并发与时长预算门：让"重复 run 自己挤自己"和"没有超时的 job 挂着烧额度"现形。

为什么需要它（都是托管 runner 上的真金白银）：
  - workflow 没写 `concurrency` ⇒ 同一分支连推三次会有三份完整 CI 并行排队，
    要么排队等 runner、要么互相抢；前一份被后面的顶掉时，第一份的日志还会被 GitHub 折叠。
  - job 没写 `timeout-minutes` ⇒ 挂死的 job 默认能占 **6 小时**额度；私有仓按分钟计费，
    公开仓也占着 runner 队列，别人（和几小时后的你自己）都得等。
  - 反过来也有坑：`timeout-minutes` 设得比实际耗时还紧，偶发就红——所以还要拿真实 runs 对账。

判定口径（能证明的才拦）：
  拦：workflow 没有 workflow 级 `concurrency`；
  拦：job 有 `runs-on` 却没有 `timeout-minutes`；
  拦：`timeout-minutes` 超过预算（默认 30 分钟，`--budget` 可调）；
  拦：线上模式取不到 runs 清单——"读不到"不等于"没问题"；
  警：有 `concurrency` 但 `cancel-in-progress: false`（有的发布流是故意不许取消，别判红）；
  警：真实耗时中位数 ≥ 该 job `timeout-minutes` 的 80%（快撞上限了）；
  警：窗口内同一 (workflow, ref) 出现多个 run 且有被 cancelled 的（重复提交自己挤自己）。

用法：
  python scripts/ci_budget.py                    # 静态面 + 线上 runs 对账（要 gh）
  python scripts/ci_budget.py --offline           # 只跑静态面（无网络；公示"没跟真实耗时对过账"）
  python scripts/ci_budget.py --repo o/r --days 7 --limit 200 --budget 30
  python scripts/ci_budget.py --self-test
"""

from __future__ import annotations

import datetime
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOWS = ".github/workflows"
DEFAULT_BUDGET = 30
BUDGET_FILE = ".github/ci-budget.txt"
RUNS_URL = "repos/{repo}/actions/runs?per_page={per}&created=>{since}"
CANCELLED_MARK = "cancelled"

WORKFLOW_KEY = re.compile(r"^([A-Za-z0-9_-]+):(.*)$")
JOB_ID = re.compile(r"^  ([A-Za-z0-9_.-]+):\s*(?:#.*)?$")
JOB_KEY = re.compile(r"^    ([A-Za-z0-9_.-]+):\s*(.*)$")
TIME_RE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})Z$")


def _say(text: str) -> None:
    """中文 Windows（cp936）控制台上打印带 ⇒ 的判定会炸——炸了就等于门没跑。"""
    try:
        print(text)
    except UnicodeEncodeError:
        sys.stdout.buffer.write((text + "\n").encode("utf-8", "replace"))


def strip_comment(line: str) -> str:
    if line.lstrip().startswith("#"):
        return ""
    out: list[str] = []
    quote = ""
    for index, char in enumerate(line):
        if quote:
            out.append(char)
            if char == quote:
                quote = ""
            continue
        if char in "'\"":
            quote = char
            out.append(char)
            continue
        if char == "#" and (index == 0 or line[index - 1] in " \t"):
            break
        out.append(char)
    return "".join(out)


def parse_workflow(text: str, workflow: str) -> dict:
    """按缩进抠：workflow 级 concurrency / cancel-in-progress + 每个 job 的 runs-on 与 timeout-minutes。"""
    info = {"workflow": workflow, "name": workflow, "concurrency": False,
            "cancel_in_progress": None, "jobs": [], "dynamic_matrix": False}
    in_jobs = False
    current: dict | None = None
    for raw in text.splitlines():
        line = strip_comment(raw)
        if not line.strip():
            continue
        top = WORKFLOW_KEY.match(line)
        if top and not line.startswith(" "):
            key, value = top.group(1), top.group(2).strip()
            if key == "name" and value:
                info["name"] = value.strip("'\"")
            in_jobs = key == "jobs"
            current = None
            if key == "concurrency":
                if value:
                    info["concurrency"] = value != "false"
                    info["cancel_in_progress"] = None
                else:
                    info["concurrency"] = True
            continue
        if in_jobs:
            head = JOB_ID.match(line)
            if head:
                current = {"id": head.group(1), "runs_on": False, "timeout": None,
                           "name": head.group(1)}
                info["jobs"].append(current)
                continue
            key = JOB_KEY.match(line)
            if key and current is not None:
                name, value = key.group(1), key.group(2).strip()
                if name == "runs-on":
                    current["runs_on"] = True
                elif name == "timeout-minutes" and value:
                    try:
                        current["timeout"] = int(value.split()[0])
                    except ValueError:
                        current["timeout"] = None
                        current["bad_timeout"] = value
                elif name == "name" and value:
                    current["name"] = value.strip("'\"")
        elif current is None and re.match(r"^\s+cancel-in-progress:", line):
            value = line.split(":", 1)[1].strip()
            info["cancel_in_progress"] = value == "true"
    return info


def read_overrides(root: pathlib.Path) -> tuple[dict[tuple[str, str], tuple[int, str]], list[str]]:
    """`.github/ci-budget.txt`：一行 `workflow.yml#job 分钟  # 理由`，给确实需要跑很久的 job 抬预算。

    没有理由的豁免行直接判红——无理由的豁免等于把这道门拆了，而且下一个人不敢删。
    """
    path = root / BUDGET_FILE
    if not path.is_file():
        return {}, []
    overrides: dict[tuple[str, str], tuple[int, str]] = {}
    problems: list[str] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        target, _, rest = text.partition(" ")
        if "#" not in target:
            problems.append(f"{BUDGET_FILE} 第 {number} 行形状不对（要 `文件#job 分钟`）：{text}")
            continue
        pieces = rest.split("#", 1)
        minutes = pieces[0].strip()
        reason = pieces[1].strip() if len(pieces) > 1 else ""
        workflow, _, job = target.partition("#")
        if not minutes.isdigit():
            problems.append(f"{BUDGET_FILE} 第 {number} 行的分钟数不是整数：{minutes or '(空)'}")
            continue
        if not reason:
            problems.append(f"{BUDGET_FILE} 第 {number} 行 {target} 没写理由（无理由的豁免等于把门拆了）")
            continue
        overrides[(workflow, job)] = (int(minutes), reason)
    return overrides, problems


def collect_static(root: pathlib.Path) -> list[dict]:
    directory = root / WORKFLOWS
    if not directory.is_dir():
        return []
    out = []
    for path in sorted(list(directory.glob("*.yml")) + list(directory.glob("*.yaml"))):
        out.append(parse_workflow(path.read_text(encoding="utf-8"), path.name))
    return out


def audit(static: list[dict], runs: list[dict] | None, budget: int = DEFAULT_BUDGET,
          overrides: dict[tuple[str, str], tuple[int, str]] | None = None) -> tuple[list[str], list[str], dict]:
    blocking: list[str] = []
    warnings: list[str] = []
    if overrides is None:
        # 没给清单不等于"没有豁免"：那样任何台账问题都会被静默绕过 ⇒ 判红
        return ([f"没给 {BUDGET_FILE} 的豁免清单 ⇒ 判红（读不到不等于没有）；"
                "调用方要显式传 overrides（可以为空表）"], [],
                {"workflows": len(static), "jobs": sum(len(i["jobs"]) for i in static), "runs": 0})
    for info in static:
        workflow = info["workflow"]
        if not info["concurrency"]:
            blocking.append(f"{workflow}：没有 workflow 级 `concurrency` ⇒ 同分支连推会有多份 CI 并行抢 runner")
        elif info["cancel_in_progress"] is False:
            warnings.append(f"{workflow}：有 concurrency 但 `cancel-in-progress: false`"
                            "（发布类是故意的就别改；纯 CI 类等于排队时旧 run 还跑完）")
        for job in info["jobs"]:
            if not job["runs_on"]:
                continue
            label = f"{workflow} › job `{job['id']}`"
            if job.get("bad_timeout"):
                blocking.append(f"{label}：`timeout-minutes` 不是整数（{job['bad_timeout']}）⇒ 证明不了它设了多久")
            elif job["timeout"] is None:
                blocking.append(f"{label}：有 `runs-on` 却没有 `timeout-minutes` ⇒ 挂死时默认占 6 小时")
            else:
                ceiling = budget
                granted = overrides or {}
                for (workflow_name, job_id), (minutes, _reason) in granted.items():
                    if workflow_name == workflow and job_id in (job["id"], "*"):
                        ceiling = max(ceiling, minutes)
                if job["timeout"] > ceiling:
                    blocking.append(f"{label}：`timeout-minutes: {job['timeout']}` 超预算（{ceiling} 分钟）"
                                    f"——要么拆步，要么在 {BUDGET_FILE} 里写清为什么需要这么久")

    used = {(info["workflow"], job["id"]) for info in static for job in info["jobs"]}
    stale = sorted(f"{workflow}#{job}（{minutes} 分钟，理由：{reason}）"
                   for (workflow, job), (minutes, reason) in (overrides or {}).items()
                   if job != "*" and (workflow, job) not in used)
    if stale:
        warnings.append(f"{BUDGET_FILE} 里有豁免指向不存在的 job（job 改名/删了没同步）：{stale}")
    stats = {"workflows": len(static), "jobs": sum(len(i["jobs"]) for i in static), "runs": 0}
    if runs is None:
        blocking.append("取不到 runs 清单 ⇒ 判红（不许跳过就绿）；离线请显式 `--offline` 并公示没对过账")
        return blocking, warnings, stats
    stats["runs"] = len(runs)
    durations: dict[str, list[int]] = {}
    by_ref: dict[tuple[str, str], list[dict]] = {}
    for run in runs:
        workflow = run.get("name") or run.get("workflow_id") or "?"
        head = run.get("head_branch") or "?"
        by_ref.setdefault((workflow, head), []).append(run)
        start, finish = run.get("run_started_at"), run.get("updated_at")
        seconds = duration_seconds(start, finish)
        if seconds is not None and run.get("status") == "completed":
            durations.setdefault(str(workflow), []).append(seconds)
    by_name = {info.get("name", info["workflow"]): info for info in static}
    for (workflow, head), group in sorted(by_ref.items()):
        if len(group) < 2:
            continue
        cancelled = [r for r in group if r.get("conclusion") == CANCELLED_MARK]
        if not cancelled:
            continue
        config = by_name.get(workflow)
        if config is None:
            warnings.append(f"{workflow} @ {head}：{len(group)} 个 run、{len(cancelled)} 个被取消，"
                            "但本地 workflow 里对不上这个名字 ⇒ 证明不了是谁顶掉的")
            continue
        if not config["concurrency"]:
            continue  # 已经在"缺 concurrency"那条判红了，这里不重复出声
        if config["cancel_in_progress"] is not True:
            warnings.append(f"{workflow} @ {head}：{len(group)} 个 run、{len(cancelled)} 个被取消，"
                            "但这个 workflow 没开 cancel-in-progress ⇒ 取消是外来动作（手动/规则），问一句")
    for workflow, times in sorted(durations.items()):
        if not any(info.get("name", info["workflow"]) == workflow for info in static):
            warnings.append(f"{workflow}：runs 里有这个 workflow 名，但本地 .github/workflows 对不上"
                            "（改名/删文件没同步）⇒ 时长与预算没法对账")
    for workflow, times in sorted(durations.items()):
        median = sorted(times)[len(times) // 2]
        # run.name 是 **workflow 的显示名**（不是 job 名）：拿该 workflow 里最长的 job timeout 当上限
        ceiling = max((job["timeout"] for info in static
                       if info.get("name", info["workflow"]) == workflow
                       for job in info["jobs"] if job["timeout"]), default=None)
        if ceiling and median >= ceiling * 60 * 0.8:
            warnings.append(f"{workflow}：真实耗时中位数 {median // 60} 分钟，已到 timeout 的 "
                            f"{int(median / (ceiling * 60) * 100)}% ⇒ 偶发超时会像 flake，先加步数预算或拆 job")
    return blocking, warnings, stats


def iso_seconds(value: str | None) -> int | None:
    """`2026-11-03T10:00:00Z` -> UTC 整数秒。

    第一版是手算的（年*365 + 月*30）：跨月会算平——11-01 与 10-31 落在同一个数上，
    `--days 3` 这种窗口筛选在月末就不可信。改用 stdlib 日历换算；形状不对一律 None。
    """
    if not value or not TIME_RE.match(value):
        return None
    try:
        parsed = datetime.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError:
        return None
    return int(parsed.replace(tzinfo=datetime.timezone.utc).timestamp())


def duration_seconds(start: str | None, finish: str | None) -> int | None:
    a, b = iso_seconds(start), iso_seconds(finish)
    if a is None or b is None or b < a:
        return None
    return b - a


def gh_runs(repo: str, days: int, limit: int) -> list[dict] | None:
    """拉近 `days` 天的 runs（只要名字/分支/结论/时间），取不到就返回 None 让门判红。"""
    result = subprocess.run(
        ["gh", "api", f"repos/{repo}/actions/runs?per_page=100", "--paginate",
         "--jq", '[.workflow_runs[] | {name, head_branch, status, conclusion,'
                 ' run_started_at, updated_at}]'],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        encoding="utf-8", errors="replace")
    if result.returncode != 0:
        return None
    runs: list[dict] = []
    # `--paginate --jq` 每页输出一行紧凑 JSON 数组：逐行解析，不能整块 loads（今天真踩到：
    # 三页拼在一起 json.loads 直接失败，门报"取不到清单"红——判红是对的方向，但读数会一直空）
    for line in result.stdout.splitlines():
        if not line.strip():
            continue
        try:
            data = json.loads(line)
        except json.JSONDecodeError:
            return None
        items = data.get("workflow_runs", data) if isinstance(data, dict) else data
        if isinstance(items, list):
            runs.extend(item for item in items if isinstance(item, dict))
    cutoff = int(datetime.datetime.now(datetime.timezone.utc).timestamp()) - days * 86400
    fresh = [run for run in runs
             if (iso_seconds(run.get("run_started_at")) or 0) >= cutoff]
    return fresh[:limit] if fresh else []


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root = ROOT
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1])
    budget = int(argv[argv.index("--budget") + 1]) if "--budget" in argv else DEFAULT_BUDGET
    days = int(argv[argv.index("--days") + 1]) if "--days" in argv else 7
    limit = int(argv[argv.index("--limit") + 1]) if "--limit" in argv else 200
    static = collect_static(root)
    overrides, ledger_problems = read_overrides(root)
    if "--offline" in argv:
        blocking, warnings, stats = audit(static, [], overrides=overrides)
        blocking = ledger_problems + blocking
        warnings = warnings + ["离线：没跟真实 runs 对过账（只证明了静态面）"]
        mode = "离线"
    else:
        repo = argv[argv.index("--repo") + 1] if "--repo" in argv else None
        if not repo:
            result = subprocess.run(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"],
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    text=True, encoding="utf-8", errors="replace")
            repo = result.stdout.strip()
        runs = gh_runs(repo, days, limit) if repo else None
        blocking, warnings, stats = audit(static, runs, overrides=overrides)
        blocking = ledger_problems + blocking
        mode = f"线上 {repo}（近 {days} 天）"
    _say(f"{mode}｜workflow {stats['workflows']} 个｜job {stats['jobs']} 个｜runs {stats['runs']}｜"
         f"拦 {len(blocking)}｜警 {len(warnings)}")
    for line in blocking:
        _say(f"  [拦] {line}")
    for line in warnings:
        _say(f"  [警] {line}")
    if blocking:
        _say("结论：红（并发/时长预算对不上，runner 额度与排队时间都在暗处花掉）")
        return 1
    _say("结论：绿（能证明的部分都对上了；真实耗时对账见上）")
    return 0


def self_test() -> int:
    import tempfile

    passed, failed = 0, 0

    def check(label: str, got: object, want: object) -> None:
        nonlocal passed, failed
        if got == want:
            passed += 1
        else:
            failed += 1
            print(f"  FAIL {label}: 期望 {want!r}，实得 {got!r}")

    GOOD = ("on: pull_request\n"
            "concurrency:\n"
            "  group: ci-${{ github.ref }}\n"
            "  cancel-in-progress: true  # 新提交顶掉旧 run\n"
            "jobs:\n"
            "  build:\n"
            "    runs-on: ubuntu-latest\n"
            "    timeout-minutes: 20\n"
            "    steps:\n"
            "      - run: echo ok\n")
    # 用"整行剔除"造缺 concurrency 的夹具：跟着 GOOD 的措辞做字符串替换会静默失效（今天踩过）
    NO_CONCURRENCY = "".join(line for line in GOOD.splitlines(keepends=True)
                             if not line.startswith(("concurrency:", "  group: ci-", "  cancel-in-progress:")))
    NO_TIMEOUT = GOOD.replace("    timeout-minutes: 20\n", "")
    OVER_BUDGET = GOOD.replace("timeout-minutes: 20", "timeout-minutes: 90")
    NO_CANCEL = GOOD.replace("cancel-in-progress: true", "cancel-in-progress: false")

    def make_repo(root: pathlib.Path, files: dict[str, str]) -> None:
        directory = root / WORKFLOWS
        directory.mkdir(parents=True, exist_ok=True)
        for name, content in files.items():
            (directory / name).write_text(content, encoding="utf-8")

    def one(text: str, runs: list[dict] | None = [], overrides: dict | None = None) -> tuple:
        # 默认显式传"空豁免清单"：不传就该判红（见下面"没给清单"那条断言）
        return audit([parse_workflow(text, "ci.yml")], runs,
                     overrides={} if overrides is None else overrides)

    blocking, warnings, stats = one(GOOD)
    check("配好的不判红", blocking, [])
    check("行尾注释不被当成值的一部分（否则 cancel-in-progress 会被读成没开）", warnings, [])
    check("统计口径", (stats["workflows"], stats["jobs"]), (1, 1))

    check("缺 concurrency 判红", any("concurrency" in b for b in one(NO_CONCURRENCY)[0]), True)
    check("缺 timeout 判红且点名 job",
          any("build" in b and "timeout-minutes" in b for b in one(NO_TIMEOUT)[0]), True)
    check("超预算判红", any("超预算" in b for b in one(OVER_BUDGET)[0]), True)
    check("cancel-in-progress: false 只警不拦",
          (one(NO_CANCEL)[0], any("cancel-in-progress" in w for w in one(NO_CANCEL)[1])), ([], True))
    check("runs 取不到 ⇒ 判红（不许跳过就绿）",
          any("取不到 runs" in b for b in one(GOOD, None)[0]), True)

    # 时长对账：真实中位数接近 timeout ⇒ 警
    slow = [{"name": "CI", "head_branch": "main", "status": "completed",
             "conclusion": "success", "run_started_at": "2026-11-03T10:00:00Z",
             "updated_at": "2026-11-03T10:18:00Z"}]
    info = [{"workflow": "ci.yml", "name": "CI", "concurrency": True,
              "cancel_in_progress": True,
             "jobs": [{"id": "build", "runs_on": True, "timeout": 20, "name": "build"}]}]
    blocking, warnings, _ = audit(info, slow, overrides={})
    check("耗时中位数达 timeout 的 90% ⇒ 警", any("已到 timeout" in w for w in warnings), True)
    check("这条不升级为拦", blocking, [])

    fast = [dict(slow[0], updated_at="2026-11-03T10:02:00Z")]
    check("跑得快就不警", audit(info, fast, overrides={})[1], [])
    orphan = [dict(slow[0], name="不存在的 workflow 名")]
    check("run 的 workflow 名对不上本地文件 ⇒ 时长条要点名而不是静默跳过",
          any("对不上" in w for w in audit(info, orphan, overrides={})[1]), True)

    dup = [dict(slow[0], conclusion="cancelled"), dict(slow[0])]
    # 分档后的口径：配了 cancel-in-progress 的重复取消是设计行为（不刷噪音），
    # 只有"没开取消却被取消"才出声——见下面两条断言。
    check("没给豁免清单 ⇒ 判红（读不到不等于没有）",
          any("没给" in b and "豁免清单" in b for b in audit(info, dup)[0]), True)
    check("耗时那条会给出百分比（单位算错就露馅）", any("90%" in w for w in warnings), True)

    check("时长尺：非法时间戳返回 None", duration_seconds("not-a-time", "2026-11-03T10:00:00Z"), None)
    check("时长尺：结束早于开始返回 None",
          duration_seconds("2026-11-03T10:05:00Z", "2026-11-03T10:00:00Z"), None)
    check("时长尺：正常算出差",
          duration_seconds("2026-11-03T10:00:00Z", "2026-11-03T10:18:00Z"), 18 * 60)

    # 反证：注释掉的 concurrency 不算配了（否则整道门对这种写法空转）
    commented = GOOD.replace("concurrency:", "# concurrency:")
    check("注释里的 concurrency 不算数", any("concurrency" in b for b in one(commented)[0]), True)

    # 反证：timeout 写成模板/非整数 ⇒ 证明不了就判红
    weird = GOOD.replace("timeout-minutes: 20", "timeout-minutes: ${{ vars.T }}")
    check("timeout 不是整数时判红而不是放行", any("证明不了" in b for b in one(weird)[0]), True)

    # 反证：没有 runs-on 的 job（reusable/远端调用）不该因为没 timeout 被点
    no_runner = "on: push\nconcurrency: x\njobs:\n  call:\n    uses: o/r/.github/workflows/x.yml@main\n"
    check("没有 runs-on 的 job 不要求 timeout",
          [b for b in audit([parse_workflow(no_runner, "x.yml")], [], overrides={})[0] if "timeout" in b],
          [])
    # 上面这条必须"有牙齿"：把 runs-on 守卫摘掉后，这条就得判出 timeout 缺口
    no_runner_static = [parse_workflow(no_runner, "x.yml")]
    check("该 job 确实在静态面里（不是空表混过）",
          (len(no_runner_static[0]["jobs"]), no_runner_static[0]["jobs"][0]["runs_on"]), (1, False))

    # 反证：workflow 级 `concurrency: 字符串`（一行写法）也算配了。
    # 这副夹具必须是整行新写的：靠对 GOOD 做字符串替换会静默失配（今天踩过——
    # 措辞一改替换就落空，变异体当然"存活"，其实是自测根本没跑那条路）。
    oneline = ("on: pull_request\n"
               "concurrency: ci-${{ github.ref }}\n"
               "jobs:\n"
               "  build:\n"
               "    runs-on: ubuntu-latest\n"
               "    timeout-minutes: 20\n")
    check("一行写法的夹具本身确实只有标量一行 concurrency（没有 group 块）",
          oneline.count("group:"), 0)
    check("一行写法的 concurrency 也算配了", [b for b in one(oneline)[0] if "concurrency" in b], [])

    # 豁免台账：有理由才抬预算；无理由判红；指向不存在的 job 要出声
    with tempfile.TemporaryDirectory() as tmp:
        ledger_root = pathlib.Path(tmp)
        make_repo(ledger_root, {"mutation.yml": OVER_BUDGET})
        check("没有台账文件 ⇒ 空清单且不报错", read_overrides(ledger_root), ({}, []))
        (ledger_root / BUDGET_FILE).write_text("mutation.yml#build 120\n", encoding="utf-8")
        check("无理由的豁免 ⇒ 点名", bool(read_overrides(ledger_root)[1]), True)
        (ledger_root / BUDGET_FILE).write_text(
            "mutation.yml#build 120  # 变异全量一轮要两小时\n", encoding="utf-8")
        declared, issues = read_overrides(ledger_root)
        check("有理由的豁免读得出来",
              (declared, issues),
              ({("mutation.yml", "build"): (120, "变异全量一轮要两小时")}, []))
        ledger_static = collect_static(ledger_root)
        check("豁免生效 ⇒ 不再判超预算", audit(ledger_static, [], overrides=declared)[0], [])
        (ledger_root / BUDGET_FILE).write_text("mutation.yml#ghost 120  # job 早改名了\n",
                                               encoding="utf-8")
        ghost, _ = read_overrides(ledger_root)
        make_repo(ledger_root, {"mutation.yml": GOOD})   # GOOD 的 timeout 在预算内，才看得出"只警不拦"
        tidy_static = collect_static(ledger_root)
        check("豁免指向不存在的 job ⇒ 警告而不是拦",
              (audit(tidy_static, [], overrides=ghost)[0],
               any("不存在的 job" in w for w in audit(tidy_static, [], overrides=ghost)[1])),
              ([], True))
        (ledger_root / BUDGET_FILE).write_text("mutation.yml 120  # 少写了 #job\n", encoding="utf-8")
        check("台账行形状不对 ⇒ 点名（不静默跳过）",
              any("形状不对" in problem for problem in read_overrides(ledger_root)[1]), True)
        (ledger_root / BUDGET_FILE).write_text("mutation.yml#build abc  # 分钟数写错\n",
                                               encoding="utf-8")
        check("分钟数不是整数 ⇒ 点名",
              any("不是整数" in problem for problem in read_overrides(ledger_root)[1]), True)
        (ledger_root / BUDGET_FILE).write_text("mutation.yml#* 90  # 整个 workflow 都要跑很久\n",
                                               encoding="utf-8")
        wildcard, _ = read_overrides(ledger_root)
        check("通配 job（`*`）也抬预算", audit(ledger_static, [], overrides=wildcard)[0], [])

    # 退出码面：门必须真的拦（走 main 的 rc，不是只测纯函数）
    with tempfile.TemporaryDirectory() as rc_tmp:
        rc_root = pathlib.Path(rc_tmp)
        make_repo(rc_root, {"bad.yml": NO_CONCURRENCY})
        check("判红时退出码是 1（门真的拦）", main(["--root", str(rc_root), "--offline"]), 1)
        make_repo(rc_root, {"good.yml": GOOD})
        import os
        os.remove(rc_root / WORKFLOWS / "bad.yml")
        check("接线对上时退出码是 0", main(["--root", str(rc_root), "--offline"]), 0)
        (rc_root / BUDGET_FILE).write_text("good.yml#build 120\n", encoding="utf-8")
        check("台账无理由 ⇒ 退出码也是 1（无理由的豁免拦得住）",
              main(["--root", str(rc_root), "--offline"]), 1)

    check("跨月不会算平（11-01 必须晚于 10-31 最后一秒）",
          iso_seconds("2026-11-01T00:00:00Z") > iso_seconds("2026-10-31T23:59:59Z"), True)
    check("形状对但日期非法 ⇒ None 而不是硬算", iso_seconds("2026-13-40T00:00:00Z"), None)
    check("带时区偏移的不当 UTC 认（宁可少看不误判）", iso_seconds("2026-11-03T10:00:00+08:00"), None)

    healthy = [{"workflow": "ci.yml", "name": "CI", "concurrency": True,
                "cancel_in_progress": True, "jobs": [], "dynamic_matrix": False}]
    check("配了 cancel-in-progress 的分支重复 run 不再刷噪音",
          audit(healthy, dup, overrides={})[1], [])
    no_cancel_cfg = [dict(healthy[0], cancel_in_progress=False)]
    check("没开取消却被取消 ⇒ 要问一句并给计数",
          any("外来动作" in w and "2 个 run" in w and "1 个被取消" in w
              for w in audit(no_cancel_cfg, dup, overrides={})[1]), True)
    unknown = [dict(healthy[0], name="别的名字")]
    check("run 名字对不上本地 workflow ⇒ 点名（不静默）",
          any("对不上" in w for w in audit(unknown, dup, overrides={})[1]), True)
    no_concurrency = [dict(healthy[0], concurrency=False)]
    check("缺 concurrency 的重复取消不重复出声（那条已经判红）",
          (any("concurrency" in b for b in audit(no_concurrency, dup, overrides={})[0]),
           any("挤自己" in w for w in audit(no_concurrency, dup, overrides={})[1])), (True, False))

    print(f"ci_budget 自测：{passed} 过 / {failed} 红")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
