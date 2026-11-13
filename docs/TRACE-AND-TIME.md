# 轨迹与时序：可回放的编排判定史

> 副标题：三套时钟、因果边，以及“不要用一个时间戳解释一切”
> 基础：`trit-orch` 的 `TraceStep` / `TraceStore` / `replay`，`trit-schedule` 的检查点与运行时指标，`trit-immune` 的 HLC
> 日期：2026-11-13

---

## 0. 一句话

**轨迹不是日志，是判定史；时序不是一个时间戳，是三套时钟加因果边。**

只按 `recorded_at` 排序的 trace，会把“晚到的证据”误判成“后发生的事实”。这两种东西一旦混在一起，回放、审计和指标都会给出看似确定、实际错误的结论。

这份文档要回答四个问题：

1. 一条编排轨迹到底该记什么，才能从“事后看日志”变成“可重放、可对账、可追责”？
2. 事件时间、观测时间、逻辑时间分别该用在哪，为什么不能互相替代？
3. 晚到、重复、乱序、跨主机的轨迹，如何在不篡改历史的前提下正确归并？
4. 三值语义（True / False / Unknown / 冲突）在轨迹里如何留下证据，而不是只留下一个结果？

---

## 1. 现有轨迹的能力与缺口

### 1.1 已经有的

| 能力 | 落点 | 说明 |
|---|---|---|
| 单步回放输入 | `crates/trit-orch/src/shadow.rs:7` 的 `TraceStep` | 记 `revision`、`tasks`、`snapshot`、`limits`、`ledger` |
| 追加式 JSONL | `TraceStore` | 一行一步；缺文件视为空；坏行报行号，不跳过、不猜 |
| 内核重算 | `replay` | 每步重新执行 `select_with_effects`，不信历史结论 |
| 影子策略对比 | `ShadowReport` | `rejected` / `unsafe_extra` / `skipped` / 顺序差异 |
| 注入式时间 | `claim` / `receipt` / `handoff_gate` / `compaction_gate` | 时间由调用方传入，内核不读系统时钟 |
| 逻辑时钟 | `revision` / `epoch` / `fencing` / `attempt` | 计划版本、任务定义版本、写者世代、效果尝试号 |
| 运行时指标 | `crates/trit-schedule/src/runtime/types.rs:166` | started / completed / failed / deferred / preemptions / peak_usage |
| 检查点版本 | `crates/trit-schedule/src/checkpoint/mod.rs` | `CHECKPOINT_VERSION = 1` |
| 混合逻辑时钟 | `crates/trit-immune/src/hlc.rs` | `(物理毫秒, 计数, 写者)`，单调、因果、可全序 |
| 事件模型 | `crates/trit-orch/src/trace.rs` `TraceEvent` | schema、双时间、逻辑时间、actor、因果字段 |
| 单 trace 事件日志 | `TraceLog` | 按 seq 排序、id 去重、seq 冲突判红、迟到标记、缺口可见 |
| 事件折叠 | `TraceLog::fold` / `latest_step` | 完整状态事件按 seq 折叠成 `TraceStep`；缺 plan/snapshot/limits 失败关闭 |
| 事件持久化 | `crates/trit-orch/src/trace_store.rs` `TraceEventStore` | JSONL 追加/加载、schema 校验、坏行报行号、写入后同步 |
| 旧轨迹迁移 | `events_from_step` / `append_step` | `TraceStep` 拆成 plan/snapshot/limits/ledger 四条事件，可折叠回同一 step |
| 协调事件记录 | `TraceRecorder` | claim / receipt / handoff / compaction / obligation / effect 生成带 kind、correlation、seq 的事件 |
| 协调审计折叠 | `TraceLog::coordination` | 按 seq 收集 claims / receipts / handoffs / compactions / obligations / effects |
| 状态重建 | `TraceLog::state_at` / `step_at` | 折叠到指定 seq；未来事件不参与 |
| 因果追问 | `TraceLog::why` / `CausalChain` | 沿 causation 回溯；缺失引用列出，成环判红 |
| 选择回放 | `SelectionCaptured` / `replay_selections` | 记录选择 vs 重算选择，差异分类到 `ReplayReport` |
| 时间口径指标 | `TraceLog::metrics` / `TraceMetrics` | event / observation / logical 分开；在途年龄、交接延迟、写入滞后、时钟回拨 |
| 脱敏与保留 | `TraceEvent::redacted` / `redacted_with`、`TraceEventStore::redact_before` / `compact_before` | payload 哈希、策略脱敏、旧载荷压缩；事件骨架与因果保留 |
| 多主机归并 | `Hlc` / `merge_logs` | seq / 因果边决定先后，HLC 只给并发事件破平；冲突与环失败关闭 |
| 轨迹不变量 | `TraceLog::check_invariants` / `TraceInvariantReport` | seq 缺口、因果引用、effect 状态机、回执四态、fence、交接义务 |

