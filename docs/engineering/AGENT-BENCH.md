# AGENT-BENCH：真实任务基准与外部基准谱系

> 定位：回答“智能体编排到底能不能在真实任务上跑通”，而不是只看回执字段。
> 本文记录外部基准的覆盖谱系、本仓的本地切片、任务格式、读数与边界。
> 状态：`agent_bench.py -SelfTest` **331/331**；`-Run` **28 tasks / 53 runs**；
> `-Repeat 2` **106 runs / 28 tasks × 2 全过**；AR1 + AS1 已接入 `tools/selftest.ps1`。

## 1. 为什么不是再加一批合成回执

`agent_metrics.py` 能统计回执，但回执是编排链的输出；它不能证明：

- 真实代码任务里的 gold patch 能不能被 `agent_verify` 执行并通过；
- 行为错误的 negative patch 会不会被测试和编排链一起拦住；
- policy、budget、dry-run、compact 这些非 happy-path 分支是否真的按语义走；
- 同一个任务重复跑，结果是否稳定（pass^k），而不是一次偶然通过。

`agent_bench` 把这四件事做成可重跑的真实任务夹具：每个 patch 任务都有
初始代码、gold patch、negative patch、真实 `unittest` 命令；每个任务在
隔离临时目录里跑 `agent_verify`，读回执判定 class/exit/run/step/落盘内容。

## 2. 外部基准谱系与本仓切片

只选与“代码/终端/工作流/可靠性”直接相关的基准；网页、GUI、DB、ML 训练类
明确不做复现，只保留为覆盖边界。

| 基准 | 它测什么 | 本仓对应的本地切片 | 不复现的部分 |
|---|---|---|---|
| SWE-bench / Verified（arXiv:2310.06770；swebench.com/verified） | 真实 GitHub issue + PR patch + 测试 | 19 个 Python bugfix/feature/refactor/etc. 夹具，gold/negative 双 patch | GitHub issue 语料、真实 PR、LLM 生成 patch |
| Terminal-Bench（arXiv:2601.11868；OpenReview a7Qa4CcHak） | 真实终端任务、Docker 环境、人工验证测试 | CLI/off-by-one/安全/policy/回执场景，真实命令执行 | Docker 容器、89 个官方任务、人工评分 |
| AgentBench（arXiv:2308.03688） | 8 个交互环境：OS/DB/KG/游戏/网页/家务 | 代码、CSV、CLI、policy 四类本地任务 | OS/DB/KG/网页/游戏环境 |
| τ-bench（arXiv:2406.12045；taubench.com） | 模拟用户 + API + 领域 policy + pass^k | policy 拒绝、budget 挂起、`-Repeat` 的 pass^2 | 模拟用户、retail/airline/telecom API 数据库 |
| TheAgentCompany（arXiv:2412.14161） | 模拟公司里的浏览/写代码/沟通/长程任务 | ledger/config 这类工作流小任务 | 公司内网、同事模拟、175 个官方任务 |
| OSWorld / WebArena / WorkArena（arXiv:2404.07972 / 2307.13854 / 2403.07718） | 真实 OS、网页、企业 SaaS 的 GUI 操作 | 仅保留为边界，不做 GUI | 浏览器、桌面、ServiceNow |
| GAIA（arXiv:2311.12983） | 多模态、网页、工具使用的通用助手问题 | 仅保留为边界 | 网页检索、多模态、私有答案 |
| MLE-bench / RE-Bench（arXiv:2410.07095 / 2411.15114） | Kaggle ML 工程、AI R&D 长程任务 | 仅保留为边界 | 训练、GPU、Kaggle 环境 |
| Spider2-V（spider2-v.github.io） | 数据科学/工程工作流，CLI+GUI | `py-csv-average` 一个 CSV 聚合切片 | 20 个企业应用、GUI、494 任务 |
| Agent 评测综述（arXiv:2507.21504） | 一致性/鲁棒性/成本/安全等维度 | 把 pass^k、negative、拒绝、预算纳入本地报告 | 不把综述当本地结论 |

参考之外，不把“公开基准数字”抄进本仓；本地夹具是**独立实现**，不是任何
上游数据集的子集。

## 3. 本地任务矩阵

任务文件 `testdata/agent-bench/tasks.json`（schema 1），28 个任务：

| 组 | 任务 |
|---|---|
| bugfix (10) | `py-sum-offbyone` `py-json-schema-email` `py-multifile-key` `py-date-inclusive` `py-ledger-balance` `py-dedupe-order` `py-retry-backoff` `py-priority-max` `py-word-count-punct` `py-compact-sum` |
| feature (3) | `py-cli-upper-flag` `py-person-fullname` `py-config-fallback` |
| refactor/data/security (3) | `py-normalize-refactor` `py-csv-average` `py-path-traversal` |
| text/finance/cli (3) | `py-text-wrap` `py-money-cents` `py-cli-exitcode` |
| multi-language (6) | Node: `js-array-sum` `js-slug-normalize`；Go: `go-max` `go-dedupe-order`；Rust: `rs-sum-offbyone` `rs-normalize-lower` |
| 非 happy-path (3) | `sec-policy-refuse` `agent-budget-suspend` `agent-dry-run` |

