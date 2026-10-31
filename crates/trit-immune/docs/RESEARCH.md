# 研究纪要：外部技术与本系统的咬合点

> 2026-10-31 研究轮：六个方向、每个都回答同一个问题——**这个技术和三进制免疫
> 系统的语义咬合吗？** 咬合才落地，不咬合只记账（防为学术而学术）。
> 状态：✅ 已落地 / 🔜 排队（含触发条件） / 📝 记账（明确不做及原因）。

## 一、Belnap 四值逻辑 → 矛盾检测器 ✅

**来源**：Belnap 1977 "How a computer should think"；Arieli & Avron, *The value of
the four values*, Artificial Intelligence 1998（被引 385+）。FOUR =
{T, F, U(无信息), ⊥(both/矛盾)}——为多来源信息库设计，"both" 显式表示矛盾。

**咬合点**：K3 AND 折叠把矛盾压成坏消息支配（红队 `-1` + CI `+1` → `-1`），
但**矛盾本身是信息**——同一节点被两个来源判出相反结论，恰恰是"有人错了"的
信号，值得人看。账本（VerdictEvent 历史）已经存了矛盾证据，只是没有视图消费。

**落地**：`dag::contradictions()`——历史既有 `-1` 又有 `+1` 且未隔离的节点，
输出 0 态候选队列。不改 Trit 内核（K3 语义不动），是账本之上的**增量视图**——
Belnap 的 ⊥ 不进存储、只在查询时从账本派生。隔离节点不重复上报（已有人看）。

## 二、攻击图（Sheyner 系）→ 打穿来源回溯 ✅

**来源**：Sheyner et al. "Tools for Generating and Analyzing Attack Graphs"
（模型检查生成攻击图，被引 365+）；Palma et al., Computers & Security 2025 综述。
攻击图 = 攻击者可达路径的图模型，多步攻击链分析的标准形态。

**咬合点**：系统已有"分支即攻击向量" + (min,max) 传播（**会不会被打穿**），
缺的是"**从哪打穿的**"——渗透路径归因。merge_gate 折叠的来源（因果链祖先 /
锚定攻击分支 / 跨树边）本身就是攻击路径的三种形态。

**落地**：`dag::breach_sources(target) -> Vec<(NodeId, BreachKind)>`——列出全部
把 target 拉向 `-1` 的来源，`BreachKind` 标注路径种类（Ancestor/AnchoredAttack/
CrossLink）。与 merge_gate 折叠的同一批来源**严格对齐**——"门禁为什么坏"与
"红队渗透路径报告"是同一份账的两种读法，永不矛盾。

**🔜 后续（触发条件：跨仓渗透报告需求出现）**：多跳 CrossLink 的完整路径枚举
（图搜索 + 深度限制防爆炸），产出端到端攻击链。

## 三、in-toto / SLSA / VEX → 裁决互操作导出 ✅（VEX 部分）