### 1.2 缺什么

`TraceStep` 是**快照式**的，不是**事件式**的。它足够回放“这一轮选了谁”，但不足以回答“这件事为什么发生、什么时候发生、谁先谁后”。

| 缺口 | 后果 |
|---|---|
| 没有 `now` | claim / receipt / handoff / compaction 的时间判定无法回放 |
| 没有 `seq` / `trace_id` / `span_id` | 只能靠文件行号排序；跨文件、跨主机无法归并 |
| 没有 `event_time` 与 `observed_at` 的区分 | 晚到的回执会被当成“后发生的事实” |
| 没有 `causation_id` / `correlation_id` | 无法回答“哪个决定导致了这次派发” |
| 没有 claim / receipt / handoff / compaction 事件 | 只覆盖选批面，协调面与效果面缺失 |
| 没有 schema 版本 | 轨迹格式演进后，旧轨迹可能无法解释 |
| 没有哈希链 | 轨迹可被静默改写，审计价值有限 |
| 没有迟到 / 重复 / 乱序策略 | 多主机或网络抖动下，轨迹顺序不可信 |

**结论：现在的轨迹是“选批回放器”，还不是“端到端判定史”。**

---

## 2. 三套时钟：不要再用一个时间戳解释一切

### 2.1 三个时间

| 时钟 | 回答的问题 | 典型来源 | 不能用来做什么 |
|---|---|---|---|
| **事件时间** `event_time` | 事实是什么时候发生的 | 测试开始/结束、外部系统确认、消息投递 | 不能用来排序轨迹 |
| **观测时间** `observed_at` | 编排是什么时候知道的 | 宿主收到回执、读到外部状态 | 不能代表事实发生时间 |
| **逻辑时间** `logical` | 判定推进到哪一步 | `revision` / `epoch` / `fence` / `round` / `attempt` / `seq` | 不能换算成秒，不能做 SLA |

再加一条**因果边**：`causation_id`（谁导致的）和 `correlation_id`（属于同一条业务链）。时钟只能给“时刻”，因果边才能给“顺序”。

### 2.2 为什么不能混

考虑一次外部副作用：

```text
T1  发送请求
T2  外部系统实际执行成功
T3  网络断开，本地没有拿到响应
T4  对账查到“已执行”，记为 Confirmed
```

如果按 `recorded_at` 排序，会得到：

```text
T1 发送 -> T3 超时 -> T4 确认
```

看起来像“超时后又被确认”，但事实是 **T2 就已经发生了**。更糟的是，如果 T4 的回执晚到，而 T3 之后又发了别的任务，按记录时间排序会把“后发起的任务”排在“先发生的事实”之前。

正确排序应该是：

```text
因果：T1 发送 -> T2 外部发生 -> T4 观测到并确认
```

回放顺序用**逻辑时间 + 因果边**；SLA 和业务延迟用**事件时间**；运维排障用**观测时间**。三者各归其位。

### 2.3 用哪套时钟做什么

| 用途 | 用哪套时钟 | 理由 |
|---|---|---|
| 回放/重算顺序 | `seq` + `causation_id` | 并发事件没有天然全序，不能假装有 |
| 业务延迟（效果、交接、测试） | `event_time` | 用户关心事实发生到事实结束 |
| 排队/处理延迟 | `observed_at` | 运维关心“我们多久才知道” |
| 跨主机破平 | HLC `(physical, counter, writer)` | 物理位只做近似，计数与写者保证全序 |
| 真相维护 | `epoch` / `revision` | 旧定义的证据必须作废，与墙钟无关 |
| 写权限 | `fence` | 单调世代，旧写者一律拒绝 |
| 租约/截止期 | 注入的 `now` + TTL | 内核不读系统时钟，测试与回放可确定 |

---

## 3. 轨迹的数据模型

### 3.1 从“一步快照”到“事件流”

建议把 `TraceStep` 保留为**压缩检查点**，新增 `TraceEvent` 作为基本单位：

```rust
pub struct TraceEvent {
    pub schema: u32,                 // 轨迹格式版本
    pub trace_id: TraceId,           // 一次会话/一条业务链
    pub span_id: SpanId,             // 一个操作
    pub parent: Option<SpanId>,      // 因果父操作
    pub seq: u64,                    // 同一 trace 内严格递增
    pub kind: TraceEventKind,
    pub causation_id: Option<EventId>, // 直接触发它的事件
    pub correlation_id: String,      // task / effect / handoff / obligation
    pub logical: LogicalTime,        // revision / epoch / fence / round / attempt
    pub event_time: Option<u64>,     // 事实发生时间；未知就是 None
    pub observed_at: u64,            // 编排观察到的时间（注入）
    pub recorded_at: u64,            // 宿主写入时间，仅供审计，不参与排序
    pub actor: Actor,                // agent / human / host
    pub payload_hash: String,        // 大载荷按引用存
    pub prev_hash: String,           // 哈希链，防静默改写
}

pub struct LogicalTime {
    pub revision: u64,
    pub epoch: u64,
    pub fence: u64,
    pub round: u64,
    pub attempt: u64,
}
```

