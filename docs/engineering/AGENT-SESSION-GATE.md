# AGENT-SESSION-GATE：会话级存储门控

> 定位：给在本仓做长周期记忆/上下文管理的人读。它回答：一段会话结束后，凭什么决定
> “这次值不值得写进记忆”，以及 `skip` 为什么不是删除、账本记什么。
> 边界：只覆盖宿主侧 `tools/session_gate.py` 的判定与 `tools/memory.py` 的写入口；
> 不覆盖 PerMem-Bench 全量复现、LLM 门控质量、Mem0/RMM 适配、个性化删除。
> 状态：**已跑通**（`session_gate.py -SelfTest` 17/17；AQ1 已接入
> `tools/selftest.ps1`，全量 **676 passed / 0 failed / 0 skipped**）。

## 1. 为什么是门控，不是换记忆系统

- PerMem-Bench（arXiv:2605.25535）的核心观察：统一静态记忆策略会把预算花在瞬时会话上，
  真正需要跨会话保留的上下文反而被挤掉；不同用户的“值得存”会话并不相同。
- 论文提出的 wrapper 是 **session-level storage gating**：每段会话后判
  `long-horizon` / `transient`；transient 跳过 memory 操作，底层记忆系统不改。
- 论文同时给了一个冷水结论：Oracle 门控收益明显，但 Greedy/Context/Structure 三类
  不完美门控只有边际收益，甚至低于 Universal；profile shift 后 Structure-aware 会崩。
  所以本仓先落**可审计的门控边界**，不把“AI 自动判断”当已解决能力。

## 2. 接口

判定端：

```powershell
python tools/session_gate.py -Sessions testdata/session-gate.jsonl -Policy greedy -Eval
python tools/session_gate.py -Sessions sessions.jsonl -Policy context -State .agent/session-gate/state.json
python tools/session_gate.py -Sessions sessions.jsonl -Policy external -Decision skip
```

- `-Sessions`：JSON 数组或 JSONL；每条至少 `user`、`session`、`turns`/`text`。
  可选 `domains`、`summary`、`facts`；`label` 只在 `-Eval`/`-Policy oracle` 使用。
- `-Policy`：
  - `greedy`：只看当前会话的显式长期/瞬时线索；
  - `context`：加最近 3 条脱敏摘要；
  - `structure`：维护 project / isolated 结构，能回填项目归属；
  - `universal`：全存；`oracle`：读夹具标签，只作评测上界；
  - `external`：消费真实模型/调度器给出的 `store|skip`，本工具不替它判断。
- `-State`：只存脱敏摘要和项目词，不落原始 transcripts；可给目录（自动加
  `state.json`）或直接给 `.json` 文件；默认 `.agent/session-gate/state.json`。
- `-Out`：决策 JSONL；`-Eval`：要求每条都有 `label`，输出 F1 与 retention proxy。

写入端：

```powershell
python tools/memory.py -Mode note -Root . -Title "..." -Evidence "..." `
  -Session s1 -Gate skip -Gate-Reason "one-off weather query"
```

- `-Gate skip`：**不写** `log.jsonl`，只往 `.agent/memory/gate.jsonl` 追加一条决策；
  旧记忆一条不删。
- `-Gate store`：正常写 entry，并把 session/gate/policy/reason 附在 entry 上。
- `-Gate` 必须与 `-Session` 同时出现；只有 session 没有门控决定会被拒绝（exit 2）。

## 3. 证据

夹具 `testdata/session-gate.jsonl`：8 条会话，2 个用户，5 条 long / 3 条 transient，
含一个“瞬时会话但命中已有项目”的 profile-shift 陷阱。

| policy | store | skip | F1 | retention proxy |
|---|---:|---:|---:|---:|
| greedy | 4 | 4 | 0.8889 | 0.8000 |
| context | 8 | 0 | 0.7692 | 1.0000 |
| structure | 5 | 3 | 0.8000 | 0.8000 |
| oracle | 5 | 3 | 1.0000 | 1.0000 |

- 这些是**夹具读数**，不是 PerMem-Bench 复现，也不是 LLM 质量结论。greedy 的漏判是
  “what should I do next” 这类无表面线索的 long 会话；context 把历史摘要当证据后全存；
  structure 命中已有项目就把 profile-shift 的瞬时会话也存了。
- `session_gate.py -SelfTest` 17/17：四种判定、oracle F1/retention、profile-shift 已知缺陷、
  external 缺失拒绝、memory skip 不写 entry、store 才写、gate 账本逐条留痕、audit 认账。
- `tools/selftest.ps1` 的 AQ1 调同一自测；全量 selftest 结果以本地跑数为准。

## 4. 边界（诚实）

- 本工具是**门控原语 + 可复算夹具**，不是“AI 个性化记忆”的成品。LLM 门控接在
  `-Policy external`；质量要用真实轨迹/评测，不靠本夹具的 F1。
- 只跳过写入，不做个性化删除/驱逐；论文也把删除留作 future work。
- `context`/`structure` 目前是确定性词面基线，不能跨语言/跨领域泛化；
  profile shift 下 structure 的崩溃是**已知缺陷**，不是待修完才准用。
- 不替代 `memory.py` 的 evidence 规则：skip 仍要求标题/证据参数通过安全检查，
  避免把跳过变成绕过 write-time guard 的后门。
