# 智能体稳定性/抖动探针（flaky / nondeterministic / jittery）

## 为什么加这一层

“这次跑过了”经常被智能体直接读成“这个东西是稳的”。但开发项目里至少有四类
不同的事会被同一个 exit 0 掩盖：

- 同一个测试这次过、下次挂（flaky test）；
- 每次都过，但每次输出不同（nondeterministic）；
- 每次输出一样，但耗时时快时慢（jitter）；
- 每次都失败，但失败码/输出还在变（不是 flaky，而是更不稳定的失败）。

只跑一次，或者只记录一次退出码，无法把“稳定通过”和“碰巧通过”分开。
`tools/stability_probe.py` 把这件事做成一个**重复执行、可复算、unknown 不放行**的
观测器：清单声明要探什么，工具按命令重复运行，报告保留每次的退出码、耗时和
输出 SHA-256，最后按固定规则分类。

它不判断“这条测试是否在语义上测对了需求”--那是
[`AGENT-ALIGNMENT.md`](AGENT-ALIGNMENT.md) 的 v3 run-bound 证据要回答的事。
它只回答：**同一条命令重复跑，结果稳不稳。**

## 清单格式 `stability/1`

```json
{
  "schema": "stability/1",
  "repeat": 3,
  "warmup": 1,
  "timeout_s": 120,
  "max_duration_ratio": 3.0,
  "require_identical_output": true,
  "commands": [
    {
      "id": "T1",
      "command": "python -m pytest tests/test_a.py -q",
      "repeat": 5,
      "timeout_s": 60,
      "max_duration_ratio": 3.0
    },
    {
      "id": "T2",
      "command": "cargo test -p trit-orch --test ledger_cross_process",
      "repeat": 3
    }
  ]
}
```

顶层字段：

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `schema` | 必填 | 固定 `stability/1` |
| `repeat` | `3` | 每条命令重复次数，`2..50`；`repeat=1` 直接 unknown |
| `warmup` | `0` | 正式测量前的预热次数，`0..5`；预热退出码参与 flaky 判定，耗时/输出指纹不计入 |
| `timeout_s` | `120` | 单次运行超时秒数，`0.1..3600` |
| `max_duration_ratio` | `3.0` | 单次运行 max/min 耗时比阈值，`1..100` |
| `require_identical_output` | `true` | 是否要求重复运行的 stdout+stderr 指纹一致 |
| `commands` | 必填 | 非空数组；每项有唯一 `id` 和非空 `command` |

每条命令可覆盖 `repeat` / `warmup` / `timeout_s` / `max_duration_ratio` /
`require_identical_output`；CLI 的 `--repeat`、`--warmup`、`--timeout-s`、
`--max-duration-ratio`、`--lenient-output` 会覆盖整份清单。命令用 shell 执行，
工作目录固定为 `--root`；清单路径必须留在 root 内，绝对路径/`..` 逃逸按
`manifest-outside-root` 拒绝。

`warmup` 是为了把“首次编译/冷启动”从耗时判定里拿掉，不是为了让失败消失：
预热运行如果超时或启动失败，整体仍是 `unknown`；预热运行如果非 0、正式测量
却全过，整体仍是 `flaky`。预热只影响耗时和输出指纹的统计口径。

## 分类规则

每条命令独立分类；整体报告取最严重的状态。

| 分类 | 观测 | 含义 |
| --- | --- | --- |
| `stable-pass` | 全部 exit 0；输出指纹一致（或显式允许漂移）；耗时比未超阈值 | 这批重复运行稳定通过 |
| `stable-fail` | 全部非 0；输出指纹一致 | 确定性失败；不是 flaky，但也不能当通过 |
| `flaky` | 同一命令既有 exit 0 又有非 0 | 最常见的 flaky test / 时序抖动 |
| `nondeterministic` | 全部 exit 0，但输出指纹不同 | 通过但结果不可复现 |
| `jittery` | 全部 exit 0，输出一致，`max/min` 耗时超过阈值（且最短运行 ≥ 1 ms） | 耗时抖动 |
| `unstable-fail` | 全部非 0，但退出码或输出还在变 | 不稳定的失败，不是确定性失败 |
| `unknown` | 任一次超时或启动失败 | 无法判定；按 fail-closed 处理 |