### 3.2 事件类型

| 类别 | 事件 | 关键字段 |
|---|---|---|
| 计划 | `PlanPatched` | base_revision、add/remove/replace、epoch bump |
| 判定 | `JudgmentDerived` | verdict（T/F/U/B）、evidence ids、missing edges |
| 选批 | `TaskSelected` / `TaskHeld` / `TaskDenied` | admission、reasons、obligations |
| 效果 | `EffectPrepared` / `EffectSending` / `EffectSent` / `EffectConfirmed` / `EffectRefuted` / `EffectUnknown` | operation、attempt、fence |
| 对账 | `EffectReconciled` | attempt、observed Present/Absent、旧状态 `from`、新状态 `to` |
| 认领 | `ClaimAcquired` / `ClaimRenewed` / `ClaimReleased` / `ClaimExpired` | owner、paths、fencing、expires_at |
| 回执 | `ReceiptSent` / `Delivered` / `Read` / `Acted` / `Rejected` / `Expired` | idempotency_key、reason |
| 交接 | `HandoffGated` | gate T/U/F、state_hash、expires_at、obligation |
| 义务 | `ObligationScheduled` / `ObligationFulfilled` / `ObligationSkipped` | kind、cost、owner、deadline、reason |
| 上下文 | `CompactionDecided` | tier、admission、blocks、obligations |
| 完成 | `FinishRefused` / `FinishConfirmed` | pending/in_flight/inconclusive/conflicts |

> `EffectReconciled` 现在携带 `attempt / observed / from / to`。`Unknown -> Confirmed/Refuted`
> 必须有匹配的对账事件：Confirmed 需要 `Present`，Refuted 需要 `Absent`。

### 3.3 快照 + 事件：混合模型

纯事件流回放成本高，纯快照又丢失过程。建议：

```text
checkpoint(seq=N) + events(N+1..M)
```

- 检查点保存 fold 后的状态（plan/snapshot/ledger/claims/receipts）；
- 事件流保存从 N 之后发生的事实；
- 回放到任意时刻：加载最近检查点，再 fold 后缀事件。

`TraceStep` 可以作为检查点的一种，不必立刻废弃。

---

## 4. 排序规则

### 4.1 局部全序，全局偏序

- **同一 correlation**（同一个任务、效果、交接）内：`seq` 是严格全序。
- **跨 correlation**：只有因果边给出的偏序；并发事件**没有**天然先后。
- **跨主机**：用 HLC 做确定性破平，但不宣称它是事实时间。

不要把偏序强行变成全序。强行排序的代价是：并发被误读成因果，晚到被误读成后发。

### 4.2 回放排序

```text
sort by (trace_id, seq)
```

不是：

```text
sort by recorded_at   // 错
sort by event_time    // 也错（事件时间可能相同或缺失）
```

`recorded_at` 只用于审计“什么时候写下来的”。`event_time` 只用于业务窗口与 SLA。

### 4.3 晚到、重复、乱序

| 情况 | 处理 |
|---|---|
| 重复事件 | 按 `event_id` / idempotency key 去重；重复推进到当前态幂等 |
| 晚到但在宽限期内 | 接收，按 `seq` 插入；可用事件时间修正指标，但**不改写历史决定** |
| 晚到超出宽限期 | 记为 `LateButRecorded`；作为新证据，不追溯修改已发生的判定 |
| 乱序 | 按 correlation 缓冲到 watermark；跨 correlation 用因果边，不用时间戳 |
| 缺失事件 | 记 `TraceGap`，不要用“看起来合理”的默认值补齐 |
| 更正 | 发**补偿事件**，不改旧事件；历史只追加 |

### 4.4 反模式

- `sort_by(recorded_at)` 当因果顺序。
- 用最后写入者获胜解决冲突。
- 把逻辑时钟的数值当毫秒做 SLA。
- 忽略 `epoch`，拿旧定义的证据支撑新结论。
- 把 TTL 过期当成删除：过期不是 Free，也不是 False。
- 把 `Sent` 当 `Acted`，把“发出去了”当“协调完成”。

---

## 5. 回放模型

### 5.1 四种回放

| 模式 | 输入 | 输出 | 是否产生副作用 |
|---|---|---|---|
| 精确回放 | 原轨迹 + 原 `now` | 重算决定；与记录逐项对比 | 否 |
| 反事实回放 | 原轨迹 + 新策略/配置 | 差异报告；危险建议判红 | 否 |
| 状态重建 | `state_at(seq)` | 那一时刻的计划、快照、台账、认领、回执 | 否 |
| 因果追问 | `why(event_id)` | 证据证书、触发链、缺失边 | 否 |

