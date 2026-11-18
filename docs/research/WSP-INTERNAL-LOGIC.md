# WSP 内部逻辑读取：从「特征」到「程序」

> 状态：**设计，未开工**。起点 `feat/trit-gamescan` @ `75bd2c5e`。
> 本文只写「怎么把读懂内部逻辑变成可判定问题」，不含实现；任何「已实现」的说法都不成立。
> 论文分两类标注：**已核**＝取过 arXiv abs 页核对标题/摘要/会议；**仅标题**＝只在 arXiv
> 检索页看到标题与摘要，未取 abs 页。

## 0. 一句话

把「读懂权重」从**特征层**（这个方向对应什么概念）提到**程序层**（这段权重等价于什么
算法），并让「等价」在 trit 的有界域上成为**可判定**而不是近似：反编译出最小自动机、
穷尽验证中间状态承重、把状态轨迹读成思考链。

## 0.5 现有资产（不要从零写）

`crates/trit-wspace` 已经有行为级与零件级的读取，缺的是**逻辑级**：

| 能力 | 现状 | 属于哪层 |
|---|---|---|
| `Oracle` 穷举行为（`crates/trit-wspace/src/oracle.rs`，上限 2^20 / 3^10） | 已跑通 | 行为 |
| `Oracle::exact` 逐样本判等（同文件） | 已跑通 | 行为 |
| `probe` 行级消融 / `localize` 行内 trit 定位 | 已跑通 | 零件 |
| `explain` 单样本为什么错 / `analyze` 单点编辑实测影响 | 已跑通 | 零件 |
| `repair` 无梯度精确修补 | 已跑通 | 搜索 |
| `program` / `Patch` / `Journal` 前置后置行为条件与回滚 | 已跑通 | 事务 |
| **中间状态轨迹** | **缺**：`grep trace crates/trit-wspace/src` 零命中 | — |
| **状态抽象 / 最小化** | **缺** | — |
| **权重 → 程序 反编译** | **缺** | — |

可复用的外围件：

- `crates/trit-wspace/src/net.rs` 的 `forward` 只返回最终输出；`pre_activations`
  返回每层整数前激活，已经够当「状态向量」的原材料。
- `crates/trit-machines/src/vm.rs` 的三值栈式解释器遇到 `X` 会**分叉**并返回所有可能输出与
  路径数——它本身就是一个「思考链可读」的执行器，可作为轨迹层的参考语义。
- `crates/trit-circuit` 有 AIGER/BLIF 网表、SAT-CNF、技术映射：逻辑级路线要借它的
  「网表 ↔ 逻辑等价」工具，而不是另造一套。

## 1. 问题定义：什么叫「读懂逻辑」

四层，每层给**可机器判**的判据，不给「看起来像」：

| 层 | 回答什么 | 判据 | 本仓现状 |
|---|---|---|---|
| L-A 特征 | 某方向/单元对应什么概念 | 探针/消融的相关性，只能给「相关」 | `probe`/`localize` |
| L-B 电路 | 哪些单元经什么路径产生行为 | 消融后行为漂移是否显著 | `probe` 的粗版 |
| L-C 逻辑 | 这段权重等价于什么**程序** | 存在 P 使穷举域上逐点相等 | **缺** |
| L-D 轨迹 | 这次计算的中间状态序列是什么 | 每步可干预、可承重 | **缺** |

本文说的「读懂」指 **L-C + L-D**，形式化为两条同时成立：

1. **行为等价**：存在显式程序 `P`（自动机 / 结构化程序），使得对声明域 `D` 的每个输入
   `x`，`Net(x) = P(x)`。`D` 必须写在报告里，域外一概不主张。
2. **状态承重**：存在把网络中间激活映到 `P` 的中间变量的映射 `φ`，且对所有**穷尽的**
   interchange intervention 都成立——把某一步的激活换成另一条输入的，输出必须按 `P` 的
   语义变化；不变化的那一步是装饰。

只有第 1 条，得到的是一份**描述**（可以拟合但没解释力）；加上第 2 条才是**机制**。

「读懂思考链」在这一框架下不是读输出文本的 CoT（已有多篇证伪，见 §2 第四组），而是：

- 轨迹 = 网络一次前向的离散中间状态序列；
- **可读** = 轨迹能映射到 `P` 的变量命名（「当前进位=1」「已见左括号=2」）；
- **忠实** = 沿轨迹做单步干预，输出按 `P` 预测改变（不是「换个说法照样对」）。

## 2. 论文地图（四组）

### 2.1 程序 ↔ 权重（双向）