分类看全部退出码（含预热）：预热失败 + 正式测量通过仍是 `flaky`；`jittery` /
`nondeterministic` 只看正式测量段的耗时和输出指纹，避免把首次编译算成抖动。

整体状态：

| 状态 | 退出码 | 条件 |
| --- | --- | --- |
| `stable` | `0` | 所有命令 `stable-pass` |
| `failed` | `1` | 存在 `stable-fail`，且没有更严重的抖动 |
| `unstable` | `1` | 存在 `flaky` / `nondeterministic` / `jittery` / `unstable-fail` |
| `unknown` | `3` | 清单缺失/坏 JSON/结构非法，或任一命令超时/启动失败 |
| 坏调用 | `2` | CLI 参数越界（如 `repeat<2`） |

`unknown` 永远不是通过：清单读不出、schema 不对、命令超时、进程起不来，都
`exit 3`。只有全部 `stable-pass` 才是 `exit 0`。

## 用法

```bash
python tools/stability_probe.py \
  --root . --manifest .agent/stability.json

python tools/stability_probe.py \
  --root . --manifest .agent/stability.json --json

python tools/stability_probe.py \
  --root . --manifest .agent/stability.json \
  --repeat 5 --warmup 1 --timeout-s 60 --out target/stability.json

python tools/stability_probe.py self-test
```

`--out` 写一份 ASCII JSON 报告；报告里每条命令记录：

- `classification`、`exit_codes`、`passes` / `failures`；
- `warmup`、`warmup_exit_codes`、`warmup_failures` / `warmup_runs`；
- `duration_ms`、`duration_min_ms` / `duration_max_ms`、`duration_ratio`；
- `distinct_outputs`、每次 run 的 `output_sha` 和 `output_bytes`；
- 每次 run 的 `timeout` / `error`（如果有）。

工具不保存原始 stdout/stderr，只保存 SHA-256 和字节数，避免把测试日志里的
敏感信息写进报告；需要看原文时，重新跑一次单条命令即可。

## 阈值与边界

- **重复次数不是证明。** `repeat=5` 全过只能说明“这 5 次没抖”，不能证明第 6
  次不会挂。要抓低频 flaky，应提高 `repeat`，或把 CI rerun 历史作为外部证据。
- **耗时比是相对量。** 默认 `max_duration_ratio=3.0`；启动开销很大的命令会把
  真实抖动稀释。对启动慢、但执行时间短的命令，应显式调低阈值或改用绝对耗时
  上限；后者本工具尚未建模。
- **预热只去掉冷启动，不证明后续稳定。** `warmup=1` 会把首次编译/缓存填充的
  耗时排除在 `duration_ratio` 之外；它不能消除后续的锁竞争、GC、磁盘抖动。
  预热运行的退出码仍计入 flaky 判定，不能被用来“洗掉”首次失败。
- **输出指纹是字节级。** CRLF/LF、时间戳、随机 ID、并发日志顺序都会让
  `distinct_outputs` 变大；这可能是真实 nondeterminism，也可能只是日志噪声。
  清单可以显式 `require_identical_output=false`，但报告仍记录
  `distinct_outputs`，不能假装输出一致。
- **超时会杀进程树。** Windows 用 `taskkill /T /F`，POSIX 用新 session +
  `killpg`；避免 shell 退出后子进程继续跑、占住临时目录。
- **命令是信任边界。** 清单里的 `command` 会被 shell 执行；接进 `agent_verify`
  时会先过 `policy.py`，但 policy 允许不等于副作用安全。只对受信任的仓库/清单
  运行，不要把外部输入直接拼进清单。报告会原样记录 `command`，不要把 token/
  密码写进命令，改用环境变量或外部凭证。