### 5.2 确定性要求

- 内核不读系统时钟；所有 `now` / `ttl` 由轨迹提供。
- 所有集合遍历用稳定顺序（`BTreeMap` / 显式排序键）。
- 随机性必须带种子并记录在轨迹里。
- 外部输入（文件、环境、网络响应）以哈希或引用入轨迹。
- 回放不得调用外部系统；`dispatch_ready` 只做校验，不发请求。

### 5.3 差异分类

现在的 `ShadowReport` 只区分“策略建议不同”和“危险额外派发”。完整回放需要更细的差异类型：

| 差异 | 含义 | 处置 |
|---|---|---|
| `PolicyOnly` | 影子策略与内核不同 | 预期内，记录 |
| `KernelChanged` | 内核版本/代码变化 | 用版本号标注，不混入回归 |
| `MissingEvent` | 轨迹缺事件 | 报 `TraceGap`，不猜 |
| `ClockSkew` | 时间戳互相矛盾 | 用 HLC/因果边复核 |
| `Nondeterminism` | 同输入不同输出 | 视为 bug，隔离 |
| `SchemaDrift` | 轨迹格式版本不匹配 | 拒绝静默解释 |
| `DataLoss` | 台账/快照缺项 | 失败关闭，不用默认值补齐 |

关键原则：**回放重算决定，但信任记录下来的事实**。receipt、observation、effect transition 是输入；selection 是输出。不要“信任历史结论”，也不要“怀疑历史事实”。

---

## 6. 时间与判定状态

### 6.1 证据的两种“过期”

一条证据有两个时间：

```rust
pub struct EvidenceTime {
    pub event_time: Option<u64>, // 事实何时发生；未知则 None
    pub observed_at: u64,        // 我们何时知道
    pub ttl: u64,                // 事实有效期
}
```

对应两种新鲜度：

| 新鲜度 | 公式 | 用途 |
|---|---|---|
| 事实新鲜 | `event_time + ttl > now` | 能不能支撑当前判定 |
| 观测新鲜 | `observed_at + ttl > now` | 缓存/观测值是否还值得复用 |

**外部事实必须用事件时间判断新鲜度。** 一份一小时前跑完、刚刚才读到的测试结果，不会因为“刚读到”就变新鲜。

如果 `event_time` 未知：

- 低风险判定：可以用 `observed_at`，但标记 `event_time_unknown = true`；
- 高风险/不可逆判定：新鲜度判 `Unknown`，先生成补证或换基义务。

这比“默认新鲜”安全得多。

### 6.2 各种时间谓词

| 谓词 | 时钟 | 过期后的正确语义 |
|---|---|---|
| 认领 TTL | `now + ttl` | 过期 ≠ Free；要重新 claim、拿新 fence |
| 证据新鲜度 | `event_time + ttl` | 过期 → Unknown，不是 False，也不是“沿用最后已知值” |
| 任务 epoch | 逻辑时间 | 定义变更 → 旧证据作废，可重新派发 |
| 写者 fence | 逻辑时间（单调） | 旧 fence 的写一律拒绝 |
| 回执过期 | `now` / `observed_at` | 过期 → Unknown；晚到的 `Acted` 是新事实，不追溯 |
| 交接包过期 | `now` | 过期 → Unknown；先对账/重新交接 |
| 义务 deadline | 绝对逻辑时间 | 过期仍先排，但标 `overdue`；不静默丢弃 |
| 效果在途年龄 | `observed_at` | 超过对账期限 → 升级 `Reconcile` / `Escalate` |
| 重试/尝试号 | 逻辑时间 | 达上限 → 换义务类型，不无限循环 |

### 6.3 一个判定时间线

```text
seq 1  PlanPatched(rev=7)
seq 2  TaskSelected(a)
seq 3  EffectPrepared(a, attempt=1, fence=3)
seq 4  EffectSending(a)              // 落盘后才真正发
seq 5  EffectUnknown(a)              // 网络断开
seq 6  ObligationScheduled(Reconcile, deadline=...)
seq 7  EffectReconciled(a, Present)  // 外部事实：其实在 seq 5 之前就已发生
seq 8  EffectConfirmed(a)
seq 9  FinishRefused(conflicts=0, in_doubt=0)
```

注意 `seq 7` 的 `event_time` 可能早于 `seq 5` 的 `observed_at`。**回放按 seq 走，SLA 按 event_time 算**。两者都对，混在一起就都错。

> 注：`seq 7` 是独立对账事件，携带 `observed`、`from`、`to`。`seq 8` 的 Confirmed
> 只有在 `observed = Present` 时才合法；Refuted 则要求 `Absent`。

---

## 7. 指标：按时间口径拆开

### 7.1 三类指标