每个 patch 任务包含：

```json
{
  "id": "py-sum-offbyone",
  "kind": "patch",
  "files": {"calc.py": "...", "test_calc.py": "..."},
  "gold": {"calc.py": "..."},
  "negative": {"calc.py": "..."},
  "changed": ["calc.py"],
  "negative_changed": ["calc.py"],
  "commands": ["{py} -m unittest -q"],
  "expect": {"gold": "passed", "negative": "failed"}
}
```

- `{py}` 在执行前替换成当前解释器并加引号。
- `kind=compact` 使用同一套 gold/negative，但额外传 `--compact`，同时检查回执 mode。
- `kind=policy/budget/dry_run` 不要求 gold patch，只检查对应分支的 verdict、run 缺失/预算字段。
- 任务文件不含绝对路径、`..`、符号链接；执行时每个 variant 单独临时 root。

## 4. 接口

```powershell
python tools/agent_bench.py -List
python tools/agent_bench.py -SelfTest
python tools/agent_bench.py -Run -Out bench.json
python tools/agent_bench.py -Run -Candidates E:\tmp\solutions -Out bench.json
python tools/agent_bench.py -Run -Repeat 2 -Out bench.json
```

- 默认跑 gold/reference + negative；`-Candidates` 时跑
  `<task-id>/<相对路径>` 的候选文件树，不再用 gold。
- `-Task` / `-Category` 过滤；`-Repeat N` 生成 pass^k 稳定性读数。
- `-Strict` 在任一 unexpected result 时 exit 1；默认只报告不阻断。
- `-Out` 写 per-run 结果 + `agent_metrics` 聚合指标；`-Keep` 保留临时 root。
- `-Candidates` 接收“文件树”，不解析 git diff；候选必须把所有 changed 文件放到
  `<task-id>/` 下。

## 5. 读数

全量 `tools/selftest.ps1`：**677 passed / 0 failed / 0 skipped**（含 AR1 + AS1）。

一次 `-Run`（本机 Windows，Python 3.13 + Node 22 + Go 1.27 + Rust 1.99，`-Timeout 120`）：

```
tasks=28 runs=53
reference=25/25 passed
negative=25/25 detected, false_accept=0
special=3/3 passed
wall=32.66s
agent_metrics: receipts=53, steps=516, wall_p95=1.526s
```

一次 `-Repeat 2`：

```
tasks=28 runs=106
reference=50/50 passed
negative=50/50 detected
special=6/6 passed
repeat_all_ok=28/28
wall=64.21s
agent_metrics: receipts=106, steps=1032
```

自测：

```
agent-bench self-test: 331/331
```

331 项覆盖：suite 结构（任务数/类别数/语言数）、gold/negative 双跑、每个 run 的
rc/class/mode/run presence/nonzero/patch 落盘、policy refused+no run、budget
declared=0+no run、dry-run no run、紧凑回执 mode、metrics receipt 数量/坏回执为
0/strict_ok。

## 6. 边界（诚实）

- **不是 LLM 能力分数**。gold/negative 是人写的参考补丁；19/19 证明“编排链能执行并被
  测试拦住”，不证明任何模型能写出这些补丁。
- **不是上游数据集复现**。没有下载 SWE-bench/Terminal-Bench 的实例，任务与代码均为
  本地独立夹具；不把本仓读数与公开 leaderboard 横比。
- **只有本地四种运行时**。Python/Node/Go/Rust 的测试命令在临时目录真实执行；没有
  浏览器、桌面、数据库、ServiceNow、Docker、GPU，也没有 Java/C#/TypeScript 工程。
  OSWorld/WebArena/WorkArena/GAIA/MLE-bench 只在覆盖表里，不在执行面。
- **没有模拟用户**。τ-bench 的 user simulator 和多轮 policy 对话没有复现；这里只有
  policy 静态拒绝与预算/拒绝分支。
- **negative 不是安全攻击集**。negative 是行为错误补丁，不是漏洞利用或红队语料。
- **pass^k 只证明确定性夹具稳定**。22 个任务 × 2 次全过，不等于真实任务分布上的
  可靠性；真实任务仍要按任务族、语言、环境分别统计。
- **`agent_metrics` 的成功率不能直接读**。benchmark 报告里 negative 是故意失败的，
  所以整体 `success_rate` 会被 negative 拉低；看 `reference`、`negative`、`special`
  三个分组，而不是看总体 success_rate。