- **不自动修复。** 探针只分类和留证，不重跑直到绿、不修改测试、不判断
  “这个抖动是否可接受”；是否阻塞合并由调用方/CI 决定。

## 接入 `agent_verify`

探针已接进 `agent_verify` 链，位置在 `alignment` 之后、`journal-begin` 之前：

- 项目放了 `.agent/stability.json` 且 `settings.stability.enabled=true`（默认）时自动运行；
- `--stability <manifest>` 强制指定清单，`--no-stability` 关闭本次自动发现；
- 清单里的每条 `command` 先由同一 `profile` 的 `policy.py` 批分类，**探针执行前
  就拒绝**，清单不能绕过命令策略；
- 只有探针 `exit 0` 且 `status=stable` 才通过；`flaky` / `nondeterministic` /
  `jittery` / `stable-fail` / `unknown` 都在 `run` 之前拒绝，回执 `verdict` 为
  `refused by stability` 或 `refused by stability policy`；
- 回执 `stability` 段记录 `status`、`exit`、`commands`、`runs`、`warmup_runs`、
  各分类计数、失败命令 id 和报告路径/哈希；`target/agent-stability/stability-gate.json`
  是完整报告；
- 探针命令在 `journal-begin` 前执行，所以**必须写临时目录/被 git 忽略的路径**；
  它们不属于本次任务的 journal 变更。`policy` 通过不等于副作用安全。

独立运行仍然可用，适合在提交前手动探一条命令：

```bash
python tools/stability_probe.py \
  --root . --manifest .agent/stability.json --repeat 5 --warmup 1
```

## 与现有能力的关系

- `tools/agent_bench.py -Repeat N` 是 bench fixture 的 pass^k 稳定性读数；
  它只覆盖 bench 任务，不是通用项目命令的 flaky 探针。
- `tools/agent_verify.py` 的 `alignment-post` 证明“声明的 run 命令实际成功跑过”，
  但不重复执行，也不比较多次运行的退出码/输出/耗时。
- CI 的 rerun 历史能抓跨 job 的 flaky，但需要外部 API/日志；本工具是本地、
  离线、可复算的补充，不替代 CI 历史。

探针是 `agent_verify` 的可选默认门（有清单才开），不是无条件全量重复；没有
清单的项目付出为零。智能体若要声称“稳定”，应让回执里出现 `stability.status=stable`
和 `runs>0`，不能只引用一次 `exit 0`。

## 测试与读数

- `python tools/stability_probe.py self-test`：38/38，覆盖 stable-pass、
  stable-fail、flaky、unstable-fail、nondeterministic、jittery、warmup 去冷启动、
  warmup 失败不洗白、timeout/unknown、坏清单、root 外清单、
  `require_identical_output=false` 等分支。冷启动/预热分支直接构造运行样本喂
  `_classify`，不靠真实 sleep 计时；真实探针执行由 ST2-ST6 覆盖。
- `tools/selftest.ps1` 的 `ST1`：把探针自测接进全仓自测（当前 38 项）；
  `ST2-ST6`：稳定通过进入 run、确定性失败拒绝、flaky 拒绝、`--no-stability`
  跳过、清单命令被 policy 拒绝时不执行探针。全仓当前 **719 passed / 0 failed /
  0 skipped**（AW1-AW3 之后）。

## 不做什么

- 不把 `stable-fail` 当 flaky：确定性失败是“坏了”，不是“偶尔坏”。
- 不把 `unknown` 当通过：超时/启动失败没有结论，必须 fail-closed。
- 不用单次 exit 0 冒充稳定；也不把重复 5 次全过写成“已证明稳定”。
- 不替代语义对齐、代码审查、CI 历史分析；它只补“同一条命令重复跑是否稳”
  这一层观测。