| 类别 | 指标 | 口径 |
|---|---|---|
| 事件时间 | 效果延迟（Confirmed - Prepared）、交接延迟（Acted - Sent）、测试时长 | 业务事实 |
| 观测时间 | 决策延迟、排队延迟、对账积压年龄、trace 写入滞后 | 运维 |
| 逻辑时间 | 每判定轮数、epoch 抖动、fence 冲突、未定质量随轮次变化 | 正确性/稳定性 |

### 7.2 三值特有指标

| 指标 | 定义 | 目标 |
|---|---|---|
| `unknown_mass` | `Σ weight(U) + 2 × weight(Conflict)` | 单调下降 |
| `false_completion_rate` | 报完成但仍有 U / B / in-doubt 的比例 | 必须为 0 |
| `stale_ratio` | 因 event_time 过期而失效的证据比例 | 越低越好 |
| `conflict_ratio` | B 判定占已结算判定的比例 | 观察，不追零 |
| `in_doubt_age` | `Sending/Sent/Unknown` 效果的最长年龄 | 有上限 |
| `reconcile_backlog` | 待对账效果数 | 有上限 |
| `obligation_age_p95` | 义务从生成到完成的 95 分位 | 按 kind 分档 |
| `overdue_obligations` | 超过 deadline 仍未完成的义务数 | 观察 + 告警 |
| `trace_gap_count` | 缺失事件段数量 | 必须为 0（审计轨迹） |

### 7.3 不要这样算

- 用 `recorded_at` 算业务 SLA。
- 用 `event_time` 给回放排序。
- 把 `overdue` 当 `skipped`。
- 把 `conflicts` 并进 `inconclusive`（两者出口不同）。
- 用“任务完成数”代替“判定完成数”。

当前实现 `TraceMetrics` 已把事件时间与观测时间两套延迟分开，并单独计数
`clock_skew_events` 与 `event_time_missing`；逻辑时间先给出 revision / epoch / fence / round 的上界。

---

## 8. 存储、保留与审计

### 8.1 布局

```text
traces/
  <trace_id>/
    events.jsonl        # 追加式事件流
    checkpoints/        # 定期快照（含 CHECKPOINT_VERSION）
    index.json          # 按 correlation / seq / event kind 建索引
```

### 8.2 追加与完整性

- 只追加，不修改；更正用补偿事件。
- `prev_hash` 形成哈希链：不是共识，是**防静默改写**。
- `schema` 版本与 `CHECKPOINT_VERSION` 分开：事件格式和状态格式各自演进。
- 大载荷（prompt、日志、diff）按引用/哈希存，不直接塞进事件。
- 写入失败不能改变内核决定，但必须告警；必要时进入“降级审计”模式并记录 `TraceGap`。

### 8.3 保留与脱敏

| 数据 | 保留策略 |
|---|---|
| 判定/效果/回执事件 | 长期保留（审计最小集） |
| 大载荷 | 短期 + 引用；到期只删内容，不删哈希与引用 |
| 高频调试事件 | 可采样，但不能采样掉判定与副作用事件 |
| secret / token / 用户原文 | 永不入轨迹；只存哈希/引用/脱敏占位符 |

### 8.4 多智能体与交接

- 交接包携带 `trace_id` / `span_id` / `correlation_id`；
- 接收方追加到同一 trace，或用 `parent` 链接成子 trace；
- 跨主机用 HLC 的 `(physical, counter, writer)` 破平；
- 交接后的 `Acted` 是接收方的事实，不是发送方的“已读即完成”；
- 跨 trace 合并时，保留两条链的因果边，不强行线性化。

当前实现：`merge_logs` 先按 seq 与 `causation_id` 建依赖图，再用 HLC 作为就绪事件的破平键。
同 event id 内容不同判冲突；无法拓扑排序判因果环；没有 HLC 的事件按 `Hlc::ZERO` 参与破平，但仍受因果边约束。

---

## 9. 轨迹不变量

轨迹的真正价值，是能把“架构原则”变成可执行检查。建议在回放时逐条验证：

| 不变量 | 违反含义 |
|---|---|
| 每个 `Dispatch` 前有同 epoch / fence 的 `Prepared` | 可能重复发送 |
| 每个 `Confirmed` 前有 `Reconciled(Present)` 或明确的外部回执 | 可能把“以为成功”当成功；当前已检查 `Unknown -> Confirmed/Refuted` 前必须有匹配的 `EffectReconciled` |
| 每个 `Acted` 前有 `Read` | 可能把“送达”当“照做” |
| 每个 `Unknown` 判定有至少一条义务事件 | 未知变成了死路 |
| 每个 `Conflict` 有 `Adjudicate` 义务 | 冲突被多数投票压掉 |
| 每个 `FinishConfirmed` 时没有 U / B / in-doubt | 虚假完成 |
| 每个 `EffectRefuted` 后才允许新的 attempt | 盲目重试 |
| 每个旧 `fence` 的写都被拒绝 | 僵尸写者复活 |
| 每个 `TraceGap` 都被显式记录 | 轨迹在假装完整 |