| 论文 | 结论 / 对本文的用处 | 核实 |
|---|---|---|
| Tracr: Compiled Transformers as a Laboratory for Interpretability, arXiv:2301.05062 | 把人类可读程序编译成 transformer 权重；真正价值是**提供 ground truth**，让「读对了没有」第一次有答案。本文路线 3 反向用它。 | 已核（NeurIPS 2023 Spotlight） |
| Transformers Learn Shortcuts to Automata, arXiv:2210.10749 | 浅层 transformer 能表示任意有限状态自动机（层级重参数化）。含义：**层数 ≠ 推理步数**，所以轨迹不能按层读。 | 已核 |
| Weight-Space Programming Doesn't Need Transformers（本仓已复现过读取器） | 无梯度时 LLM 倾向把事实编译成稀疏表 + 固定读取器。含义：**任务是查表就没有算法可读**；要读逻辑必须先要求任务是算法。 | 本仓已有复现记录 |

### 2.2 从网络提取自动机 / 程序（L-C 的主力工具）

| 论文 | 结论 / 对本文的用处 | 核实 |
|---|---|---|
| Extracting Automata from RNNs Using Queries and Counterexamples, arXiv:1711.09576 | L* 主动学习提 DFA，网络当 oracle。本文不需要 L*：`Oracle` 能穷举完整行为表，可直接构造。 | 已核（ICML 2018） |
| Extracting Finite Automata from RNNs Using State Merging | 状态归并范式；反直觉读数：**训练超过收敛点会让内部状态空间压缩**，提取更容易。 | 仅标题（2022-01） |
| Weighted Automata Extraction and Explanation of RNNs for NL Tasks | 加权自动机；处理转移稀疏与上下文丢失。 | 仅标题（2023-06） |
| Extracting Weighted Finite Automata from RNNs for Natural Languages | 同上，ICFEM 2022 版。 | 仅标题 |
| Synthesizing Context-free Grammars from RNNs | 把 DFA 序列升成 CFG，比正则更强。 | 仅标题（TACAS 2021） |
| A Formal Language Approach to Explaining RNNs（LEXR） | 用 LTL 描述时序性质，比 DFA 更接近人类语言；PAC 保证。 | 仅标题（2020） |
| Learning Deterministic Weighted Automata with Queries and Counterexamples | 噪声下的 L* 变体，抽 PDFA。 | 仅标题（NeurIPS 2019） |
| A Comparative Study of Rule Extraction for RNNs | 提取成功率随语法复杂度下降；低复杂度语法多数网络可提。 | 仅标题（2018） |

### 2.3 因果抽象（L-D 的判据来源）

| 论文 | 结论 / 对本文的用处 | 核实 |
|---|---|---|
| Causal Abstractions of Neural Networks, arXiv:2106.02997 | 形式化「高层因果模型是低层网络的忠实简化」，interchange intervention 的定义出处。 | 已核（NeurIPS 2021） |
| Finding Alignments Between Interpretable Causal Variables and Distributed Neural Representations（DAS）, arXiv:2303.02536 | 用梯度找对齐、允许分布式表示。三值离散上下文里可换成**穷举对齐**，不需要梯度。 | 已核 |
| Towards Automated Circuit Discovery for Mechanistic Interpretability（ACDC）, arXiv:2304.14997 | 自动找电路；把手工流程系统化。本文的电路层可借它的「选度量 + 换数据集」框架。 | 已核（NeurIPS 2023 Spotlight） |

### 2.4 思考链的忠实性（为什么不能读文本 CoT）

| 论文 | 结论 / 对本文的用处 | 核实 |
|---|---|---|
| Language Models Don't Always Say What They Think, arXiv:2305.04388 | CoT 可系统性歪曲真实原因；加偏置特征后模型照样编理由，准确率最多掉 36%。 | 已核（NeurIPS 2023） |
| Measuring Faithfulness in CoT Reasoning, arXiv:2307.13702 | 干预 CoT 测忠实度；模型越大越不忠实。给出「干预」这套实验范式。 | 已核 |
| Chain of Thought Monitorability, arXiv:2507.11473 | 把可监控性当安全抓手，同时指出它脆弱。 | 已核（作者页） |
| Quantifying the Necessity of CoT through Opaque Serial Depth, arXiv:2603.09786 | **量化**「不经可读中间步最多能做多深计算」，开源算任意网络上界的方法。可直接借来证明「这条链真在算」。 | 已核 |
| Training LLMs to Reason in a Continuous Latent Space（Coconut）, arXiv:2412.06769 | 连续潜空间推理；连续 thought 可编码多条候选路径（BFS）。 | 已核（COLM 2025） |
| Unlocking the Black Box of Latent Reasoning: An Interpretability-Guided Approach to Intervention | 结构/因果/几何三探针，早期潜向量是因果枢纽；训练无关的解码期干预。 | 仅标题（ACL 2026） |
| LLM Reasoning Is Latent, Not the Chain of Thought | 立场文：研究对象应是潜状态轨迹而非表面 CoT；提出 H0/H1/H2 三假设。 | 仅标题（2026-04） |
| SPOT: Span-level Pause-of-Thought | 把 CoT 压成潜 pause token，用冻结头解码保持可读。 | 仅标题（2026-03） |
| Project Ariadne: A Structural Causal Framework for Auditing Faithfulness | 用 SCM + do-calculus 对中间推理节点做硬干预，测 Causal Sensitivity；发现「因果解耦」。 | 仅标题（2026-01） |

