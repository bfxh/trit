# 三值编排系统设计

> 副标题：把“未定”当作一等公民的智能体编排模型  
> 基础：`bfxh/trit` 的三值逻辑与 `trit-orch` 已落地原语  
> 日期：2026-11-13

---

## 0. 一句话

**老编排调度动作；这套编排调度判定。**

任务不是目标，任务是生产证据的手段。系统的产出不是“跑完了”，而是“哪些命题已被证明、哪些已被否证、哪些还没定，以及为什么、下一步补什么”。

这份文档不是给 trit 加一层编排包装，而是提出一个判断：主流智能体编排缺的不是更强的调度器，而是一套关于“不知道”的语义。三值逻辑恰好提供了这套语义——未知可以参与运算、可以沿依赖传播、可以带证书解释，也可以被定价、被排期。

先澄清一个术语：本文用的是**三值逻辑**（True / False / Unknown），不是数值上的 balanced ternary（-1 / 0 / +1）。前者负责语义，后者负责表示。`trit` 两层都有；编排主要吃第一层。将来若用 tryte 压缩状态，语义不变。

文中标注 `[建议新增]` 的部分是设计提案，不是现有实现；附录 A 给出每个概念在仓库里的落点与状态。

---

## 1. 旧编排的六个语义缺口

这些模型在它们的问题域里是对的：工作流引擎假设活动可重试，图编排假设节点是动作，多智能体框架假设消息就是协调。问题在于，LLM 智能体同时打破了这三个假设。

| # | 旧假设 | 在智能体场景下为什么失效 | 三值的回答 |
|---|---|---|---|
| 1 | 成功 / 失败二元 | “还没定”被折叠成 error，于是触发重试、升级或幻觉式断言 | Unknown 是独立状态，有独立路由 |
| 2 | DAG 的边只表达先后 | 边不表达“凭什么可以开始”，也不表达“这个前置到底需不需要” | 三值依赖：必需 / 可选 / 相关性未知 |
| 3 | 重试是默认恢复手段 | 重试假设幂等；LLM 的外部副作用（发消息、开 PR、付款）通常不幂等 | 效果台账：未知先对账，不盲重试 |
| 4 | 完成 = 所有任务结束 | 跳过验证、零断言、超时都能“完成” | 完成闸门：未定 / 未验一律拒绝报完成 |
| 5 | 消息发送 = 协调成功 | `Sent` 只证明传输层收了，不证明对方读过、更不证明照做 | 四态回执：Sent → Delivered → Read → Acted |
| 6 | 未知 = 异常 | 人只在 catch 分支里出现，成为兜底而不是决策者 | 义务队列：未定有 owner、成本、deadline |

有三个常见但危险的做法，本质上是同一个错误：

- **把 Unknown 当 False**：误杀本可恢复的任务。
- **把 Unknown 当 True**：乐观推进，把不确定性留到最后一刻爆炸。
- **把 Unknown 当“再试一次”**：重试不产生新信息，只产生新成本。

一句话：**重试不是补证，重试只是重复同一个实验。**

---

## 2. 五条公理

### A1 未定不是失败

`Unknown ≠ False`。未定不能触发失败路径，不能被计入“已否证”，不能被重试逻辑消费。它有自己的出口：补证、换路径、降级、挂起、问人。

### A2 未证不是已证

没有证据时，默认值必须是 `Unknown`，不能是 `False`，更不能是 `True`。

这条在代码里比在文档里更容易犯。`Ternary::default()` 是 `False`，所以每一道安全门都必须显式初始化成 `Unknown`——没证据不等于证据表明安全。同理：

- 零断言 = `Inconclusive`，不是通过；
- 未认领 ≠ 可写；
- 没收到拒绝 ≠ 已授权；
- 供应商没报溢出 ≠ 没有溢出。

### A3 不可逆动作只认 Confirmed

对于删除、force push、发布、付款、对外发消息这类动作，只认外部确认。`Sending` / `Sent` / `Unknown` 一律按“可能已经发生”处理：先对账，再决定是否允许下一次尝试。

不可逆门用 B3（Bochvar）语义：**沾上 Unknown 即为 Unknown，不给部分学分**。没有“大概率没问题”。

### A4 结论必须带证书

每一个 True / False / Unknown 都要能回答“凭什么”。

- True：给出通过的检查、断言数、证据出处；
- False：给出反例或否证回执；
- Unknown：给出**缺失的边**和**补证义务**。

“不知道”不是一句话，是一张待办。

### A5 证据有版本和保质期

证据的作用域是 `(命题, epoch, base, 时间)`。任务定义一换、基线一换、输入一过期，旧证据立即作废。

过期证据既不能证明成功，也不能证明失败，只能让结论回到 `Unknown`。这不是保守，这是正确：拿旧结论支撑新决策，是智能体系统里最常见、也最隐蔽的错误。

---

## 3. 核心抽象：判定，而不是任务

### 3.1 三个对象

```rust
pub struct Judgment {
    pub id: String,
    pub proposition: String,      // 要判定的命题
    pub epoch: u64,               // 命题定义的版本
    pub base: String,             // commit / 快照 / 输入哈希
    pub min_assertions: u32,      // 覆盖下限
    pub weight: u32,              // 未定质量权重
    pub evidence: Vec<Evidence>,
    pub obligations: Vec<Obligation>,
}

pub struct Evidence {
    pub id: String,
    pub source: String,           // executor / agent / human / ci
    pub epoch: u64,
    pub base: String,
    pub observed_at: u64,
    pub ttl: u64,
    pub result: CheckResult,      // Passed / Refuted / Inconclusive
    pub checks: u32,
    pub provenance: String,       // 回执 ID / 日志 / 签名
}

pub struct Obligation {
    pub id: String,
    pub judgment: String,
    pub kind: ObligationKind,
    pub expected: Option<Ternary>, // 补证后预期落到哪
    pub cost: Cost,               // tokens / wall / risk
    pub deadline: u64,
    pub owner: Option<String>,
    pub attempts: u32,
    pub max_attempts: u32,
    pub idempotency_key: String,
}
```

三个对象的关系：

- `Judgment` 是要回答的问题；
- `Evidence` 是目前为止的答案素材；
- `Obligation` 是把 `Unknown` 变成可判定结论的最小动作。

### 3.2 两张图

| 图 | 节点 | 边 | 回答的问题 |
|---|---|---|---|
| 判定图 | 命题 | 证据依赖 | 什么必须先被判定 |
| 动作图 | 任务 / 效果 | happens-before | 怎么生产证据、怎么产生副作用 |

两张图通过任务上的 `resolves` 字段绑定：

```rust
pub struct TaskSpec {
    pub task: Task,
    pub resolves: Vec<String>,    // 这个任务为哪些判定生产证据
    pub produces: EvidenceSpec,   // 证据规格（最低断言数等）
    pub effects: Vec<EffectSpec>, // 会产生的副作用
    pub requirements: Resources,
    pub max_tokens: u64,
    pub epoch: u64,
}
```

> 注：`resolves / produces / effects` 均已作为原生字段落地（`model.rs` / `effects.rs`）。
> `obligation_plan.rs` 把八种 kind 的义务都任务化：生成任务原生声明
> `resolves = [目标任务]` 与 `produces`（最少断言数），Pending 目标可重写依赖；
> 资源需求可按 kind 覆盖，link 携带目标已声明的认领路径。Claim 义务要求路径非空，
> 否则以 `NoClaimPaths` 延迟。`evidence_writeback.rs` 按 link 自动回写：
> 生成任务必须 Settled、证据属于其当前 epoch 且满足 `produces.min_assertions`，
> 回写时 rebase 到目标 epoch，并按 source 幂等去重。

`EffectSpec` 绑定台账操作 ID；未声明 `effects` 的任务沿用任务 ID 作为唯一操作。
声明多个操作时，任一操作在途都会挡下整个任务，且 `dispatch_ready` 要求每个操作
都有同 epoch、Prepared 的意图。`prepare_effects` 按同一 fence 原子准备全部操作，
失败不留半截意图。同一任务内归一化后重复的操作在 `Plan::new` 直接拒绝。

### 3.3 任务 = 证据生产器

任务不是“做 X”，而是“在约束下，为判定 J 生产证据 E”。

这条约束有一个直接推论：**不减少未定的动作，不该占用编排预算**。

纯副作用任务（部署、通知、发布）也有判定——“部署成功了吗”“对方收到了吗”——只是它的证据来自外部确认，而不是本地断言。没有判定的任务，要么是忙工，要么是漏掉的验收。

### 3.4 任务简报：从内核状态到智能体引导

引导不是自由发挥的 prompt，而是从计划、选批与义务推导出的结构化契约。

```rust
pub struct TaskGuidance {
    pub objective: String,        // 目标
    pub constraints: Vec<String>, // 硬约束
    pub stop_when: Vec<String>,   // 何时停下交回上游
}

pub struct TaskBrief {
    pub task: String,
    pub guidance: TaskGuidance,
    pub resolves: Vec<String>,
    pub produces: EvidenceSpec,
    pub effects: Vec<EffectSpec>,
    pub requirements: Resources,
    pub max_tokens: u64,
    pub admission: Ternary,
    pub granted_tokens: u64,
    pub blockers: Vec<BlockReason>,
    pub obligations: Vec<AdmissionObligation>,
}

task_brief(plan, selection, task) -> Result<TaskBrief, Error>
```