这些不变量可以直接做成回放检查器；失败时输出 `event_id + invariant + 证书`，而不是一句“回放不一致”。

当前 `check_invariants` 已覆盖：`MissingSeq`、`MissingCausation`、`EffectOutOfOrder`、`StaleFence`、
`ReceiptOutOfOrder`、`HandoffWithoutObligation`，以及 `UnknownWithoutObligation`、
`ConflictWithoutAdjudication`、`FinishWithUndetermined` / `FinishWithoutPayload`。
`Confirmed` 前对账现在完整检查：`Unknown -> Confirmed` 必须有 `EffectReconciled(Present)`，
`Unknown -> Refuted` 必须有 `EffectReconciled(Absent)`；`from`/`to`/attempt 不匹配或缺载荷都会判红。

---

## 10. 落地路线

| 阶段 | 状态 | 内容 | 验收 |
|---|---|---|---|
| T0 | 已完成 | `TraceStep` / `TraceStore` / `replay`；注入式 `now`；revision/epoch/fence | 选批可回放；影子回放可用 |
| T1 | 已完成（T1.1–T1.4） | 事件模型：`TraceEvent`、`LogicalTime`、`seq` / `trace_id` / `correlation_id` / `causation_id`、`event_time` / `observed_at`、schema 版本、按 trace 的事件日志、折叠、JSONL 持久化与旧 step 迁移 | 同一 trace 可按 seq 重放；重复幂等；seq 冲突判红；晚到可标记；schema/trace 校验失败关闭；旧 `TraceStep` 可迁移 |
| T2 | 已完成 | 把 claim / receipt / handoff / compaction / obligation / effect 事件纳入轨迹 | 协调面与效果面可回放 |
| T3 | 已完成 | `state_at(seq)`、因果追问、差异分类 `ReplayReport` | 能回答“当时知道什么、为什么” |
| T4 | 已完成（T4.1–T4.3） | 按事件时间/观测时间/逻辑时间拆指标；保留与脱敏；多主机 HLC 归并 | 指标口径不混；跨主机轨迹可对账 |
| T5 | 已完成 | 轨迹不变量检查进 CI；变异测试覆盖“删事件/改顺序/改 fence” | 不变量被破坏时回放判红 |

T1 是最小分水岭。没有 `seq` / 因果边 / 两种时间，后面的回放和指标都只能靠猜。

### 10.1 T1 切片

| 切片 | 状态 | 内容 | 验收 |
|---|---|---|---|
| T1.1 事件模型 | 已完成 | `TraceEvent` / `LogicalTime` / `Actor` / `TraceEventKind`；`event_time` 可为 `None`，`observed_at` 必有 | 双时间不互相覆盖；逻辑时间可序列化 |
| T1.2 事件日志 | 已完成 | `TraceLog`：按 `seq` 排序、按 event id 去重、同 seq 不同事件判红、迟到标记、缺口计算、schema/trace 校验 | 乱序追加后仍按 seq 输出；重复不翻倍；冲突不静默合并 |
| T1.3 选批折叠 | 已完成 | `TracePayload` 完整状态事件；`TraceLog::fold` / `latest_step` 折叠出 `TraceStep` 所需的 plan/snapshot/limits/ledger | 同一 trace 的选批结果可复算；缺输入或 revision 不一致失败关闭 |
| T1.4 文件持久化 | 已完成 | `TraceEventStore`：事件 JSONL 追加/加载；schema 版本；坏行报行号；`events_from_step` / `append_step` 迁移旧 `TraceStep` | 写入后 `sync_data`；旧轨迹可迁移并折叠回同一 step |

T1.2 是当前最小可测切片：先把“按 seq 排序、重复幂等、冲突判红、晚到标记、缺口可见”钉死，再谈折叠与持久化。

测试：`crates/trit-orch/tests/trace_time.rs`（9 条）+ `crates/trit-orch/tests/trace_event_store.rs`（5 条），均已通过。

### 10.2 T2 切片

| 切片 | 状态 | 内容 | 验收 |
|---|---|---|---|
| T2.1 协调载荷与记录器 | 已完成 | `ClaimAction` / `ReceiptAction` / `ObligationOutcome`、`ClaimRecord` / `ReceiptRecord` / `HandoffRecord` / `ObligationRecord` / `EffectRecord`、`TraceRecorder` | 真实协调对象生成带 kind / correlation / seq 的事件 |
| T2.2 协调审计折叠 | 已完成 | `TraceLog::coordination`：claims / receipts / handoffs / compactions / obligations / effects | 事件流可还原协调与效果审计状态 |
| T2.3 持久化往返 | 已完成 | 新载荷经 `TraceEventStore` 追加、加载、重建 `TraceLog` | JSONL 往返后协调审计状态一致 |