## 3. 为什么这条路线在三值上成立、在 f32 上不成立

| 性质 | f32 大模型 | trit 三值 + 有界域 |
|---|---|---|
| 行为等价 | 只能抽样估计 | **穷举判定**（`Oracle::exact`） |
| 反编译 | 数值噪声，不可逆 | 离散，可逆 |
| 干预 | 抽样、统计显著 | **穷尽所有干预对** |
| 状态空间 | 连续高维，无限 | 有限，可枚举、可最小化 |
| 「读懂」的性质 | 解释（可信度打分） | **证明**（对/错） |

代价同样明确：只能小网络；三值表达力有限；有界域外**不主张任何结论**。这三条要写进
每份报告的未证栏。

## 4. 四条路线（按可控性排序）

### 路线 1：权重 → 最小自动机（先做）

不用 L*，直接构造：

1. `Oracle` 穷举得到完整行为表；
2. 按 Myhill–Nerode 做**后缀等价类**划分（两个前缀等价 ⟺ 对所有后缀输出相同）；
3. 等价类即状态，得到**最小** DFA/Mealy 机；
4. 与人工写的参考自动机对拍：状态数、转移表、逐输入输出。

判据三条硬指标：`DIFF=0`；状态数等于 Myhill–Nerode 等价类数（即最小）；转移表逐项相等。

这是「读出的是程序，不是特征」的最硬证据。

### 路线 2：中间状态的因果抽象（读思考链）

1. 把 `pre_activations` 的离散状态按行为等价折叠成少量状态；
2. 给每个状态分配高层变量名（来自参考程序）；
3. 做 interchange intervention：把样本 A 第 k 步的状态换成样本 B 的；
4. 验证输出是否按参考程序的语义变化。

三值离散 ⇒ 干预空间有限 ⇒ **可以穷尽**，给出「完全忠实」而非「抽样忠实」。这一步补的
正是 §2.4 那组论文指出的洞：不读模型说的话，读它的内部状态，并证明这个状态承重。

### 路线 3：Tracr 反向闭环（唯一能证明方法有效的实验）

`program → 三值权重 → 反编译 → 必须逐位恢复原程序`。

Tracr 用 f32，编译带数值噪声，只能评估「准确率」。三值上是精确的，所以这里可以要求
**恢复率 100%**，否则就是负结果入册。这是本文最值得先做的校准实验：没有 ground truth，
「读懂了」无法证伪。

### 路线 4：真串行 —— 轨迹即链

单次前向深度固定，不随问题变长，所以严格意义的「思考链」需要循环结构：

- 用 `crates/trit-machines` 的三值 VM（`X` 分叉）当执行器，轨迹天然是状态序列；
- 或用 looped/recurrent 结构把多步推理展开；
- 用 opaque serial depth（arXiv:2603.09786）的方法给「不经过可读步最多能算多深」算上界，
  作为「这条链不是装饰」的量化证据。

## 5. 最小切片（M0）

对象：一个**已知算法**的三值网络（三选一，按实现成本排序）：

1. 3-bit 二进制加法器（有进位，状态少，参考程序最短）；
2. 括号匹配 DFA（状态 3–4 个，轨迹直观）；
3. Tomita #3（`2.2` 那组论文的标准基准，可对外比较）。

步骤：

| 步 | 内容 | 产出 |
|---|---|---|
| M0a | 用 `Oracle` 穷举行为表 | 完整输入→输出表 + 域声明 |
| M0b | Myhill–Nerode 最小化 | 最小 DFA（状态/转移表） |
| M0c | 与参考程序对拍 | 状态数、转移表、逐输入三方差分 |
| M0d | 轨迹提取 + 状态命名 | 每个输入的状态序列 + 变量标签 |
| M0e | 穷尽干预 | 承重矩阵（哪步换了会变、哪步不会） |
| M0f | Tracr 反向闭环 | 程序→权重→程序 的恢复率 |

