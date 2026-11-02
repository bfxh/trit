#!/usr/bin/env python3
"""密钥/变量接线对账：workflow 引用的名字 vs 仓库真配过的名字（只比名字，永远读不到值）。

和 `secret_scan.py` 的分工：那个找**泄露的值**，这个找**接错的线**。CI 红的一个常见原因
不是密钥泄露，而是 `${{ secrets.NEU_TOKEN }}` 拼错了，或者 job 声明 `environment: staging`
而那个名字只配在 `production` 里——要等发布跑到那一步才炸。

判定口径：
  - 拦：引用了、清单里没有、且没有任何声明能解释（外部清单 / 环境）；
  - 拦：疑似拼错——引用的名字和已配的名字只差一两个字符（改名没同步的形状）；
  - 拦：名字只配在**别的环境**里，而引用它的 job 声明的是另一个环境（或没声明）；
  - 拦：取不到清单。取不到＝证明不了，不许"跳过就绿"；
  - 拦：外部声明行没写理由（无理由的豁免等于把门拆了）；
  - 警：配了没人引用（可能给 org 级 / 手动 dispatch 用，不能判红）；
  - 警：`secrets[inputs.x]`、`toJSON(secrets)` 这类动态引用——静态看不见名字，数出来公示，
        不假装覆盖到了。

用法：
  python scripts/secret_wiring.py                       # 静态面 + 线上清单（要 gh）
  python scripts/secret_wiring.py --offline             # 只看静态面（无网络/无权限时用）
  python scripts/secret_wiring.py --provided dump.json  # 用离线导出的清单
  python scripts/secret_wiring.py --self-test
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOWS = ".github/workflows"
EXTERNAL_FILE = ".github/secrets-external.txt"
# GitHub 自己注入，不会出现在 secrets 列表里
BUILTIN = {"GITHUB_TOKEN"}

EXPR = re.compile(r"\$\{\{(.*?)\}\}", re.S)
NAMED = re.compile(r"(?:^|[^.\w])(secrets|vars)\.([A-Za-z0-9_]+)")
INDEXED = re.compile(r"(?:^|[^.\w])(secrets|vars)\[")
MARSHAL = re.compile(r"(?:toJSON|fromJSON)\s*\([^)]*\b(secrets|vars)\b", re.I)
TOP_KEY = re.compile(r"^([A-Za-z0-9_-]+):(.*)$")
JOB_ID = re.compile(r"^  ([A-Za-z0-9_.-]+):\s*(?:#.*)?$")
JOB_KEY = re.compile(r"^    ([A-Za-z0-9_.-]+):\s*(.*)$")
NESTED_KEY = re.compile(r"^\s+([A-Za-z0-9_.-]+):\s*(.*)$")
NO_REASON = "没写理由"
CANNOT_PROVE = "证明不了"


def _say(text: str) -> None:
    """中文 Windows（cp936）控制台上打印带箭头/emoji 的判定会炸——炸了就等于门没跑。"""
    try:
        print(text)
    except UnicodeEncodeError:
        sys.stdout.buffer.write((text + "\n").encode("utf-8", "replace"))


def strip_comment(line: str) -> str:
    """抠掉 YAML 注释：注释里的引用不生效，把它算成引用就是假红。"""
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


def expr_refs(expression: str) -> tuple[list[tuple[str, str]], list[str]]:
    """一个 `${{ ... }}` 里：能静态点名的引用 + 点不出名字的动态引用描述。"""
    named = [(kind, name) for kind, name in NAMED.findall(expression)]
    dynamic = []
    for kind in INDEXED.findall(expression):
        dynamic.append(f"{kind} 下标动态取名")
    for kind in MARSHAL.findall(expression):
        dynamic.append(f"{kind} 整个上下文被序列化，名字看不见")
    return named, dynamic


def parse_workflow(text: str, workflow: str) -> tuple[list[dict], list[dict]]:
    """抠出每个引用点：哪个文件第几行、名字、落在哪个 job（及其 environment），或 workflow 顶层。"""
    uses: list[dict] = []
    dynamic: list[dict] = []
    in_jobs = False
    job: dict | None = None
    pending_env_name = False
    for number, raw in enumerate(text.splitlines(), 1):
        code = strip_comment(raw)
        if not code.strip():
            continue
        top = TOP_KEY.match(code)
        if top:
            in_jobs = top.group(1) == "jobs"
            job = None
            pending_env_name = False
        if in_jobs:
            head = JOB_ID.match(code)
            if head:
                # 每碰到一个 2 缩进的键就是新 job：否则第一个 job 的 environment 会
                # 被后面所有 job 继承，把"只在 staging 有的密钥"看成"prod 也有"（假绿）
                job = {"id": head.group(1), "environment": None}
                pending_env_name = False
            else:
                key = JOB_KEY.match(code)
                if key and job is not None:
                    name, value = key.group(1), key.group(2).strip()
                    if name == "environment":
                        pending_env_name = not value
                        if value:
                            job["environment"] = value.strip("'\"")
                    else:
                        pending_env_name = False
                elif pending_env_name and job is not None:
                    nested = NESTED_KEY.match(code)
                    if nested and nested.group(1) == "name" and nested.group(2).strip():
                        job["environment"] = nested.group(2).strip("'\"")
                        pending_env_name = False
        for expression in EXPR.findall(code):
            named, dynamics = expr_refs(expression)
            for kind, name in named:
                uses.append({"workflow": workflow, "line": number, "kind": kind, "name": name,
                             "job": job["id"] if job else None,
                             "environment": job["environment"] if job else None})
            for why in dynamics:
                dynamic.append({"workflow": workflow, "line": number, "why": why,
                                "text": expression.strip()[:80]})
    return uses, dynamic


def collect_uses(root: pathlib.Path) -> tuple[list[dict], list[dict]]:
    uses: list[dict] = []
    dynamic: list[dict] = []
    directory = root / WORKFLOWS
    if not directory.is_dir():
        return uses, dynamic
    for path in sorted(list(directory.glob("*.yml")) + list(directory.glob("*.yaml"))):
        one_uses, one_dynamic = parse_workflow(path.read_text(encoding="utf-8"), path.name)
        uses.extend(one_uses)
        dynamic.extend(one_dynamic)
    return uses, dynamic


def read_external(root: pathlib.Path) -> tuple[dict[str, str], list[str]]:
    """`.github/secrets-external.txt`：一行一个"仓外注入（org 级 / 手动 dispatch）"的名字 + 理由。"""
    path = root / EXTERNAL_FILE
    if not path.is_file():
        return {}, []
    declared: dict[str, str] = {}
    problems: list[str] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        parts = text.split(None, 1)
        reason = parts[1].lstrip("# ").strip() if len(parts) > 1 else ""
        declared[parts[0]] = reason
        if not reason:
            problems.append(f"{EXTERNAL_FILE} 第 {number} 行 {parts[0]} {NO_REASON}"
                            "（无理由的豁免等于把这道门拆了）")
    return declared, problems


def distance(left: str, right: str) -> int:
    if left == right:
        return 0
    previous = list(range(len(right) + 1))
    for i, ca in enumerate(left, 1):
        current = [i]
        for j, cb in enumerate(right, 1):
            current.append(min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (ca != cb)))
        previous = current
    return previous[-1]


def near_miss(name: str, pool: list[str]) -> str | None:
    """拼错的形状：只差一两个字符，或只是大小写不同。距离按名字长度收，避免把两个不同用途的密钥凑成一对。"""
    best: str | None = None
    best_score = None
    for other in pool:
        if other == name:
            continue
        score = distance(name.lower(), other.lower())
        if score == 0:
            # 只有大小写不同：这是改名没同步最强的形状，直接锁定
            return other
        limit = 1 if max(len(name), len(other)) < 12 else 2
        if score > limit:
            continue
        if best_score is None or score < best_score:
            best, best_score = other, score
    return best


def reconcile(uses: list[dict], provided: dict, declared: dict[str, str]) -> tuple[list[str], list[str]]:
    """纯函数：引用点 + 清单 ⇒（拦，警）。CI 里跑的就是这条路径，夹具能喂任意形状的清单。"""
    blocking: list[str] = []
    warnings: list[str] = []
    repo_secrets = list(provided.get("secrets", []))
    repo_vars = list(provided.get("variables", []))
    pools = {"secrets": repo_secrets, "vars": repo_vars}
    envs = provided.get("environments", {})
    envs_readable = provided.get("environments_readable", True)
    seen: set[tuple[str, int, str, str]] = set()
    referenced: set[str] = set()

    for use in uses:
        key = (use["workflow"], use["line"], use["kind"], use["name"])
        if use["name"] in BUILTIN or key in seen:
            continue
        seen.add(key)
        referenced.add(use["name"])
        where = f"{use['workflow']}:{use['line']}"
        job = use["job"] or "workflow 顶层"
        pool = pools[use["kind"]]
        label = f"{use['kind']}.{use['name']}"
        if use["name"] in pool:
            continue
        if use["name"] in declared:
            continue
        typo = near_miss(use["name"], pool)
        if typo:
            blocking.append(f"{where}（job {job}）引用 {label}，仓里配的是 {use['kind']}.{typo}"
                            " ⇒ 疑似改名没同步（几乎一定是打字错）")
            continue
        env_name = use["environment"]
        if env_name:
            if not envs_readable:
                blocking.append(f"{where}（job {job}，environment {env_name}）引用 {label}："
                                f"仓级清单没有，环境清单又读不到 ⇒ {CANNOT_PROVE}")
                continue
            if use["name"] in list(envs.get(env_name, {}).get(_kind_key(use["kind"]), [])):
                continue
            elsewhere = sorted(env for env, kinds in envs.items()
                               if use["name"] in list(kinds.get(_kind_key(use["kind"]), [])))
            if elsewhere:
                blocking.append(f"{where}（job {job}，environment {env_name}）引用 {label}，"
                                f"只配在环境 {elsewhere} ⇒ 这个 job 运行时拿不到")
                continue
        elif not envs_readable:
            blocking.append(f"{where}（job {job}）引用 {label}：仓级清单没有，而环境清单读不到"
                            f" ⇒ {CANNOT_PROVE}（要么配到仓级，要么让 job 声明环境）")
            continue
        blocking.append(f"{where}（job {job}）引用 {label} ⇒ 运行时必炸："
                        "仓里没配、也没在 " + EXTERNAL_FILE + " 里声明为仓外注入")

    for kind, pool in (("secrets", repo_secrets), ("vars", repo_vars)):
        unused = sorted(name for name in pool if name not in referenced)
        if unused:
            warnings.append(f"配了但 workflow 没引用（可能给 org 级 / 手动触发在用）：{unused}"
                            f"｜kind={kind}")
    stale = sorted(name for name, reason in declared.items()
                   if name in set(repo_secrets) | set(repo_vars))
    if stale:
        warnings.append(f"{EXTERNAL_FILE} 里声明成仓外的名字其实仓里就有，可以删掉该行：{stale}")
    unused_declared = sorted(name for name in declared if name not in referenced)
    if unused_declared:
        warnings.append(f"{EXTERNAL_FILE} 里声明了但 workflow 没引用（可能是过期豁免）：{unused_declared}")
    return blocking, warnings


def _kind_key(kind: str) -> str:
    return "secrets" if kind == "secrets" else "vars"


def audit(root: pathlib.Path, provided: dict | None) -> tuple[list[str], list[str], dict]:
    if provided is None:
        return ([f"取不到密钥/变量清单 ⇒ 判红（不许跳过就绿）；"
                 "检查 gh 登录态，或给 job 加 `permissions: contents: read, actions: read`"], [],
                {"uses": 0, "dynamic": 0})
    uses, dynamic = collect_uses(root)
    declared, format_problems = read_external(root)
    blocking, warnings = reconcile(uses, provided, declared)
    blocking = format_problems + blocking
    for workflow, line, why in sorted({(d["workflow"], d["line"], d["why"]) for d in dynamic}):
        warnings.append(f"动态引用（{why}）静态看不见名字，本门不覆盖：{workflow}:{line}")
    return blocking, warnings, {"uses": len(uses), "dynamic": len(dynamic)}


def gh_json(api_path: str) -> object:
    result = subprocess.run(["gh", "api", api_path], stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, encoding="utf-8", errors="replace")
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def names(payload: object, field: str) -> list[str] | None:
    if not isinstance(payload, dict):
        return None
    return [item.get("name", "") for item in payload.get(field, []) if item.get("name")]


def fetch_provided(repo: str) -> dict | None:
    """线上取清单：仓级 secrets/variables + 每个环境的同名清单（只有名字）。"""
    secrets = names(gh_json(f"repos/{repo}/actions/secrets?per_page=100"), "secrets")
    variables = names(gh_json(f"repos/{repo}/actions/variables?per_page=100"), "variables")
    if secrets is None or variables is None:
        return None
    provided = {"secrets": secrets, "variables": variables, "environments": {},
                "environments_readable": True}
    listing = gh_json(f"repos/{repo}/environments?per_page=100")
    if not isinstance(listing, dict):
        provided["environments_readable"] = False
        return provided
    for env in listing.get("environments", []):
        env_name = env.get("name")
        if not env_name:
            continue
        env_secrets = names(gh_json(f"repos/{repo}/environments/{env_name}/secrets?per_page=100"), "secrets")
        env_vars = names(gh_json(f"repos/{repo}/environments/{env_name}/variables?per_page=100"), "variables")
        if env_secrets is None or env_vars is None:
            provided["environments_readable"] = False
            break
        provided["environments"][env_name] = {"secrets": env_secrets, "vars": env_vars}
    return provided


def ex(name: str, kind: str = "secrets") -> str:
    """夹具里造一个真实的表达式引用（避免源码里出现花括号串把自测搞糊）。"""
    return "${{" + f" {kind}.{name} " + "}}"


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root = ROOT
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1])
    provided = None
    if "--provided" in argv:
        raw = pathlib.Path(argv[argv.index("--provided") + 1])
        provided = json.loads((raw if raw.is_absolute() else root / raw).read_text(encoding="utf-8"))
    elif "--offline" not in argv:
        if "--repo" in argv:
            repo_name = argv[argv.index("--repo") + 1]
        else:
            result = subprocess.run(["gh", "repo", "view", "--json", "nameWithOwner",
                                     "-q", ".nameWithOwner"],
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    text=True, encoding="utf-8", errors="replace")
            repo_name = result.stdout.strip()
        if not repo_name:
            _say("取不到仓库名（不在 git 仓里？）⇒ 判红；可用 --repo o/r 或 --offline")
            return 1
        provided = fetch_provided(repo_name)
    if provided is None and "--offline" not in argv:
        blocking, warnings, stats = audit(root, None)
    elif "--offline" in argv:
        blocking, warnings, stats = audit(root, {"secrets": [], "variables": [], "environments": {},
                                                 "environments_readable": True})
        # 离线模式没有清单：格式类问题照判，引用缺口降级成"待线上核对"并公示
        blocking = [b for b in blocking if "没写理由" in b]
        stats["deferred"] = True
    else:
        blocking, warnings, stats = audit(root, provided)

    if "--offline" in argv:
        _say(f"离线（不核对线上清单）｜引用点 {stats['uses']}｜动态引用 {stats['dynamic']}｜"
             f"拦 {len(blocking)}｜警 {len(warnings)}")
    else:
        _say(f"引用点 {stats['uses']}｜动态引用 {stats['dynamic']}｜拦 {len(blocking)}｜警 {len(warnings)}")
    for line in blocking:
        _say(f"  [拦] {line}")
    for line in warnings:
        _say(f"  [警] {line}")
    if blocking:
        _say("结论：红（密钥/变量接线对不上，CI 会跑到那一步才炸）")
        return 1
    _say("结论：绿（能证明的部分都对上了；动态引用已公示）")
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

    provided = {"secrets": ["CARGO_REGISTRY_TOKEN", "DEPLOY_KEY"],
                "variables": ["RELEASE_CHANNEL"],
                "environments": {}, "environments_readable": True}

    def repo(files: dict[str, str], root: pathlib.Path) -> None:
        directory = root / WORKFLOWS
        directory.mkdir(parents=True, exist_ok=True)
        for name, text in files.items():
            (directory / name).write_text(text, encoding="utf-8")

    def refs_yml(ref: str) -> str:
        return "on: pull_request\njobs:\n  build:\n    steps:\n      - run: echo " + ref + "\n"

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)

        repo({"ci.yml": refs_yml(ex("CARGO_REGISTRY_TOKEN"))}, root)
        check("接线正确不判红", audit(root, provided)[0], [])

        repo({"ci.yml": refs_yml(ex("CARGO_REGISTRYY_TOKEN"))}, root)
        blocking, _, _ = audit(root, provided)
        check("少一个字母 ⇒ 点名疑似改名", any("疑似改名" in b for b in blocking), True)
        check("疑似改名的判红点名两个名字",
              any("CARGO_REGISTRYY_TOKEN" in b and "CARGO_REGISTRY_TOKEN" in b for b in blocking), True)

        repo({"ci.yml": refs_yml(ex("NPM_TOKEN"))}, root)
        blocking, _, _ = audit(root, provided)
        check("引用了没配 ⇒ 拦", any("仓里没配" in b for b in blocking), True)
        check("判红文案带判决性箭头（cp936 夹具要打印它）", any("⇒" in b for b in blocking), True)

        repo({"ci.yml": refs_yml(ex("GITHUB_TOKEN"))}, root)
        check("GitHub 自带的 token 不判红", audit(root, provided)[0], [])

        commented = ("on: pull_request\njobs:\n  build:\n    steps:\n"
                     "      # 整行注释里提过 " + ex("OBSOLETE_LINE") + "\n"
                     "      - run: echo hello  # 行尾注释里也提过 " + ex("OBSOLETE_TRAILING") + "\n")
        repo({"ci.yml": commented}, root)
        blocking, _, _ = audit(root, provided)
        check("整行/行尾注释里的引用都不算（假红防线）", [b for b in blocking if "OBSOLETE" in b], [])
        check("注释行不产生引用点", audit(root, provided)[2]["uses"], 0)

        (root / EXTERNAL_FILE).write_text("SOMETHING_ORG_LEVEL\n", encoding="utf-8")
        repo({"ci.yml": refs_yml(ex("SOMETHING_ORG_LEVEL"))}, root)
        blocking, _, _ = audit(root, provided)
        check("外部声明没写理由 ⇒ 拦", any("没写理由" in b for b in blocking), True)
        (root / EXTERNAL_FILE).write_text("SOMETHING_ORG_LEVEL  # org 级，发布脚本注入\n",
                                          encoding="utf-8")
        check("有理由的外部声明 ⇒ 放行", audit(root, provided)[0], [])

        staging = "on: pull_request\njobs:\n  deploy:\n    environment: staging\n" \
                  "    steps:\n      - run: echo " + ex("API_TOKEN") + "\n"
        repo({"ci.yml": staging}, root)
        other_env = dict(provided, environments={"production": {"secrets": ["API_TOKEN"], "vars": []}})
        blocking, _, _ = audit(root, other_env)
        check("名字只在别的环境里 ⇒ 拦，且点名两个环境",
              any("只配在环境" in b and "production" in b and "staging" in b for b in blocking), True)
        same_env = dict(provided, environments={"staging": {"secrets": ["API_TOKEN"], "vars": []}})
        check("环境对上 ⇒ 放行", audit(root, same_env)[0], [])

        # 真踩到的 bug：job 归属不切换 ⇒ 第二个 job 继承了第一个的 environment（假绿）
        two_jobs = ("on: pull_request\njobs:\n  first:\n    environment: staging\n"
                    "    steps:\n      - run: echo " + ex("API_TOKEN") + "\n"
                    "  second:\n    steps:\n      - run: echo " + ex("API_TOKEN") + "\n")
        repo({"ci.yml": two_jobs}, root)
        blocking, _, _ = audit(root, same_env)
        check("第二个 job 不继承第一个的环境 ⇒ 判红并点名 second",
              any("second" in b and "API_TOKEN" in b for b in blocking), True)
        uses_list, _ = parse_workflow(two_jobs, "ci.yml")
        check("两个 job 各自归属", [(u["job"], u["environment"]) for u in uses_list],
              [("first", "staging"), ("second", None)])

        mapping_env = "on: pull_request\njobs:\n  build:\n    environment:\n      name: prod\n" \
                      "      url: https://example.invalid\n    steps:\n      - run: echo " \
                      + ex("PROD_ONLY") + "\n"
        repo({"ci.yml": mapping_env}, root)
        prod_env = dict(provided, environments={"prod": {"secrets": ["PROD_ONLY"], "vars": []}})
        check("environment 写成映射也认得", audit(root, prod_env)[0], [])

        blind = dict(provided, environments={"production": {}}, environments_readable=False)
        repo({"ci.yml": refs_yml(ex("MYSTERY_TOKEN"))}, root)
        blocking, _, _ = audit(root, blind)
        check("读不到环境清单 ⇒ 证明不了就判红", any("证明不了" in b for b in blocking), True)
        repo({"ci.yml": "on: pull_request\njobs:\n  build:\n    environment: prod\n"
                         "    steps:\n      - run: echo " + ex("MYSTERY_TOKEN") + "\n"}, root)
        blocking, _, _ = audit(root, blind)
        check("声明了环境但环境清单读不到 ⇒ 也算证明不了", any("证明不了" in b for b in blocking), True)

        both = ("on: pull_request\nenv:\n  KEY: " + ex("DEPLOY_KEY") + "\njobs:\n  build:\n"
                "    steps:\n      - run: echo " + ex("CARGO_REGISTRY_TOKEN") + "\n")
        repo({"ci.yml": both}, root)
        _, warnings, stats = audit(root, provided)
        check("workflow 顶层 env 也算引用点", stats["uses"], 2)
        check("未引用 ⇒ 只警不拦", audit(root, provided)[0], [])
        check("配了没人用 ⇒ 警", any("没引用" in w for w in warnings), True)

        dyn = ("on: pull_request\njobs:\n  build:\n    steps:\n"
               "      - run: echo ${{" + " secrets[matrix.name] " + "}}\n"
               "      - run: echo ${{" + " toJSON(secrets) " + "}}\n")
        repo({"ci.yml": dyn}, root)
        _, warnings, stats = audit(root, provided)
        check("动态引用计数", stats["dynamic"], 2)
        check("动态引用公示（不装作覆盖）",
              (any("下标动态取名" in w for w in warnings), any("序列化" in w for w in warnings)),
              (True, True))

        reusable = ("on: pull_request\njobs:\n  build:\n    uses: ./.github/workflows/x.yml\n"
                    "    secrets:\n      CALLER: " + ex("MISSING_ONE") + "\n"
                    "  other:\n    steps:\n      - run: echo " + ex("CARGO_REGISTRY_TOKEN") + "\n")
        repo({"ci.yml": reusable}, root)
        blocking, _, _ = audit(root, provided)
        check("reusable workflow 的 secrets 映射也查", any("MISSING_ONE" in b for b in blocking), True)

        repo({"ci.yml": refs_yml(ex("RELEASE_CHANNEL", "vars"))}, root)
        uses, _ = parse_workflow("on: push\njobs:\n  a:\n    steps:\n      - run: echo "
                                 + ex("RELEASE_CHANNEL", "vars") + "\n", "x.yml")
        check("vars 引用同样对账", [(u["kind"], u["name"]) for u in uses], [("vars", "RELEASE_CHANNEL")])
        check("vars 名字对了 ⇒ 不判红", audit(root, provided)[0], [])

        with tempfile.TemporaryDirectory() as bare:
            empty_blocking, empty_warnings, empty_stats = audit(pathlib.Path(bare), provided)
            check("没有 workflows 目录 ⇒ 不炸且不判红", (empty_blocking, empty_stats), ([], {"uses": 0, "dynamic": 0}))
            check("空仓里配着的密钥只报未引用（警）", any("没引用" in w for w in empty_warnings), True)
        check("清单读不到 ⇒ 判红（不许跳过）", audit(root, None)[0] != [], True)

        check("距离尺：差一个字母", distance("ABC_TOKEN", "ABCD_TOKEN"), 1)
        check("距离尺：完全不同用途的密钥不相近",
              near_miss("SLACK_WEBHOOK_URL", ["AWS_ACCESS_KEY_ID", "GPG_KEY"]), None)
        check("大小写不同也算拼错", near_miss("deploy_key", ["DEPLOY_KEY"]), "DEPLOY_KEY")
        uses_list, _ = parse_workflow("on: push\njobs:\n  a:\n    env:\n      X: 1\n"
                                      "    steps:\n      - run: echo " + ex("T") + "\n", "y.yml")
        check("job 的 env 块不会顶掉 job 归属", uses_list[0]["job"], "a")
        uses_dup, _ = parse_workflow("on: push\njobs:\n  a:\n    steps:\n"
                                     "      - run: echo " + ex("T") + " " + ex("T") + "\n", "z.yml")
        check("同行同名引用只记一次", len(uses_dup), 2)

        # 退出码面：门必须真的拦（把 main 的 `return 1` 改成 `return 0` 要靠这两条抓到）
        dump = root / "provided.json"
        dump.write_text(json.dumps({"secrets": ["DEPLOY_KEY"], "variables": [],
                                    "environments": {}, "environments_readable": True}),
                        encoding="utf-8")
        repo({"ci.yml": refs_yml(ex("NPM_TOKEN"))}, root)
        check("判红时退出码是 1（门真的拦）", main(["--root", str(root), "--provided", str(dump)]), 1)
        repo({"ci.yml": refs_yml(ex("DEPLOY_KEY"))}, root)
        check("接线对上时退出码是 0", main(["--root", str(root), "--provided", str(dump)]), 0)

    print(f"secret_wiring 自测：{passed} 过 / {failed} 红")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