测试：`crates/trit-orch/tests/trace_coordination.rs`（4 条，已通过）。

### 10.3 T3 切片

| 切片 | 状态 | 内容 | 验收 |
|---|---|---|---|
| T3.1 状态重建 | 已完成 | `TraceLog::state_at` / `step_at` | 指定 seq 的状态；未来事件不参与 |
| T3.2 因果追问 | 已完成 | `TraceLog::why` / `CausalChain` | 根 → 目标；缺失 cause 显式列出；成环判红 |
| T3.3 选择回放 | 已完成 | `SelectionCaptured` 载荷、`replay_selections`、`ReplayReport` | 匹配 / 差异 / 缺输入可分类 |

测试：`crates/trit-orch/tests/trace_replay.rs`（3 条，已通过）。

### 10.4 T4 切片

| 切片 | 状态 | 内容 | 验收 |
|---|---|---|---|
| T4.1 时间口径指标 | 已完成 | `TraceMetrics` / `TraceLog::metrics`：event / observation / logical 三套口径 | 延迟口径不混；在途年龄、交接延迟、写入滞后、时钟回拨可见 |
| T4.2 保留与脱敏 | 已完成 | `RedactionPolicy`、`redacted` / `redacted_with`、存储级 `redact_before` / `compact_before` | 事件骨架长期保留；旧载荷压缩为 `fnv1a64` 哈希；secret 按策略替换；因果审计不丢 |
| T4.3 多主机 HLC 归并 | 已完成 | `Hlc` 字段、`merge_logs`：seq/因果边做拓扑约束，HLC 做并发破平 | 并发有确定性全序；因果优先于 HLC；重复去重、冲突/环失败关闭 |

测试：`crates/trit-orch/tests/trace_metrics.rs`（3 条）+ `crates/trit-orch/tests/trace_redaction.rs`（3 条）+ `crates/trit-orch/tests/trace_merge.rs`（5 条），均已通过。

### 10.5 T5 切片

| 切片 | 状态 | 内容 | 验收 |
|---|---|---|---|
| T5.1 不变量检查器 | 已完成 | `TraceLog::check_invariants` / `TraceInvariantReport`：seq、因果、effect、回执、fence、交接、verdict、finish、对账 | 干净轨迹零违例；违例带 event / seq / 具体原因 |
| T5.2 变异测试 | 已完成 | 删事件、改顺序、改 fence、Acted 缺 Read、交接缺义务、因果引用缺失、Unknown/Conflict/Finish/对账边界 | 每种变异都被判红 |
| T5.3 CI 接入 | 已完成 | 新 test target 由现有 `cargo nextest run --workspace` / `cargo test --workspace` 自动执行 | 无需改 workflow |
| T5.4 边界载荷 | 已完成 | `Judgment` / `Finish` payload、`TraceRecorder::judgment` / `finish` | Unknown 有义务、Conflict 有 Adjudicate、FinishConfirmed 无 U 可检查 |
| T5.5 独立对账事件 | 已完成 | `EffectReconciled { attempt, observed, from, to }` payload、`TraceRecorder::effect_reconciled`、`Unknown -> Confirmed/Refuted` 对账检查 | `Present/Absent` 证据可回放；不变量第 2 条完全覆盖 |

测试：`crates/trit-orch/tests/trace_invariants.rs`（16 条，已通过）。

T1–T5 已完成。三条边界不变量已通过 `Judgment` / `Finish` 载荷接入；
`Unknown -> Confirmed/Refuted` 现在要求匹配的 `EffectReconciled(Present/Absent)` 证据。

---

## 11. 开放问题

1. **事件时间未知怎么办**：所有外部事实都能拿到可信的 `event_time` 吗？拿不到时，高风险判定是否一律 `Unknown`？
2. **宽限期多长**：晚到事件在多久内可以修正指标？超过后如何只追加补偿事件？
3. **HLC 够不够**：跨智能体是否需要向量时钟来检测并发冲突，而不只是全序破平？
4. **轨迹格式演进**：schema 升级时，旧事件如何解释？是否保留未知字段？
5. **脱敏与回放**：prompt 被脱敏后，回放还能复现判定吗？哪些字段必须保留哈希？
6. **压缩与因果**：检查点可以压缩状态，但能否压缩掉中间因果边？哪些证书必须永久保留？
7. **多主机重复**：同一事件从两条链路到达，如何在不信任时钟的前提下判重？
8. **写入失败的边界**：轨迹写失败时，是否允许继续产生副作用？建议允许但降级告警，需要明确策略。
9. **“未知的未知”**：缺失事件如何与“确实没有发生”区分？`TraceGap` 的表达力够不够？
10. **人的时间**：人的决策延迟是否应该进入同一套 deadline/预算，还是单独建模？