- `TaskGuidance` 是任务定义的一部分，随 epoch 一起失效；它不参与准入。
- `TaskBrief` 只读：`admission / blockers / obligations` 来自当前选批，宿主负责渲染成提示或工具参数。
- plan 与 selection 的 revision 必须一致；任务或决定缺失都 fail-closed。
- 生成义务任务时，objective 默认取义务 detail；空 detail 回退为“为 `<task>` 补齐 `<kind>` 证据”。
- 引导不改变内核决定：brief 说“可派发”不等于派发，`dispatch_ready` 仍是最后一道闸。

**研究对照（2026-11 检索）**

- “何时干预”比“干预文本”更难：[Learning to Steer LLM Agents and Multi-Agent Systems](https://arxiv.org/html/2610.22316v1)
  用快 / 慢两路判断是否干预，并让干预频率可控；[From Uncertainty to Action](https://arxiv.org/html/2610.09115v1)
  发现不确定性适合发现失败、但不适合定位该在哪一步纠正，改用反事实价值函数。
  本内核把“何时干预”落在义务队列与 `P(flip) / cost` 排序上，`stop_when` 只是静态提示，
  尚无学习型干预策略。
- 过程监督与逐级验证：[AgentPRM](https://arxiv.org/html/2511.08325v1) 用 TD / GAE 估步骤对目标的接近度；
  [Verification as an Architectural Layer for LLM Agents](https://arxiv.org/html/2609.31937v1)
  主张把验证做成强制层、每级规格配独立 verifier、判词必须 binding。
  本内核的依赖 / 证据门、完成闸门与轨迹不变量对应“确定性 gate”，但还没有 learned judge 与规格分级。
- 多验证器组合：[Multi-Agent Verification](https://arxiv.org/html/2502.20379v1) 用多 verifier 投票扩展测试时计算；
  本内核的双轨证据拒绝多数投票，冲突单独走 Adjudicate，这是有意的反向取舍。
- 编排综述：[A Survey on Agent Workflow](https://arxiv.org/abs/2508.01186) 与
  [The Orchestration of Multi-Agent Systems](https://arxiv.org/html/2601.13671v1)
  都把控制面（planning / execution / state）与治理（policy / quality / observability）分层；
  本内核的区别是状态与判定显式三值 / 四值，引导从状态推导，而不是另立一套 prompt 协议。

**遵循度检查**：`check_adherence(brief, observation)` 只看当前观察；
`check_adherence_with_trace(brief, observation, log, since_seq)` 只统计简报发出后的事件。
判定三类偏离：`ActedWhileBlocked`（准入未过却产生证据 / 回执）、
`EvidenceBelowSpec`（声称完成但判定为 `Neither` 或断言数不足）、
`ObligationsIgnored`（有行动但义务没有对应 `ObligationFulfilled`）。
每条偏离产出一条 subject 为 `adherence:*` 的 `Escalate` 义务；检查只读，不改内核状态，
也不重复派发。

**跨任务干预**：`plan_interventions(reports, policy)` 把单任务偏离收敛成分级计划：

- 安全类（`ActedWhileBlocked`）优先分组；默认 `Quarantine`：任务进入 `held`，
  并产出一条分组 Escalate。`quarantine_unsafe = false` 时只升级、不隔离。
- 被隔离任务不再叠加低优先级干预；`ObligationsIgnored` 分组升级；
  `EvidenceBelowSpec` 转成可任务化的 `Measure`（subject `intervention:evidence_gap`），每个任务一条。
- `tick` 接收 `InterventionPolicy`；`interventions.held` 是“解除前不要重新派发”的建议，
  内核仍不持久化 held 状态，解除也由宿主决定。

研究对照：[AgentSpec](https://arxiv.org/html/2503.18666v1) 用 trigger / predicate / enforcement
规则执行 Stop / User Inspection / Self-Examine / Invoke Action；[ProbGuard](https://arxiv.org/html/2508.00500v4)
先估计风险再干预；[ContrAgent](https://arxiv.org/html/2609.18128v1) 用有限轨迹上的线性时序逻辑；
[SupervisorAgent](https://arxiv.org/html/2510.26585v2) 用 LLM-free 过滤器在高风险点介入；
[Agent Safety Should Be a Runtime Contract](https://arxiv.org/html/2608.11274) 把安全拆成
preventive gate 与 evidential proof 两面。我们的干预是固定规则 + 事后证据对齐，
尚无用户自定义规则、风险预测与干预效果评估。

**生成与渲染**：`guidance_from_text` 是确定性规则抽取，不调用模型：

- 支持中英文段落标题：`目标/objective`、`约束/constraints/must`、`停止/stop when`；
- 无标题时按句号 / 分号切句：含“必须 / 禁止 / 不得 / 只能”（或 must / never / do not）
  的句子进约束；以“如果 / 一旦 / 当”（或 if / when / until）开头的句子进停止条件；
  其余句子并入目标；
- `render_brief` 把 `TaskBrief` 渲染成稳定纯文本提示：目标、约束、停止条件、证据规格、
  效果操作（未声明时显式写出任务 ID 回退）、准入状态、阻塞原因、待办义务。
  宿主可以直接用，也可以替换成自己的模板。

**遵循度聚合**：`GuidanceMetrics` 累计 `briefs`、有违规任务数、三类违规计数，
`violation_rate_bp` 给出“有违规的简报占比”；`tick` 每轮输出 `guidance_metrics`，
宿主可跨 tick 继续 `observe` 得到长期比率。

**当前缺口**：规则抽取只是 fallback，语义抽取仍需宿主用模型完成；
干预只到“计划 + held 建议”，没有解除状态、效果度量与学习型策略；
验证者分级与 learned judge 未接，当前只有确定性 gate、证据回写、遵循度与固定规则干预。

---

## 4. 代数分工：未知怎么组合

三值不是“三个状态的状态机”。三个状态如果按二元方式组合，仍然是二元系统。真正决定行为的是**未知如何参与运算**。

| 场景 | 代数 | 规则 | 直觉 |
|---|---|---|---|
| 所有前置都必须成立 | K3 AND | False 否决；否则有 U 则 U | 一票否决，未知顺延 |
| 任一替代路径成立即可 | K3 OR | True 短路；否则有 U 则 U | 有路可走就不算死 |
| 不可逆动作的前置 | B3 AND | 沾 U 即 U | 毒化，不给部分学分 |
| 独立证据累积 / 法定人数 | Ł3 强加 | `min(1, a+b)` | 两个 0.5 可到 1，需独立性假设 |
| 强保证的合取 | Ł3 强合取 | `max(0, a+b-1)` | 两个 0.5 不能互相担保 |
| 多源证据合并（共识） | Belnap consensus | `T⊗B=T`，`T⊗F=N` | 只保留两轨都同意的部分 |
| 多源证据合并（轻信） | Belnap gullibility | `T⊕F=B` | 两边都信，就制造了冲突 |
| 多驱动总线仲裁 | TriState | Z 是单位元；同值一致；0/1 冲突 | 沉默、同意、争用 |

### 4.1 不要用错代数

- **K3 OR 不是多数投票**。多数投票会把 `B`（两边都有证据）压成 `T` 或 `F`，丢掉“存在反证”这个事实。
- **B3 不是“更严格的 AND”**。B3 的用途是毒化：只要有一个输入未知，整个不可逆动作就不许发。它不该用于普通任务，否则系统会过度冻结。
- **Ł3 需要校准**。`X ⊕ X = True` 成立的前提是两票独立、权重已校准。两票来自同一个模型、同一段上下文，不叫独立证据。
- **Belnap 的 B 不是 U**。U 是“不知道”；B 是“有理由相信真，也有理由相信假”。前者要补证，后者要裁决。

### 4.2 双轨证据：为什么要存两轨

只存一个 `Ternary`，会丢掉“没有证据”和“证据冲突”的区别。双轨表示把两者都留下：

```text
(t, f) = (0, 0)  -> N / 未定：两边都没有证据
(t, f) = (1, 0)  -> T / 已证
(t, f) = (0, 1)  -> F / 已否证
(t, f) = (1, 1)  -> B / 冲突：两边都有证据
```

三值双轨禁止 `(1,1)`；四值 Belnap 正是把这条约束放开。**是否允许 `(1,1)`，是一个架构决策**：

- 单人、单执行器：可以禁止，冲突在写入前就应被挡住；
- 多智能体、多来源：必须允许，否则冲突会被静默吞掉。

当前实现：`EvidenceRails` 分别存 `support / counter / inconclusive`；
`Observation.evidence_rails` 非空时优先于单条 `evidence`。`CheckResult::Conflicted`
同时进入两轨；选批与完成闸门通过 `Observation::judgment` 派生
`Proved / Refuted / Neither / Conflicted`。

### 4.3 一个反例：为什么“三态状态机”不够

两个输入都是 Unknown：

- 在 K3 AND 下，结果是 Unknown；
- 在 B3 AND 下，结果还是 Unknown；
- 在 Ł3 强加下，结果是 True。

状态一样，语义不同，系统的行为也不同。**三值的价值在算子，不在标签。**

---

## 5. 判定派生：状态怎么算出来

判定状态不存储为“真相”，每次从证据重新派生。存储真相会漂移；派生真相可复算、可回放、可审计。

### 5.1 派生规则

```rust
fn derive(j: &Judgment, now: u64) -> Verdict {
    // 1. 只采信当前 epoch、当前 base、未过期的证据
    let ev: Vec<&Evidence> = j.evidence.iter()
        .filter(|e| e.epoch == j.epoch && e.base == j.base)
        .filter(|e| now <= e.observed_at.saturating_add(e.ttl))
        .collect();

    if ev.is_empty() {
        return Verdict::unknown("没有当前证据");
    }

    // 2. 证据分为支持轨与反对轨；Inconclusive 不进任何一轨
    let support = ev.iter().any(|e| e.result == Passed && e.checks >= j.min_assertions);
    let counter = ev.iter().any(|e| e.result == Refuted && e.checks >= j.min_assertions);

    match (support, counter) {
        (true, true)   => Verdict::conflict(),   // Belnap B，不是 Unknown
        (true, false)  => Verdict::proved(),
        (false, true)  => Verdict::refuted(),
        (false, false) => Verdict::unknown("只有未定证据或覆盖不足"),
    }
}
```

规则表：

| 输入情况 | 结论 | 动作 |
|---|---|---|
| 没有证据 | U | 生成 Measure 义务 |
| 只有 Inconclusive | U | 生成 Measure / 换测法义务 |
| 断言数低于下限 | U | 生成补覆盖义务 |
| 证据过期 | U | 生成 Rebasis 义务 |
| epoch / base 不匹配 | U | 旧证据作废，重新取证 |
| 同时有通过与否证 | B | 生成 Adjudicate 义务 |
| 只有通过且覆盖足够 | T | 可解锁下游 |
| 只有否证且覆盖足够 | F | 剪枝 / 回退 |

两条容易漏的守卫：

- **零断言必须是 Unknown**。`checks == 0` 时，无论 `Passed` 还是 `Refuted` 都不算数。
- **覆盖不足必须是 Unknown**。跑了一条断言通过，不等于命题成立。

### 5.2 证书

```rust
pub struct Verdict {
    pub value: Ternary,          // T / F / U
    pub conflict: bool,          // Belnap B（与 value 分开存）
    pub evidence: Vec<String>,   // 支撑这次结论的证据 ID
    pub epoch: u64,
    pub observed_at: u64,
    pub missing: Vec<MissingEdge>, // U 时：还缺哪条边
    pub obligations: Vec<String>,  // U 时：下一步补什么
}
```

`sansheng` 的三值拓扑已经给出了这个模式：`TernaryTopology::explain` 返回 `Proved` / `Undetermined` / `Refuted`，并在 `Undetermined` 时列出 `unknown_edges`——**未定带着“补哪条边”的线索，而不是一句“可能”**。编排的证书应当同构。

### 5.3 冲突为什么必须单独存在

把冲突压成 Unknown，会得到错误的路由：系统会去“再取一次证”，但问题不是证据不足，而是证据互相矛盾。再取一次同样的证，只会得到第三份矛盾。

把冲突压成 True 或 False（多数投票、最后写入获胜），会得到更糟的结果：系统在已知有反证的情况下继续推进。

所以冲突必须是第四种状态，并且有独立的出口：**裁决**。

当前实现用 `CheckResult::Conflicted` + `EvidenceVerdict::Conflicted` 接入选批：依赖冲突产生 `Adjudicate`，不产生 `Measure`；完成闸门单独统计 `conflicts`，不并进 `inconclusive`。

---

## 6. 准入：五道门 + 三值依赖

### 6.1 依赖门

```rust
pub fn dependency_gate(required: Ternary, evidence: Ternary) -> Ternary {
    match required {
        Ternary::False   => Ternary::True,   // 确定不需要，直接放行
        Ternary::Unknown => Ternary::Unknown, // 相关性未知，挂起
        Ternary::True    => evidence,         // 必需，看证据
    }
}
```

| `required` \ 证据 | True | Unknown | False |
|---|---|---|---|
| True（必需） | 可开始 | 挂起 | 剪枝 / 回退 |
| False（可选） | 可开始 | 可开始 | 可开始 |
| Unknown（相关性未知） | 挂起 | 挂起 | 挂起 |

这张表里最重要的是第三行。

大多数 DAG 只有“必需 / 可选”两种边，于是“可能需要的步骤”被默默当成“可选”——这就是漏掉关键检查的典型路径。三值依赖把“还不知道需不需要”显式表达出来，并要求它**挂起**，而不是赌一把。

`trit-schedule` 已经落地这个语义：`Dependency.required` 取 `True/False/Unknown`，直接 Unknown 依赖以及沿必需边传播到不动点的后继都会被 `DeferReason` 标记为延迟，而不是猜成可运行。

### 6.2 五道门 + 效果门

```rust
admission = dependencies & authorization & resources & freshness & claim & effect; // K3 AND
```

| 门 | True | Unknown | False |
|---|---|---|---|
| 依赖 | 全部满足 | 有未结算 / 过期 / 冲突 | 有必需依赖被否证 |
| 授权 | 策略证明允许 | 策略无法判定 / 缺上下文 | 策略明确拒绝 |
| 资源 | 当前可容纳 | 暂时不足 / 排队 | 永远超出上限 |
| 新鲜度 | 输入未过期 | 过期 / 未知 | 不使用 False |
| 认领 | 自己完整覆盖 | 未认领 / 过期 / 部分覆盖 | 他人有效认领 |
| 效果 | Prepared / Refuted / 无记录 | Sending / Sent / Unknown | Confirmed（终态） |

效果门只在 `select_with_effects` 中启用；`select` 保留空台账行为，兼容旧调用。任务声明的任一操作处于 `Sending/Sent/Unknown` 时，任务在**占用预算之前**就被挡下，并生成一条 `Reconcile` 义务；`dispatch_ready` 仍是最后一道 `Prepared` / epoch 校验，并逐个校验声明操作。

claim 门在 `select_with_claims` / `select_with_effects_and_claims` 中启用：`ClaimSet` 提供 owner、`now` 和任务→路径映射。任务未在映射中出现按 `Unknown` 处理，空路径视为“无需认领”；不传 `ClaimSet` 时保持旧行为，由宿主用 `admit_with_claim` 组合。

几条不能妥协的规则：

- **False 支配 Unknown**：只要有一道门明确拒绝，结论就是拒绝，不再等别的门。
- **未知不是默认放行**：未认领、未授权、未测量，统统是 Unknown，不是 True。
- **临时不足与永久超出要分开**：资源当前被占满 → Unknown（等）；需求超过容量上限 → False（改计划）。把前者当 False 会误杀，把后者当 Unknown 会永远等。
- **新鲜度不产生 False**：证据过期不能证明命题为假，只能让它回到 Unknown。
- **冲突依赖不产生 False**：依赖方证据冲突，是“无法判定”，不是“确定不满足”。给 Adjudicate 义务，不要剪枝。

### 6.3 授权策略要可证明，而不是写在 prompt 里

`trit-policy` 的做法值得直接搬进编排：策略是一张三值决策表，`assertions` 在构建期对**所有合法赋值**验证恒真，`assumptions` 限定前提成立的赋值集合。

```json
{
  "algebra": "kleene",
  "binary": ["infeasible", "contended"],
  "definitions": {
    "reject": "infeasible | (hopeless & contended)"
  },
  "assertions": [
    { "name": "硬约束", "expr": "infeasible => reject" },
    { "name": "空槽不拒", "expr": "(!contended & !infeasible) => !reject" }
  ]
}
```

这不是例子测试，是全赋值证明：对任意三值输入组合，断言都必须为 True。授权门的语义因此是“**已经证明对所有情况成立**”，而不是“提示词里写了请谨慎”。

### 6.4 选批算法

```text
validate(snapshot, revision)
budget = from_snapshot(tasks, observations, limits)
for task in priority_order:
    match phase:
        InFlight  -> hold
        Settled:
            if run_epoch != task_epoch -> 按 Pending 重新准入（旧证据作废）
            else -> settled（不可派发，但不是拒绝）
        Pending   -> admit:
            deps = K3 AND over dependency_gate(required, verdict)
            resources = capacity ? False : temporarily-unavailable ? Unknown : True
            freshness = stale ? Unknown : True
            admission = deps & authorization & resources & freshness & claim
            if admission == True:
                reserve(budget)
                selected.push(task)
```

选批只选 `True`。`Unknown` 不是“稍后再选”的同义词——它必须带着原因进入义务队列，否则下一轮还会原样卡住。

---

## 7. 义务：把 Unknown 变成工作队列

### 7.1 八种义务

| 类型 | 触发 | 动作 | 完成判据 |
|---|---|---|---|
| Measure | 缺测量 / 证据不足 | 跑最小测量、补断言 | 产生新 Evidence |
| Reconcile | 效果状态 Unknown | 查外部真实状态 | Confirmed / Refuted |
| Claim | 路径未认领 / 已过期 / 被他人占用 | 认领或续租路径 | 拿到有效 claim 与 fencing |
| Adjudicate | 证据冲突（B） | 比对出处、控制变量重跑、人裁决 | 冲突消解为 T / F |
| Rebasis | epoch 变 / 输入过期 | 换基线重新取证 | 新 epoch 下的证据 |
| Handoff | 上下文或权属转移 | 交接包 | 对方 `Acted` |
| Defer | 外部事件 / 限流 | 定时或事件唤醒 | TTL 内可重试 |
| Escalate | 计算无法决定 | 人做价值判断 / 授权 | 有授权的判定 |

除 Escalate 外，其余义务都能靠计算或协调推进；Escalate 不是。

> 当前实现已把八种义务都接入生产点：准入（Measure / Defer / Escalate / Rebasis）、
> 路径认领（Claim）、效果在途（Reconcile）、依赖冲突（Adjudicate）、交接与回执（Handoff）。
>
> 每条义务带稳定 `id`、默认 `cost`（tokens / wall / risk / human）、`owner` 与 `priority`；
> `ObligationPolicy` 负责补 `deadline`，`schedule_obligations` 在 token 与人的注意力预算内排期。

有些命题本质上不是经验命题：要不要接受这个风险、要不要发布、这个业务取舍是否值得。再多的计算也不能决定它们。系统必须能识别这类命题，并把它路由给人，而不是让模型编一个理由。

### 7.2 义务是任务，不是重试

义务和重试的区别：

| | 重试 | 义务 |
|---|---|---|
| 改变什么 | 只改变尝试次数 | 改变证据状态或问题本身 |
| 信息增益 | 通常为 0 | 目标明确：把 U 翻成 T/F |
| 副作用 | 可能重复发生 | 有幂等键与台账 |
| 失败出口 | 再试一次 | 达到上限后升级 / 换路径 |
| 记账 | 次数 | 成本、收益、deadline、owner |

同一个实验重复跑、不给新信息，就是义务抖动。义务必须有 `max_attempts`，超限后必须换类型（Measure → Escalate，Reconcile → Handoff），不能无限循环。

当前实现：`ObligationGuard` 记录每个义务的尝试次数；达到 `max_attempts` 后调度器以
`Thrashed { attempts }` 跳过。`escalation_for` 给出升级路径：Reconcile → Handoff，
其余可计算义务 → Escalate；Escalate / Handoff / Defer 本身是终点，不再自我升级。

### 7.3 义务调度

```text
score(o) = P(flip) * weight / (tokens + λ * risk + μ * human_attention)
```

这是启发式，不是定理。`P(flip)` 可以从回放历史里估：同类义务过去把 U 翻成 T/F 的比例。权重来自命题对目标的重要性。风险项给不可逆动作加权。

当前实现：`FlipRates::from_trace` 从 `ObligationFulfilled` 事件 + 同一 task 前后的
`JudgmentDerived` 估 `P(flip)`：前一条必须是 `Neither`，后一条是 `Proved/Refuted` 才算翻转。

冷启动用按 kind 的 Beta 先验 `FlipPrior`：默认 Beta(1,1)（拉普拉斯），无样本时返回先验均值。
`calibrated_from_trace` 用一段校准语料估每个 kind 的先验均值，把 `strength` 当伪计数总量；
返回的 rates 只含先验、不含观测计数，未见过的 kind 保持均匀。
`strength = 0` 或先验总强度为 0 时回退均匀，不会除零。
`ScorePolicy` 的 risk / wall / human 权重仍由宿主配置，不从语料自动校准。

分布漂移防护：`FlipRates::windowed(n)` 按 kind 只保留最近 `n` 个样本，
`windowed_from_trace` 直接按窗口入轨；窗口填满后先验占比稳定在
`strength / (n + strength)`，可用 `prior_share_bp` 观察。
窗口越短越跟得上漂移，但方差更大；这是显式取舍，不是自动调参。

`score = probability_bp × priority / cost`，cost 含 tokens / wall / risk / human；
`schedule_obligations_by_score` 和 `schedule_obligations_with_budget_by_score` 用评分排序，
截止期只做同分 tie-break。预算不足时仍跳过当前项、继续尝试后面的小项。

调度目标也要换：不是最小化 makespan，而是最小化

```text
未定质量 = Σ weight(U) + 2 * weight(Conflict)
```

因为一个“跑完但什么都没证明”的计划，makespan 再短也没有价值。

当前实现：`undecided_mass(tasks, snapshot)` 返回 `UndecidedMass { unknown, conflicts, mass }`；
每个任务权重暂按 1 计，缺证据 / 过期 / epoch 不匹配都算 U，冲突按 2 计。
`decreased_from` 用于 P4 的单调下降验收。

### 7.4 三种预算

- **执行预算**：产生动作与证据；
- **验证预算**：独立检查、断言、交叉验证；
- **对账预算**：处理在途不明的效果；
- **人的注意力预算**：需要 Escalate 的决策数量。

只给执行预算的系统，在压力下第一个砍掉的就是验证。于是它会“完成”得越来越快，也越来越不可信。三种机器预算必须分开记账；人的注意力必须限流、批处理、带截止时间。

> 当前实现：`BudgetBook` / `BudgetSplit` / `BudgetReport` 管理三池与人的预算。
>
> - 选批在 `Decision.granted_tokens` 给出每个选中任务的执行额度。
> - `reserve_for_dispatch` / `dispatch_with_execution_budget` 在派发前批量预留；派发校验失败自动释放。
> - `settle_execution` 按实际 usage 结算，未用部分释放；超支作为既成事实记录；取消用 `release_execution`。
> - `schedule_obligations_with_budget` 按义务类型扣费：`Reconcile → 对账池`，其余 `→ 验证池`。
> - 失败扣费是原子的；某一池耗尽只跳过当前项，不阻塞其他类预算。
>
> 执行池与选批的 `Limits.tokens` 分开：选批每轮重算，不能直接扣费；宿主需持久化
> `BudgetBook` 与预留记录，避免重启后重复预留。

---

## 8. 效果：不可逆动作的台账

### 8.1 状态机

```text
Prepared ──mark_sending──> Sending ──mark_sent──> Sent ──record_result(true)──> Confirmed
                                │                    │
                                │                    └──record_result(false)──> Refuted ──> Prepared'（新 attempt）
                                └── 没有可靠结果 ──> Unknown ──reconcile──> Confirmed | Refuted
```

规则：

- 只有 `Prepared` 允许真正发起外部调用；
- `Sending` 必须在调用**之前**落盘：崩溃后看到它，就知道副作用可能已经发生；
- 没有可靠结果 → `Unknown`，不是失败，也不是成功；
- `Unknown` 不允许盲重试，必须先对账；
- `Confirmed` 是终态；`Refuted` 才可重试；
- 每次尝试带单调递增的 `fence`，旧执行器不能提交。

### 8.2 派发闸门

```rust
dispatch_ready(selection, tasks, ledger) -> Vec<Attempt>
```

每个被选中的任务，其 `EffectSpec` 声明的每个操作都必须已有**同 epoch、状态为 Prepared** 的意图记录；未声明 `effects` 时沿用任务 ID。也就是说：

**选中 ≠ 可以发；发之前必须有意愿落账。**

`prepare_effects(ledger, spec, fence)` 负责原子落账：先按同一 fence 预检全部声明操作，
任一操作不可重试、fence 过期或操作重复都**不写入任何意图**，避免留下“半个任务已准备”；
预检通过后按声明顺序逐个 `prepare`，成功路径与 `dispatch_ready` 返回的 Attempt 一一对应。

选批层现在会先用 `select_with_effects` 把任一操作处于 `Sending/Sent/Unknown` 的任务挡在预算之外并产出 `Reconcile` 义务；`dispatch_ready` 仍是最后一道 `Prepared` / epoch 校验。

执行预算的预留也在这里完成：`dispatch_with_execution_budget` 先按 `Decision.granted_tokens` 预留，
再跑 `dispatch_ready`；派发失败自动 `release_execution`，派发成功后结算时用实际 usage 调 `settle_execution`。

这把“调度”和“副作用”解耦，同时防止崩溃窗口里的重复发送。

### 8.3 不可逆动作用 B3 毒化 `[建议新增]`

现有五门是 K3 AND；不可逆动作应在五门之后**再套一层** B3 毒化门：

```text
irreversible_admission =
    B3_AND(precondition_1, ..., precondition_n)
```

任何一项是 Unknown，整体就是 Unknown。没有“其他都通过了，先发吧”。普通任务仍用 K3，避免系统因为一个无关的未知项整体冻结。

适用动作：删除、force push、发布、付款、对外发消息、修改共享配置、关闭 issue/PR。这些动作的共同点不是“重要”，而是**难以撤销或影响外部**。

### 8.4 不承诺 exactly-once

分布式系统里承诺 exactly-once 通常是自欺。这里承诺的是：

- **at-most-once**：不可逆动作至多发生一次；
- **reconcile**：在途不明时查外部真实状态；
- **idempotency_key**：同一意图重复到达时去重；
- **fencing**：旧执行器的写一律拒绝。

### 8.5 重启恢复

只看落盘事实，不看内存进度：

| 落盘状态 | 恢复分类 | 动作 |
|---|---|---|
| Prepared | ready | 可安全发起 |
| Sending / Sent / Unknown | in_doubt | 必须先对账 |
| Refuted | retryable | 可重试 |
| Confirmed | done | 不再动 |

---

## 9. 协调：权属、回执、交接

### 9.1 认领与 fencing

认领是**写权限**，不是建议。

| 状态 | 含义 |
|---|---|
| Free | 没有认领 |
| Held | 有有效认领 |
| Contested | 恢复数据里出现多个有效 owner |
| Expired | 有认领，但已过期；**不等于 Free** |

认领门的结果：

| 结果 | 条件 |
|---|---|
| True | 请求路径被 owner 的有效认领完整覆盖 |
| False | 任一请求路径被别的 owner 的有效认领重叠 |
| Unknown | 未认领、已过期、或只覆盖了一部分 |

三条容易写错的规则：

- **未认领不是 True**：必须显式 claim，不能“默认没人”；
- **过期不是 Free**：过期后要写，必须重新 claim、拿新 fence；
- **窄认领不能覆盖父路径**：持有 `src/a.rs` 时请求 `src` 是 Unknown，不是 True。

fencing 单调递增。旧会话即使“复活”，也不能用旧 fence 提交。这解决的是分布式系统里的僵尸写者问题，在多智能体场景里同样常见：一个超时的 agent 醒来，拿着一小时前的上下文继续写。

### 9.2 四态回执

```text
Sent -> Delivered -> Read -> Acted
```

- 不能跳级；`Sent -> Read` 是 `ReceiptSkip`；
- 重复推进到当前态幂等；
- `Rejected` / `Expired` 是终止失败，之后不能再推进；
- 只有 `Acted` 能说“对方照做了”。

**消息发送成功只证明邮局工作，不证明对方行动。** 智能体之间的交接，最容易在这里产生虚假共识：A 发了任务，B 的进程收到了，但 B 没读、没做，A 却认为协调完成。

当前实现把这一点接进义务：`Sent / Delivered / Read` 都会产出 `Handoff` 义务，只有 `Acted` 才关闭。

### 9.3 交接包

```rust
pub struct HandoffPacket {
    pub id: String,
    pub from: String,
    pub to: String,
    pub goal: String,
    pub base_commit: String,
    pub state_hash: String,
    pub decisions: Vec<String>,
    pub open_questions: Vec<String>,
    pub claims: Vec<String>,
    pub evidence: Vec<String>,
    pub next_action: String,
    pub idempotency_key: String,
    pub expires_at: u64,
}
```

| 结果 | 条件 | 动作 |
|---|---|---|
| True | 结构合法、未过期、状态哈希一致 | 接收 |
| Unknown | 过期，或状态哈希不一致 | 先对账、重新取证 |
| False | 结构不合法（空字段等） | 拒绝 |

过期不是 False：旧线索可能仍然有用，但它不能直接当依据。结构不合法才是 False。

交接门未通过时不会停在“知道有问题”：`Unknown` 产出“先对账”的 `Handoff` 义务，`False` 产出“重新生成交接包”的 `Handoff` 义务。

### 9.4 多智能体总线仲裁

`TriStateBus` 的语义可以直接用来做多智能体写入仲裁：

- 每个 agent 是一个驱动器；
- `HighZ` = 没有主张；
- 同值 = 一致；
- 相反值 = 冲突；
- `resolve_drivers` 会收集**全部**冲突车道和**全部**驱动方，而不是遇到第一个就报错。

```text
lane "commit:abc/test":
  alice: enable=true,  drive=High   // 通过
  bob:   enable=true,  drive=Low    // 失败
  => 冲突；drivers = [(alice, High), (bob, Low)]
```

这个模式适用于三类事实：

- **事实车道**：同一个 commit 的测试结论、同一个输入的性能数据；
- **权属车道**：同一路径的写权限；
- **决策车道**：批准 / 拒绝。

关键不是“解决冲突”，而是**先把冲突完整地呈现出来**。多数投票会在这一层丢掉少数派的证据；最后写入获胜会丢掉更早但更可靠的证据。两种做法都让系统在冲突面前假装确定。

硬件里这叫总线争用，编排里这叫证据冲突。处理方式相同：不静默合并，交给裁决。

当前实现：`resolve_lanes` 接收每条车道的 `LaneDriver`；`HighZ` 弃权、同值一致，
相反值冲突时 `LaneDispute` 列出全部非弃权驱动方。`LaneState::evidence_verdict` 把结果映射到
`Proved / Refuted / Neither / Conflicted`，可直接接到冲突裁决与义务路由。

---

## 10. 上下文也是副作用

长上下文压缩不是性能优化，是**对智能体工作记忆的不可逆写入**。摘要一旦覆盖原文，被丢掉的约束就再也找不回来。

因此压缩必须过闸，而不是“感觉太长了就压一下”。

| 档位 | 动作 | 放行条件 |
|---|---|---|
| Keep | 不动 | 默认 |
| Prune | 删除可回读 / 可重取的工具输出 | `recall_reachable == True` |
| Summarize | 有损改写历史 | `durable_flushed == True` 且 `constraints_pinned == True` 且 `progress_since_compaction == True` |

几条硬规则：

- 测不到 token 数 → Unknown，不压；
- 供应商报溢出但实测不足半窗 → 按误报处理，不动历史；
- 实测与报告矛盾 → Unknown，先对账；
- 承重事实没有落盘副本 → 不许摘要；
- 用户原话、红线约束不在稳定前缀 → 不许摘要；
- 上次压缩后没有新的落盘进展 → 再压就是压自己的摘要。

`trit-orch::context::compaction_gate` 已经把这套闸门落地，并且把每个 block 映射成一条最小义务（MeasureTokens / ReconcileWindow / VerifyRecall / FlushFacts / PinConstraints / RecheckAfterProgress）。

这给了一个更一般的模式：**任何有损优化，都应该按副作用处理**。压缩、缓存、采样、截断，都不是免费的。

---

## 11. 完成语义

### 11.1 完成闸门

```rust
pub fn finish(&self, tasks: &[TaskSpec], snapshot: &Snapshot) -> Result<Summary, Error>
```

只有同时满足以下条件的判定，才计入完成：

- `phase == Settled`；
- `epoch` 与当前任务定义一致；
- 输入 `freshness == True`；
- 证据覆盖足够，且结论确定（T 或 F）。

其余一律拒绝，并给出计数：

```text
Incomplete { pending, in_flight, inconclusive }
```

`Summary` 只有 `confirmed` / `refuted` / `rounds`，没有 `done: bool`。这是有意的：**完成不是一个布尔值，是一组可核对的计数。**

### 11.2 七种“未完成”

二元系统只有一个 `error` 桶。三值编排必须把它们分开，因为出口完全不同。

| 现象 | 三值位置 | 正确动作 | 老系统常做 |
|---|---|---|---|
| 命题被否证 | F | 剪枝 / 回退 | 重试（错） |
| 证据不足 | U | 补证义务 | 重试 / 猜（错） |
| 效果在途不明 | Effect::Unknown | 对账 | 重发（危险） |
| 证据冲突 | Belnap B | 裁决 | 多数投票 / 最后写入（丢信息） |
| 证据过期 / 换 epoch | stale U | 换基 / 重取 | 沿用旧结论（错） |
| 权属争议 | Claim Contested | 协商 / 等待 | 同时写（冲突） |
| 门被拒绝 | False gate | 改计划 | 重试（错） |

这张表是整套设计的实用价值所在：**它把“出错了”拆成了七种可操作的情况。**

### 11.3 对外报告

不要报“成功 / 失败”。报六个数字：

```text
confirmed   = 已被证明
refuted     = 已被否证
undetermined= 证据不足
conflicts   = 证据冲突
in_doubt    = 副作用在途不明
stale       = 证据过期
```

一个不能说出“我不知道”的编排器，最终一定会说谎。

---

## 12. 循环

```text
观测 → 派生 → 准入 → 选批 → 预备 → 派发 → 回执 → 结算 → 生义务 → 补丁 → 观测
```

| 阶段 | 输入 | 输出 | 失败时 |
|---|---|---|---|
| 观测 | 回执、日志、外部状态 | Snapshot | 标记 Unknown，不猜 |
| 派生 | 判定 + 证据 | Verdict + 证书 | 保持 U |
| 准入 | 五道门 | T / U / F | U 生成义务，F 剪枝 |
| 选批 | 优先级 + 预算 | Selection | 拒绝原因入账 |
| 预备 | Selection + 台账 | Prepared Attempt | 缺意图则拒发 |
| 派发 | 宿主桥 | 外部调用 | 无可靠结果 → Unknown |
| 回执 | 执行器 | CheckReport | 零断言 / 超时 → U |
| 结算 | 回执 + epoch | Evidence | epoch 不符 → 作废 |
| 生义务 | 所有 U / B / 在途 | Obligation 队列 | 达上限 → 升级 |
| 补丁 | 义务 + 新计划 | Plan revision + 1 | 基线不符 → 拒绝 |

两个工程约束：

- **内核不读系统时钟**：`now` / `ttl` 由调用方传入，保证确定性测试与回放；
- **回放重算，不信历史结论**：每一步用当时的 plan / snapshot / limits / ledger 重新跑内核，策略只能提议，不能改内核决定。

当前进度：选批、义务调度/评分、预算与升级已落地；`生义务 → 补丁` 已实现最小切片：

- `ObligationTaskPolicy` + `plan_patch_for_obligations` 处理全部八种 kind。
- 目标必须是 Pending：生成 `ob:<obligation.id>` 任务，并在目标上补一条必需依赖。
- InFlight、非 Pending、目标缺失、生成任务已存在都进入 `deferred`。
- 原生绑定：生成任务带 `resolves = [目标任务]`、`produces`（策略默认 1 条断言）与空
  `effects`；`Plan::resolvers_of` 可按目标反查证据生产者。
- 效果绑定：任务可用 `EffectSpec` 声明一个或多个台账操作；未声明时回退到任务 ID。
  任一操作在途挡下整任务，派发要求每个操作都有 Prepared 意图；同任务重复操作在
  `Plan::new` 拒绝。
- 资源映射：`requirements_by_kind` 按义务类型覆盖默认资源；token 仍取 `cost.tokens`。
- 路径 / 认领映射：`plan_patch_for_obligations_with_claims` 从 `ClaimSet` 读取目标路径，
  排序去重后写入 link；Claim 生成任务不得再用同一组路径做 claim gate（会自锁）。
  缺少路径的 Claim 义务以 `NoClaimPaths` 延迟，而不是生成空任务。
- `PlanPatch` 仍负责 base_revision、in-flight 保护、epoch bump、ID 去重与环校验。
  完全相同的替换（除 epoch 外全字段一致）不 bump epoch，已有证据不会被无意义作废。
- `writeback_obligation_evidence` 按 `ObligationTaskLink` 回写：要求生成任务 Settled、
  证据 epoch 与生成任务一致且满足 `produces.min_assertions`；rebase 到目标 epoch 后
  写入目标双轨，按 source 幂等；未满足条件时以 `EvidenceWritebackDeferReason` 延迟。

仍缺：跨主机租约与执行器协议；这些属于开放问题，不再是 `TaskSpec` 结构缺口。
多源 Belnap 双轨已落地。

```rust
pub struct TickInput<'a> {
    pub plan: &'a Plan,
    pub snapshot: &'a Snapshot,
    pub limits: &'a Limits,
    pub ledger: &'a Ledger,
    pub claims: Option<&'a ClaimSet<'a>>,
    pub briefed: &'a [TaskBrief],   // 上一轮发出的简报
    pub trace: Option<&'a TraceLog>,
    pub since_seq: u64,
}

pub struct Tick {
    pub revision: u64,
    pub selection: Selection,
    pub briefs: Vec<TaskBrief>,     // 本轮选中任务
    pub adherence: Vec<AdherenceReport>,
    pub obligations: Vec<AdmissionObligation>,
}

pub fn tick(input: TickInput<'_>) -> Result<Tick, Error>
```

主循环做四件事：选批（效果门 + 可选 claim 门）→ 为选中任务生成简报 →
对上一轮简报做遵循度检查（epoch 已过期的简报跳过，不算偏离）→
用 `InterventionPolicy` 收敛成跨任务干预计划，并把选批义务与干预义务按 ID 去重汇总。

同时输出本轮 `guidance_metrics`（简报数、有违规任务数、三类违规计数、比率），
宿主可跨 tick 继续 `observe` 得到长期遵循率。

派发不在 tick 内：宿主读取 `briefs`，用 `prepare_effects` 落意图，再调用
`dispatch_ready`；tick 不猜执行结果。`briefed` 的 `held` 任务在解除前不应重新派发；
解除由宿主决定，内核不持久化 held 状态。`trace` + `since_seq` 用来识别“简报之后”的
效果 / 回执；不传轨迹时只做状态检查。tick 也不调度义务，预算内排期仍走
`schedule_obligations_*`。

---

## 13. 可观测性

> 轨迹与时序的完整模型（三套时钟、因果边、回放差异、轨迹不变量）见 [TRACE-AND-TIME.md](TRACE-AND-TIME.md)。

### 13.1 轨迹

```rust
pub struct TraceStep {
    pub revision: u64,
    pub tasks: Vec<TaskSpec>,
    pub snapshot: Snapshot,
    pub limits: Limits,
    pub ledger: Ledger,
}
```

每一步落一行 JSONL。回放时重新计算内核决定，而不是相信当时记录的“结论”。这样既能复现 bug，也能安全地评估新策略。

### 13.2 指标

| 指标 | 定义 | 目标 |
|---|---|---|
| `undecided_mass` | `Σ weight(U) + 2 * weight(Conflict)` | 单调下降；`mass.rs` 已落地 |
| `decision_rate` | 每 1k tokens 产生的确定判定数 | 越高越好 |
| `false_completion_rate` | 报告完成但存在 U / B / 在途的比例 | **必须为 0** |
| `stale_ratio` | 过期证据占全部证据的比例 | 越低越好 |
| `conflict_ratio` | B 判定占已结算判定的比例 | 观察，不追零 |
| `in_doubt_age` | 在途不明效果的最长年龄 | 有上限 |
| `reconcile_backlog` | 待对账效果数 | 有上限 |
| `obligation_fulfillment` | 义务把 U 翻成 T/F 的比例 | 越高越好 |
| `obligation_thrash` | 同一义务重复无信息尝试的比例 | 越低越好 |
| `shadow_agreement` | 影子策略与内核一致步数比例 | 观察；`shadow_learn.rs` 以 basis points 计算 |
| `cache_hit_rate` | `cached / (fresh + cached)` 输入 token 占比 | 观察；`TokenUsage` 已落地 |
| `concurrency_pressure` | 已占 worker / worker 上限 | 观察；`OrchestrationMetrics` 已落地 |
| `selection_pressure` | 选中任务 / 评估任务 | 观察；过高说明预算或准入过紧 |
| `guidance_violation_rate` | 有违规的简报占比（基点）；`GuidanceMetrics::violation_rate_bp` 已落地 | 观察，暴露宿主偏离而不伪造确定性 |
| `unsafe_extra` | 影子策略建议危险额外派发的次数 | **必须为 0**；非零候选直接排除 |

### 13.2.1 编排指标快照

`OrchestrationMetrics::capture(&tick, &snapshot, &limits)` 只读一次 tick 的状态，输出：

- 规模 / 并发：`tasks_total / selected / unknown / denied / in_flight`、`workers_limit / occupied`、
  `concurrency_pressure_bp`、`selection_pressure_bp`；
- token / 缓存：`tokens_fresh / cached / output`、`cache_hit_rate_bp`、
  `observed_tokens / reserved_tokens / granted_tokens`、`reserved_resources`；
- 引导 / 义务：`briefs`、`adherence_reports`、`violations`、`interventions`、`held`、
  `obligations`、`guidance_violation_rate_bp`，以及按稳定短名聚合的 `blockers` 计数。

`to_prometheus()` 导出 `trit_orch_*` gauge，`to_prometheus_with(prefix)` 可改前缀。
指标不参与决策；墙钟耗时由宿主在外部测量，内核保持确定性。

**影子学习（P5.1 / P5.2）**：`shadow_learn.rs` 在回放语料上逐策略评估，
只推荐 `unsafe_extra == 0` 的候选；排序按一致率、漏派数、顺序分歧、额外建议、停止建议，
最后按策略名定序，因此同语料同候选集合的结果确定。
语料为空、候选为空、无安全候选时 fail-closed，返回 `Deferred`（附全部评估）。
未知内置策略名返回 `Error::UnknownShadowPolicy`；学习只产出推荐，不替代内核决定。

`ShadowLearner` 做增量长期评测：跨批次累积同一组候选的 `ShadowReport`，
合并时平移批次内序号，因此累计结果与“拼接语料直接回放”一致；
安全历史不会被新批次洗掉（一次 `unsafe_extra` 即永久排除）。
同一学习器只观察同一组策略：候选集合变化或单批重复策略名都 fail-closed，
需要 `reset` 后重新累积。

跨宿主合并走 `ShadowLearner::merge`：两边策略集合必须一致，否则
`LearnPolicySetChanged`；合并后的累计报告等价于“先回放本机语料、再回放对方语料”。
合并假设两边语料互不重叠，重复合并同一批语料会重复计数。
空学习器合并时直接采纳对方状态。

加权入口 `observe_weighted` / `observe_builtin_weighted` / `merge_weighted`：
权重 `w` 等价于把该批语料回放 `w` 次，因此仍然与重复回放一致；`w = 0` 是 no-op。
安全历史同样按权重累积，不会被加权稀释。

长期窗口是显式 opt-in：`ShadowLearner::windowed(n)` 只保留最近 `n` 步，按整批丢弃
（实际保留不超过窗口）。与默认累计模式不同，窗口模式的安全历史也会随旧批次遗忘；
需要永久安全记忆时不要使用。窗口配置不一致的合并 fail-closed（`LearnWindowMismatch`），
零窗口拒绝（`LearnWindowZero`）。窗口内合并等价于对拼接后语料保留最后 `n` 步。

### 13.3 证书即审计

每个 Verdict 带证据 ID、epoch、观察时间、缺失边、义务 ID。审计时不需要重读模型日志，只需要看证书：

- 为什么是 True？——这 3 条断言、这个回执、这个 commit。
- 为什么是 False？——这个反例 / 这个失败断言。
- 为什么是 Unknown？——缺这条边，义务是 Measure #17。
- 为什么是 B？——这两个来源冲突，义务是 Adjudicate #23。

---

## 14. 与现有编排的关系

| 维度 | 工作流引擎 | 图编排 | 多智能体对话 | 本模型 |
|---|---|---|---|---|
| 节点 | 活动 | 动作 | 消息 | 判定 |
| 边 | 先后 | 控制流 | 对话 | 证据依赖（T/F/U） |
| 未知 | 异常 / 超时 | 分支 | 沉默 / 幻觉 | 一等值 + 义务 |
| 副作用 | 幂等重试 | 节点重跑 | 消息重发 | 台账 + 对账 |
| 完成 | 所有活动结束 | 图走到 END | 对话结束 | 所有判定确定 |
| 冲突 | 最后写入 | 分支覆盖 | 多数 / 忽略 | Belnap B + 裁决 |
| 协调 | 队列 | 共享状态 | 消息 | 认领 + 四态回执 |

这不是替代关系，是叠加关系。工作流引擎擅长执行和重试，图编排擅长控制流，多智能体框架擅长通信；本模型补的是**判定语义**：

- 一个节点什么时候算“可以开始”；
- 一个结论什么时候算“成立”；
- 一个副作用什么时候算“发生过”；
- 一个计划什么时候算“完成”。

完全可以把它包在 Temporal 的 activity 外面，或者作为 LangGraph 节点之间的判定层。

---

## 15. 安全边界与非目标

**非目标：**

- 不做分布式共识；单机内存 + 宿主持久化；
- 不做强杀；执行层用协作取消与检查点；
- 不承诺 exactly-once；只承诺 at-most-once + reconcile；
- 不做跨智能体协议与能力协商；
- 不替人做价值判断；只负责识别并升级；
- 不承诺模型更聪明；这套东西管的是认知卫生，不是智力。

**安全边界：**

- 认领表是权限，不是建议；gate 不通过就不能写；
- 授权策略构建期可证明，不依赖 prompt；
- 不可逆动作必须 Confirmed，且要快照 + 人确认；
- 不可逆门的任何 Unknown 都毒化整个动作；
- secret 只放引用，不放进 `intent` / `payload`；
- 影子策略只能提议，不能派发、不能改集合、不能绕过台账；
- 回执、效果、证书都要可追溯，不能“只记最后状态”。

---

## 16. 落地路线

| 阶段 | 内容 | 验收标准 |
|---|---|---|
| P0（已在仓库） | 三值值 / 代数 / 拓扑证书 / 三值依赖调度 / 五门准入 / 认领 / 回执 / 效果台账 / 完成闸门 / 压缩闸门 / 影子回放 | 现有测试与变异门通过 |
| P1 | 把义务做成一等对象：每个 U 必须产出至少一条义务；Verdict 统一带证书 | 不存在无义务的 U；完成闸门拒绝 U；T/F 行为不变 |
| P2 | Belnap 冲突路由：B 单独计数、单独出口、禁止最后写入 / 多数投票 | 冲突不再被压成 U；每次 B 都有 Adjudicate 义务 |
| P3 | 多智能体总线仲裁：`resolve_lanes`，冲突车道列出全部驱动方 | 争用可诊断；无静默合并 |
| P4 | 三种预算 + 义务调度（信息增益 / 成本） | 验证支出可见；义务抖动有上限；未定质量单调下降 |
| P5 | 影子学习：用回放语料评估策略，unsafe_extra 必须为 0 | 策略只提议；危险建议为零；可回放复现 |

P1 是最小可用的分水岭。在 P1 之前，系统仍然能跑，但 Unknown 只是一个标签；在 P1 之后，Unknown 才真正变成工作队列。

当前进度：P0–P3 已完成；claim 门也已接入内核选批（`select_with_claims` / `select_with_effects_and_claims`）。
P4 已完成：成本 / owner / deadline、三池预算与义务扣费、执行预算派发预留、
`P(flip)` 回放估计与信息增益/成本评分、义务抖动上限、未定质量单调下降验收均已落地。
P5.1 已完成：影子回放 + `unsafe_extra` 红线之上，接入安全门控的策略评估与推荐
（`shadow_learn.rs`）。P5.2 已完成：`ShadowLearner` 跨批次累积、与拼接语料回放等价、
安全历史不遗忘、候选集合变更 fail-closed。P5.3 已完成跨宿主合并：策略集合不一致
fail-closed，合并结果与拼接语料回放一致，空学习器直接采纳对方状态。
P5.4 已完成策略权重：加权观测 / 加权合并等价于重复回放，零权重为 no-op。
P5.5 已完成长期窗口：`windowed(n)` 显式 opt-in、整批丢弃、安全性随窗口有界，
窗口不一致合并 fail-closed。
冷启动与先验校准已落地：`FlipPrior` 支持按 kind 设置 Beta 先验，
`calibrated_from_trace` 用校准语料估先验均值与强度；`ScorePolicy` 权重仍由宿主配置。
漂移防护已落地：`FlipRates::windowed` / `windowed_from_trace` 按 kind 截断历史，
`prior_share_bp` 暴露先验占比；窗口长度仍由宿主选择。
引导面已落地：`TaskGuidance` 随任务定义版本化；`guidance_from_text` 做确定性规则抽取，
`render_brief` 渲染稳定提示，`task_brief` 合成只读契约；`check_adherence` 对齐观察与轨迹，
`GuidanceMetrics` 聚合遵循率，`plan_interventions` 收敛分级干预，`tick` 串起全链路。
语义抽取、干预效果度量与学习型策略未接。
指标层已落地：`OrchestrationMetrics::capture` 汇总并发、token、缓存命中、选批压力、
引导与义务计数，并支持 `trit_orch_*` Prometheus 导出；墙钟耗时仍由宿主测量。

结构性缺口已闭合：义务 → PlanPatch 覆盖全部八种 kind + Pending 目标 + 自动证据回写 +
路径 / 认领 / 资源映射；`TaskSpec` 的 `resolves/produces/effects` 原生绑定全部落地，
多源 Belnap 双轨已落地。剩余问题是开放问题里的跨主机租约与执行器协议。
交接 / 回执义务、轨迹事件流与不变量见 [TRACE-AND-TIME.md](TRACE-AND-TIME.md)。

---

## 17. 开放问题

1. **Ł3 权重怎么校准**：两票独立性的假设如何验证？同模型、同上下文的两票不能算独立。
2. **信息增益怎么估**：已支持按 kind 的 Beta 先验、校准语料估先验、按 kind 窗口入轨与先验占比观测；仍需验证窗口长度如何随漂移自动调整，以及 `ScorePolicy` 权重是否该自动校准。
3. **冲突裁决的权威从哪来**：第三个执行器？更可靠的来源？还是人？需要一套出处可信度模型。
4. **人的注意力怎么定价**：Escalate 太多会淹没使用者；太少会让系统在不可判定的命题上空转。
5. **epoch 抖动的代价**：同定义替换已不再 bump epoch；仍需限制“有意义但高频”的变更反复作废证据。
6. **跨主机协调**：不做共识的前提下，如何避免两个宿主同时认领同一路径？可能需要外部租约服务。
7. **指标归一化**：不同证据类型的 `weight` 如何比较？一条测试断言和一次人工确认不等价。
8. **何时该 Defer 而不是 Escalate**：等待外部事件的成本，可能高于直接问人。
9. **引导是否被执行**：单任务遵循度检查与跨任务干预计划已落地；仍需干预效果度量、
   解除 held 的状态机，以及“发现偏离后如何干预”的学习型策略。

---

## 18. 自我审计（2026-11）

**这套东西是什么、不是什么。** 它是一个确定性的编排内核：准入、证据、义务、效果、引导与干预都是显式数据；它不是智能体运行时——不渲染提示、不调用模型、不执行任务。宿主负责把 `TaskBrief` 变成 Agent 输入，把回执变成 `Observation`。

**已经站得住的：**

- 未知是一等状态：U 必产义务，完成闸门拒绝 U/B/在途，效果不明先对账。
- 原生绑定：`resolves / produces / effects / guidance` 随任务定义版本化；在途保护覆盖全部字段。
- 引导是只读契约：brief 不改变准入；遵循度检查不改变内核状态；干预只产出计划。
- 回放确定性：选批、补丁、学习器、tick 都不读系统时钟，不依赖随机数。
- 运行时契约的双面：预防（依赖/授权/资源/新鲜度/claim/效果门）与证据（produces、完成闸门、证据回写、遵循度）都在内核里，和 “Agent Safety Should Be a Runtime Contract” 的主张同向。

**还不敢说站得住的：**

1. `TaskGuidance` 的抽取仍是规则 fallback：`guidance_from_text` 只认标题与关键词，语义抽取与提示质量未评估；`stop_when` 不是内核强制条件，`guidance_violation_rate` 只有宿主累计口径。
2. 遵循度是启发式：状态版分不清“简报前已存在的在途”和“简报后新行动”；断言计数用 max / total，可能掩盖某个来源的缺口；`ObligationsIgnored` 依赖宿主主动写 `ObligationFulfilled`，漏记就误报。
3. 干预只是计划：`held` 不进入 Plan / Snapshot，宿主可以无视；没有解除状态机、没有干预效果指标，也没有跨任务根因分析（共享效果 / 路径 / owner 目前不聚合）。
4. 记忆语义不统一：`ShadowLearner` 窗口模式会忘掉旧的安全事件，而遵循度与效果台账永久保留；跨子系统需要一套保留策略。
5. 原生字段部分仍是元数据：`resolves` 只被 `resolvers_of` 与 link 回写使用，选批 / 完成闸门仍按任务级判定；`produces` 只在回写与遵循度用到。
6. 分布式空白：claim / ledger / trace 都是单主机内存模型，没有跨主机租约与执行器协议。
7. 证据质量维度缺失：除了 checks 计数与 source 双轨，没有权重、可信度、时间衰减；“一条测试断言 vs 一次人工确认”仍不可比。
8. 没有端到端基准：单元 / 集成测试覆盖语义边界，但没有真实 agent 轨迹、没有延迟 / 成本基准、没有和 AgentSpec / ProbGuard 类系统的对照实验。

**方向上的自我批评：**

- 把“引导”做成结构而不是提示，避免 prompt 漂移，但也把理解成本推给宿主；如果宿主不消费 `TaskBrief`，整个引导面就是死数据。
- 三值 / 四值判定让“未知”可审计，但所有下游动作都变成显式义务，系统可能过度保守：高吞吐场景下 Reconcile / Claim 义务的调度成本可能压过收益，目前没有量化。
- 固定规则干预可预测、可回放，但面对新失败模式会漏；学习型 steerer 能泛化，却破坏确定性。折中应是“策略只提议、内核 gate 兜底”，目前只完成了前半句。

---

## 附录 A：概念与现有代码的对应

| 概念 | 现有落点 | 状态 |
|---|---|---|
| 三值值与 K3/Ł3/B3 | `crates/trit-core/src/value.rs`、`algebra.rs` | 已落地 |
| Belnap 四值 / 双轨 | `crates/trit-belnap/src/lib.rs`（`B4`、`from_rails`、`consensus`、`gullibility`） | 已落地，opt-in |
| 多源双轨证据 | `evidence_rails.rs`（`EvidenceRails` / `Observation.evidence_rails`） | 已落地：support / counter / inconclusive；Conflicted 进两轨；选批与完成闸门优先使用 |
| 三值拓扑与证书 | `crates/trit-core/src/topology.rs`（`ReachEvidence`、`unknown_edges`） | 已落地 |
| 三值依赖与 Unknown 延迟 | `crates/trit-schedule/src/lib.rs`（`Dependency`、`DeferReason`） | 已落地 |
| 五门准入 / 依赖门 | `admission.rs` / `selection.rs` / `plan.rs`（`select_with_claims`） | 已落地：依赖/授权/资源/新鲜度/claim 可选接入，再加效果门 |
| 选批与预算 | `selection.rs`、`budget.rs` | 已落地 |
| 认领 / fencing / TTL | `claim.rs` | 已落地 |
| 四态回执 / 交接包 | `receipt.rs` | 已落地 |
| 效果台账 / 对账 / 恢复 | `effects.rs` | 已落地 |
| 任务原生绑定 | `model.rs`（`TaskSpec.resolves/produces/effects/guidance`）、`plan.rs`（`resolvers_of`、`same_spec`、重复操作校验）、`effects.rs`（`EffectSpec`、`effect_operations`、`prepare_effects`）、`guidance.rs`（`guidance_from_text`）、`brief.rs`（`TaskBrief`、`render_brief`）、`adherence.rs`（`check_adherence` / `GuidanceMetrics`）、`intervention.rs`（`plan_interventions`）、`tick.rs`（`TickInput` / `Tick` / `tick`） | 已落地：多重操作在途挡整任务；原子批量准备；派发逐个校验 Prepared；重复操作 Plan 构建期拒绝；在途保护覆盖全部原生字段；简报生成 / 渲染 / 遵循度 / 跨任务干预 / 主循环；规则抽取为 fallback |
| 完成闸门 | `progress.rs` | 已落地 |
| 压缩闸门与义务映射 | `context.rs` | 已落地 |
| 影子回放 | `shadow.rs`、`policy.rs` | 已落地 |
| 影子学习（P5.1–P5.5） | `shadow_learn.rs` | 已落地：语料评估、安全门控推荐、Deferred fail-closed、确定性排序；`ShadowLearner` 增量累积、与拼接回放等价、跨宿主合并、加权观测/合并（等价重复回放）、长期窗口（opt-in、整批丢弃、安全性有界） |
| 计划补丁与 epoch 作废 | `plan.rs`、`model.rs` | 已落地：base_revision、in-flight 保护、epoch bump、同定义替换不 bump epoch、ID 去重、环校验 |
| 义务 → PlanPatch | `obligation_plan.rs`（`ObligationTaskPolicy` / `plan_patch_for_obligations` / `plan_patch_for_obligations_with_claims` / `ObligationTaskLink`）、`evidence_writeback.rs` | 已落地：八种 kind + Pending 目标 + 原生 `resolves/produces` + link 回写（epoch rebase、`min_assertions` 下限、按 source 幂等）+ 按 kind 资源覆盖 + 认领路径映射（缺路径的 Claim deferred） |
| 授权策略的构建期证明 | `crates/trit-policy/src/lib.rs` | 已落地 |
| 义务作为一等对象 | `src/obligation.rs`（`AdmissionObligation`、`ObligationKind`、`obligations_for`） | 已落地：八种义务都有生产点，带 cost / owner / deadline / EDF 排期；统一 Verdict 证书未单独建模 |
| claim 接入选批 | `claim.rs`（`ClaimSet`）、`plan.rs`（`select_with_claims`）、`selection.rs` | 已落地：未声明/过期/未覆盖 → Unknown + Claim 义务；他人有效认领 → False |
| 效果门（在途不重发） | `selection.rs`（`effect_gate`）、`plan.rs`（`select_with_effects`） | 已落地：按 `EffectSpec` 逐操作检查；Sending/Sent/Unknown → Reconcile；Confirmed → 拒绝重发 |
| 冲突路由（Belnap B → 裁决） | `model.rs`（`CheckResult::Conflicted`、`EvidenceVerdict`）、`selection.rs`、`progress.rs` | 已落地：依赖冲突 → Adjudicate；完成闸门单独计 conflicts |
| 交接面义务 | `receipt.rs`（`handoff_obligation`、`receipt_obligation`） | 已落地：哈希不一致/过期/结构不合法/未 Acted → Handoff |
| 义务成本 / 归属 / 截止期 / 排期 | `obligation.rs`（`ObligationCost`、`ObligationPolicy`、`schedule_obligations`）、CLI `obligations` | 已落地：成本 / owner / deadline / EDF 排期；评分版见下一行 |
| 轨迹与时序 | `shadow.rs` / `trace.rs` / `trace_store.rs` / `trace_merge.rs` / `trace_invariants.rs` | T1–T5 已完成：事件流、双时间、因果、回放、指标、脱敏、HLC 归并、不变量；详见 [TRACE-AND-TIME.md](TRACE-AND-TIME.md) |
| 编排指标 | `orch_metrics.rs`（`OrchestrationMetrics`） | 已落地：并发、token、缓存命中、选批压力、阻塞原因、引导 / 义务计数；Prometheus gauge 导出；墙钟耗时由宿主测量 |
| 多智能体总线仲裁 | `src/lanes.rs`（`resolve_lanes`、`LaneState`、`LaneDispute`） | P3 已完成：HighZ 弃权、同值一致、冲突列出全部 active 驱动方 |
| 三种预算 | `budget_book.rs`（`BudgetBook` / `BudgetSplit` / `BudgetReport`、`schedule_obligations_with_budget`、`dispatch_with_execution_budget`） | 已落地：三池 + 人的预算；执行预留/结算/释放 + 义务扣费；评分版调度已接入 |
| 未定质量 | `mass.rs`（`UndecidedMass` / `undecided_mass`） | 已落地：U 计 1、Conflict 计 2；缺证据/过期/epoch 不匹配按 U；`decreased_from` 验收 |
| 义务调度（信息增益） | `flip_rates.rs`（`FlipRates` / `FlipPrior` / `ScorePolicy`、`schedule_obligations_by_score`） | 已落地：从回放估 `P(flip)`，按 kind 的 Beta 先验与校准语料估先验，按 kind 窗口截断历史并暴露先验占比，评分 = P × 优先级 / 成本；EDF 仅作同分 tie-break；`ScorePolicy` 权重仍为宿主配置 |
| 义务抖动上限 | `obligation_guard.rs`（`ObligationGuard` / `escalation_for` / `schedule_*_with_guard`） | 已落地：`max_attempts`、`Thrashed` 跳过；Reconcile → Handoff，其余 → Escalate |

> 说明：`trit-orch` 的 README 标注当前来自 `feat/orchestration-core` 分支；合并状态以仓库为准。本文的“已落地”指该工作区中可读到的实现，不代表已发布到稳定版。

---

## 附录 B：一个小例子

任务：修改登录逻辑，并确认没有回归。

1. `impl-login` 认领 `src/auth.rs`，修改并提交。
2. `verify-login` 依赖 `impl-login`（required = True），跑 12 条断言，全部通过 → 判定 T。
3. `verify-no-regression` 跑全量测试，超时，零断言 → `Inconclusive` → 判定 U。
   - 不重试同一条命令；生成 Measure 义务：先跑受影响子集，再决定是否扩到全量。
4. `open-pr` 是效果任务：
   - `prepare` 落账 → `Sending` → 网络断开 → `Unknown`；
   - 不重发；生成 Reconcile 义务，查 PR 是否已创建；
   - 查到已创建 → `Confirmed`；查到不存在 → `Refuted`，才允许新 attempt。
5. 两个执行器对同一 commit 的测试结论一通过一失败 → Belnap B；
   - 不投票；生成 Adjudicate 义务：比对运行环境、测试数据哈希、失败日志；
   - 必要时 Escalate 给人。
6. 完成闸门发现还有 1 个 U、1 个 B、1 个 in_doubt → 拒绝报完成；
   - 对外报告：`confirmed=1, refuted=0, undetermined=1, conflicts=1, in_doubt=1, stale=0`。

这就是“调度判定”和“调度动作”的区别：第 2 步之后，任务并没有变多，但系统已经知道**自己还不知道什么**，并且知道下一步该补哪条边。

---

## 附录 C：术语

| 术语 | 含义 |
|---|---|
| 判定（Judgment） | 一个需要被证明或否证的命题 |
| 证据（Evidence） | 带出处、epoch、时间与覆盖数的检查结果 |
| 义务（Obligation） | 把 Unknown 变成可判定结论的最小动作 |
| epoch | 命题 / 任务定义的版本；变更即旧证据作废 |
| freshness | 证据是否仍在保质期内 |
| fence | 单调递增的执行器世代号，防旧写者 |
| in-doubt | 副作用可能已发生但结果不明 |
| B（Belnap both） | 同时存在支持与反对证据的冲突状态 |
| 未定质量 | 未定与冲突判定的加权总量 |
| 影子策略 | 只提议、不执行的候选调度策略 |