M0 不碰：大网络、f32、连续潜空间、自然语言任务。

## 6. 验收（机器可判）

- **等价**：M0c 的三方差分全零，`DIFF=0`；
- **最小**：状态数 = Myhill–Nerode 等价类数，且不存在更少状态的等价机；
- **承重**：M0e 的干预预测一致率 100%（有一步不一致就说明 `φ` 选错了，重选而不是放宽判据）；
- **闭环**：M0f 恢复率 100%；低于 100% 时按失败处理并写清哪一类程序恢复不了；
- **可复算**：一条命令跑完，读数写进 `docs/research/WSP-INTERNAL-LOGIC-RESULTS.md`；
- **负结果入册**：状态爆炸 / 提取不出 / 不等价，全部记录，不许只报成功的例子。

## 7. 交付物

- 本文（设计）。
- `docs/research/WSP-INTERNAL-LOGIC-RESULTS.md`（读数 + 未证栏）。
- `benchmarks/wsp-internal-logic/`（独立 workspace，不进 CI）：Rust 实现最小化 + 轨迹 + 干预；
  Python 只作对齐基线，不交付。
- 若路线 1 成立，再考虑把状态最小化收进 `crates/trit-wspace`（`state` 模块），并补
  `WSP1` 工件里的状态表导出。

## 8. 禁区

- 不在 f32 大模型上宣称「证明了逻辑」——那里只能抽样。
- 不把「最小 DFA」直接当「人类可读」：唯一 ≠ 好懂，命名/结构化是独立问题（见 §11）。
- 不用「看起来像某个算法」当结论；没有 `DIFF=0` 就只是假设。
- 不在有界域外主张任何等价性。
- 不把 Python 训练产物当交付。

## 9. 失败回退

- 状态爆炸（等价类数超预算）：缩域或缩网络，记录「该规模下不可读」，不换成近似方法硬凑。
- 干预一致率达不到 100%：说明状态折叠太粗（把不同变量合并了），先细化再测；仍不行则
  报告「该网络不存在小状态抽象」，这本身是负结果。
- Tracr 反向恢复失败：定位是哪类结构恢复不了（查表 / 算术 / 分支），写进未证栏。

## 10. 里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| M0a–M0c | 穷举 + 最小化 + 对拍 | 三方差分全零 |
| M0d–M0e | 轨迹 + 穷尽干预 | 承重矩阵 + 一致率 100% |
| M0f | Tracr 反向 | 恢复率读数（100% 或失败类别） |
| M1 | 换算法（加法 → 括号 → Tomita） | 同一套判据，看提取率随复杂度怎么掉 |
| M2 | 接 `trit-circuit` 做逻辑等价 | 网表级等价判定 |
| M3 | 循环结构上的真串行轨迹 | 用 opaque serial depth 给上界 |

## 11. 难度与工作量

- 难度：**S**（独立 oracle / 性质测试 / 负结果入册 / 明确的域声明，四门槛齐全）。
- 预估：M0 2–3 周（单人）；M1 +1 周；M2/M3 另计。
- 与 trit 主线的关系：**中**——复用 `Oracle` 与离散语义，但产出的是「读取方法」而非新算子。

## 12. 未证栏

- 本包没有代码；任何「已实现」都不成立。
- 三值网络能否在 M0 的三个算法上都抽出小自动机**未验证**；§2.2 组的读数是在 f32 RNN 上，
  三值前馈网的可提取性没有直接证据。
- 「穷尽干预」的规模上界未算：干预对数是状态数 × 样本数，M0 规模内可行，放大后未知。
- 最小 DFA 的唯一性（Myhill–Nerode）不等于人类可读性；把状态翻译成变量名这一步没有判据，
  目前只能人工命名后由干预验证。
- opaque serial depth 的方法是为 Transformer 设计的，搬到三值前馈/循环网上是否直接可用未验证。
- 论文核实程度：§2.1/2.3/2.4 大部分取过 abs 页；§2.2 多为检索页标题，落地前应逐篇取 PDF 校。

## 附录：复算指针

带行号的指针写进正文会被行号漂移拖着走，这里只给一次性复算命令：

```bash
sed -n '104p' crates/trit-wspace/src/oracle.rs   # pub fn boolean
sed -n '293p' crates/trit-wspace/src/oracle.rs   # pub fn exact
sed -n '446p' crates/trit-wspace/src/net.rs      # pub fn forward
grep -rn trace crates/trit-wspace/src            # 应为 0 命中（轨迹层缺口）
```