---

## 附录 A：现有代码与建议落点

| 概念 | 现有落点 | 状态 |
|---|---|---|
| 单步回放 | `crates/trit-orch/src/shadow.rs:7` `TraceStep` | 已有；快照式，无 `now` |
| JSONL 轨迹 | `shadow.rs` `TraceStore` | 已有；行号诊断，无 schema/seq |
| 内核重算 | `shadow.rs:41` `replay` | 已有；仅选批面 |
| 影子差异 | `ShadowReport` | 已有；差异类型较少 |
| 注入时间 | `claim.rs` / `receipt.rs` / `context.rs` | 已有；未进轨迹 |
| 逻辑时钟 | `revision` / `epoch` / `fence` / `attempt` | 已有；未统一为 `LogicalTime` |
| 运行时指标 | `crates/trit-schedule/src/runtime/types.rs:166` | 已有；计数，无时长/时间戳 |
| 检查点版本 | `crates/trit-schedule/src/checkpoint/mod.rs` | 已有；与轨迹 schema 未打通 |
| HLC | `crates/trit-immune/src/hlc.rs` | 已有；可复用于跨主机轨迹 |
| `TraceEvent` / `seq` / 因果边 | `crates/trit-orch/src/trace.rs`、`trace_store.rs` | T1/T2 已完成：事件模型、日志、折叠、持久化、旧 step 迁移、协调/效果事件；因果边校验待 T3 |
| 协调面 / 效果面事件 | `trace.rs`（`TraceRecorder` / `CoordinationFold`） | 已完成：claim / receipt / handoff / compaction / obligation / effect 的载荷与折叠 |
| 状态重建 / 因果追问 / 选择回放 | `trace.rs`（`state_at` / `why` / `replay_selections`） | T3 已完成：历史状态、因果链、ReplayReport 差异分类 |
| 时间口径指标 | `trace.rs`（`TraceMetrics` / `metrics`） | T4.1 已完成；HLC 归并待补 |
| 保留与脱敏 | `trace.rs` / `trace_store.rs`（`RedactionPolicy` / `redact_before` / `compact_before`） | T4.2 已完成：payload 哈希、策略脱敏、旧载荷压缩 |
| 多主机 HLC 归并 | `trace.rs`（`Hlc`）/ `trace_merge.rs`（`merge_logs`） | T4.3 已完成：因果优先、HLC 破平、去重/冲突/环判红 |
| 轨迹不变量 | `trace_invariants.rs`（`check_invariants`） | T5 已完成：seq/因果/effect/回执/fence/handoff + verdict/finish/conflict + 对账证据 |
| 独立对账事件 | `trace.rs`（`EffectReconciled`）/ `TraceRecorder::effect_reconciled` | 已完成：payload `attempt/observed/from/to`；`Unknown -> Confirmed/Refuted` 必须匹配 Present/Absent |
| `event_time` / `observed_at` 双时间 | 本文第 6 节 | 建议新增 |
| 回放差异分类 | 本文第 5.3 节 | 建议新增 |
| 轨迹不变量检查 | 本文第 9 节 | 建议新增 |

## 附录 B：最小 API 草图

```rust
pub struct TraceEvent {
    pub schema: u32,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent: Option<SpanId>,
    pub seq: u64,
    pub kind: TraceEventKind,
    pub causation_id: Option<EventId>,
    pub correlation_id: String,
    pub logical: LogicalTime,
    pub event_time: Option<u64>,
    pub observed_at: u64,
    pub recorded_at: u64,
    pub actor: Actor,
    pub payload_hash: String,
    pub prev_hash: String,
}

pub trait TraceSink {
    fn append(&mut self, event: &TraceEvent) -> Result<EventId, TraceError>;
    fn checkpoint(&self) -> Result<CheckpointId, TraceError>;
}

pub enum ReplayMode {
    Exact,
    Counterfactual { policy: String },
    StateAt { seq: u64 },
    Why { event: EventId },
}
```

## 附录 C：术语

| 术语 | 含义 |
|---|---|
| 事件时间 | 事实实际发生的时间 |
| 观测时间 | 编排获知事实的时间 |
| 逻辑时间 | revision / epoch / fence / round / attempt / seq 构成的推进序列 |
| 因果边 | `causation_id` / `parent`，表示谁直接导致谁 |
| 关联 ID | `correlation_id`，把同一业务链的事件串起来 |
| 检查点 | fold 后的状态快照，与事件流配合回放 |
| 补偿事件 | 不修改历史、用于更正的新事件 |
| `TraceGap` | 明确记录“这里缺事件”，而不是假装连续 |
| HLC | 混合逻辑时钟；物理位近似，计数与写者保证全序 |
| 回放差异 | 重算决定与记录决定之间的分类差异 |
