# AGENT-METRICS：agent_verify 回执指标面

> 定位：给在本仓观察智能体编排的人读。它回答：一堆 `agent_verify` 回执里，到底
> 通过/失败/拒绝/挂起各多少、卡在哪一步、耗时和声明 token 花在哪、不同 agent/lang/
> profile 的差异是什么。
> 边界：只读回执，不跑 agent_verify、不替代 `bench_orchestration.py`、不把低通过率
> 变成拒绝；token 只统计调用方声明的值，不测量模型侧用量。
> 状态：**已跑通**（`agent_metrics.py -SelfTest` **178/178**；AR1 已接入
> `tools/selftest.ps1`）。

## 1. 为什么要有这一层

`agent_verify` 每次运行都写回执，但回执是单次审计物，不是运行面：

- 一个目录里 100 条回执，`passed / command failed / refused by policy /
  suspended (budget)` 的分布要手数；
- 拒绝卡在 `preflight` 还是 `policy`，决定了该修环境还是改命令；
- 回执里的 `steps_seconds`、`run_seconds`、`wall_s`、`cost` 分散在 full/compact
  两种形状和 `volatile` 段里，直接 `jq` 容易读错；
- 坏 JSON、delta 回执、缺 verdict/steps/run 的文件如果被静默跳过，指标就会
  看起来比真实数据更干净。

## 2. 接口

```powershell
python tools/agent_metrics.py -Receipts <file|dir|glob|comma-list> `
  [-Out metrics.json] [-Strict] [-Quiet]
python tools/agent_metrics.py -SelfTest
```

- `-Receipts`：单个文件、目录、glob，或逗号分隔的混合列表。
- `-Out`：写 JSON 指标对象；不写则只打印摘要。
- `-Strict`：坏 JSON、缺 verdict/steps/run 时 exit 1；默认只计数、不改变退出码。
- `-Quiet`：只留错误/退出码，不打印摘要。
- full receipt 和 `--compact` receipt 都读；delta 回执（`against`/`delta` 信封）
  单独计为 `delta_skipped`，不当成完整 run。

## 3. 指标对象

顶层 14 段：`receipts / verdicts / quality / exit_codes / steps / run / changes /
budget / cost / claims / timing / groups / integrity / schema`。

- `receipts`：文件数、valid、full、compact、malformed、delta、字节分位、compact 比例。
- `verdicts`：原始 verdict 计数/比例；归一成
  `passed / failed / refused / suspended / dry_run / rejected / other` 七类。
- `quality`：runs、各类计数、success/failure/refusal/suspension rate。
- `exit_codes`：回执顶层 exit 的分布。
- `steps`：总步数、每步 `count/exits/verdicts/failures/seconds`、first_failure 分布、
  全部 step 秒数的 min/max/mean/p50/p95/p99、top-5 慢步骤。
- `run`：命令总数、exit 分布、non-zero、命令秒数分位。
- `changes`：有变更的 run 数、变更行总数、每 run 变更数分位。
- `budget`：带 budget 的 run 数、declared/remaining 总量/均值、gate 分布。
- `cost`：声明 token 总量/均值/分位、overhead 总量/均值、记账失败数。
- `claims`：带 claim/handoff 的 run 数、verdict 分布、claim 路径总数。
- `timing`：wall 秒数分位与 total、step/run 秒数合计。
- `groups`：按 `agent_id` / `lang` / `profile` 的 runs、七类计数、success_rate、
  wall_mean。
- `integrity`：malformed、delta_skipped、missing_verdict、missing_steps、
  missing_run、strict_ok。

没有把 output tail、命令原文、claim 文本、root 路径写进指标——指标只回答分布，
需要现场证据时回原文回执。

## 4. 证据与读数

一次临时夹具的 6 条**真实** `agent_verify` 回执（dry-run、proven pass、command
failed、refused by policy、suspended budget、compact pass）：

```
files=6 valid=6 full=5 compact=1 malformed=0 delta=0
runs=6 passed=2 failed=1 refused=1 suspended=1 dry_run=1
steps=46 run=3
wall p50=0.04s p95=0.0875s p99=0.0895s
first_failure: policy=1 run=1
groups/python: runs=6 success_rate=0.3333
```

这份指标对象展开是 **289 个叶子字段**。以上数字是临时夹具一次观测，不是 SLO，
也不是对真实任务成功率的估计；换任务、机器和命令都会变。

- `agent_metrics.py -SelfTest` **178/178**：
  - 60 条合成回执（full/compact 各 30，passed/failed/refused/suspended/dry-run/
    rejected 各 10），逐条检查 class 与 mode（120 项）；
  - 5 条坏回执 + 1 条 delta 信封，检查 malformed/delta/missing 计数；
  - 6 条真实 `agent_verify` 场景（dry-run / pass / command failed / policy refuse /
    budget suspend / compact），检查 rc、回执 class、聚合计数和 first_failure；
  - 聚合断言覆盖 verdict/exit、step/run 秒数、成本/预算、claims、字节分位、
    per-agent/lang/profile、strict。
- `tools/selftest.ps1` 的 AR1 调同一自测，要求 `passed == total` 且总数 ≥100；
  本分支全量 selftest **677 passed / 0 failed / 0 skipped**。

## 5. 边界（诚实）

- 回执里的 `cost.tokens` 是调用方 `--spent-token` 的声明值，不是模型侧计量；
  没有声明就是 `null`，不会被补成 0 后当成“没花钱”。
- `wall_s` / `steps_seconds` / `run_seconds` 是工具层墙钟；并发、缓存、机器负载都会
  改变它，分位只描述这批回执，不是 SLO。
- `-Strict` 只拒绝坏输入，不因为 `success_rate` 低就判失败——观测面不是闸。
- delta 回执不是完整 run，计入 `delta_skipped`，不参与 verdict/成本统计。
- 指标对象不含 raw output/命令原文/root 路径；需要复核时回原回执。