**来源**：[in-toto](https://in-toto.io)（签名声明绑定元数据到工件）、
[SLSA](https://slsa.dev)（供应链成熟度分级，provenance 以 in-toto 格式表达）、
[OpenVEX](https://openssf.org/projects/openvex)（VEX 的最小互操作实现）、
CISA《VEX 最低要求》。VEX 四状态：`affected` / `not_affected` / `fixed` /
`under_investigation`。

**咬合点**：VEX 回答的问题与三值裁决**同构**——"这个组件受这个漏洞影响吗"。
且 `fixed` 必须有证据（曾受影响现已修复），恰与账本纪律对齐。

**落地**：`vex.rs`——`export_vex(dag, author, product)` 输出 OpenVEX v0.2.0
JSON。状态映射：`-1→affected`、`0→under_investigation`、`+1→not_affected`、
`+1 且历史有 -1→fixed`（**账本派生**：没有 `曾 -1` 证据不得声明 fixed——
与"作者打勾 ≠ +1"同一条纪律，VEX 文档不许比账本乐观）。

**🔜 排队（触发条件：需要向 SBOM 生态出证）**：HMAC 签名 attestation
（复用 `auth.rs`），对齐 in-toto statement 形态（subject=工件，predicate=裁决集）。

## 四、Kani 模型检查 → GOAL.md 升级①完全体 🔜

**来源**：[Kani](https://github.com/model-checking/kani)（AWS，Rust 模型检查器，
符号执行 + CBMC）。proof harness 用符号值穷举**全部**输入空间，比随机性质测试
强一个量级（随机 = 采样，Kani = 有界穷举）。

**咬合点**：性质测试五条不变量（`tests/properties.rs`）就是现成的 harness 素材：
K3 AND 单调性、merge 交换律、门禁单调性——全是"任意输入都该成立"的命题。

**🔜 落地方式（不进常规测试，零依赖不受影响）**：`verify/` 目录放
`#[cfg(kani)]` harness，README 说明 `cargo kani` 跑法。触发条件：本机装 Kani
（`cargo install --locked kani-verifier`）。内核代码零改动。

## 五、Differential Dataflow / DBSP → 增量传播 📝 记账

**来源**：McSherry et al., "Differential Dataflow" CIDR 2013（IVM 的迭代推广，
被引 337+）；Murray et al. Naiad/timely dataflow；Budiu et al. DBSP。

**为什么记账不落地**：当前污染传播是全量 Bellman-Ford（千节点压力测试通过）。
增量维护 (min,max) 半环对"变好"方向不单调（`-1→+1` 要回收已扩散的污染），
需要 differential 级技术——复杂度量级远超当前需求。**触发条件：万级节点 ×
高频变更时重开此案。**（防过度工程：现在做是用复杂度换不需要的性能。）

## 六、Datalog / Well-Founded Semantics → 逻辑语义定位 📝 记账

**来源**：Van Gelder et al. 1991 WFS（带否定的 Datalog 语义，天然三值：
true/false/undefined——与 K3 的 +1/-1/0 对应）；McSherry 2018
"Incremental Datalog with Differential Dataflows"。

**记账理由**：门禁判定 = 因果链上带否定的最小不动点，WFS 的 undefined 恰是
0 态驻留的语义学根据——这给 GOAL.md 的理论章提供一个"我们不是发明三值，
我们是站在 WFS 肩上"的定位。无需代码，等论文写作时引用。

## 七、上游 trit-codec 压缩态 → 性价比评估（2026-10-31 追加）

**来源**：bfxh/trit workspace 的 `crates/trit-codec`（publish=false 实验层）：
`pack`（5 trit/byte = 1.6 bit/trit 二进制打包）、`plan`（按分布选编码）、
`rle`/`delta`/`sparse`/`hamming`/`checksum`（GF(3) 纠错/校验）。

**评估结论：压 journal 文本 = 性价比不行**。trit-codec 的全部编码器作用在
**三值序列**（`&[Ternary]`）上；journal 是文本行（大头是 title/note 字符串
与 HLC 时间戳），三值编码器对文本无能为力——journal 的体积问题正确解法是
已有的快照缓存（重放成本）+ backup 归档（体积上限），不是压缩。

**采纳的部分（"不行就自己写"）**：三值数据的存储压缩是真需求
（裁决批量序列化），自写零依赖 `TriPack`（`bitparallel::tri_pack/tri_unpack`，
5 trit/byte、槽位序 3⁵ 进制、13 个非法字节值全部显式报错）——
比双导轨 TriWord（2 bit/trit、32-trit 块对齐）省 20% 且无块尾浪费；
与上游 pack 同构（PR 分支内可对拍互认，独立仓库保持零依赖）。

**🔜 触发条件**：TriWord/TriWord 批量裁决导出（VEX/attestation 的二进制
载体）需求出现时，TriPack 即插即用。

## 研究方法备忘

判断咬合的三条标准（本轮实际使用）：
1. **语义对齐**：外部概念与本系统的某个既有原语是否一一对应（如 VEX 四状态
   ↔ Trit+历史）？对应则互操作，不对应则生搬硬套；
2. **账本纪律不被破坏**：任何导出/视图不得比账本更乐观（fixed 必须有 -1 历史）；
3. **机械可执行**：落地物必须是纯函数 + 确定性测试，不留"靠人眼"的口子。
