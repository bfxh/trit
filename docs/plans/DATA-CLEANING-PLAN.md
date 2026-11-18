# 数据清洗 × 三进制：方向评估、主线选择与底层缺口（2026-11-11）

> 计划与调研，不是结果；实现后按 STATUS.md 的四个状态词改写进度。
> **2026-11-11 更新：C1 + B（M0 范围）已交付** —— `crates/trit-clean`：规则 DSL + 三值求值（SQLite 对拍）
> + 精确完成计数 / 最小修复计数（与暴力枚举对拍）；C2b：BDD×DPLL 交叉校验、超限抽样（显式标记）、
> UNIQUE 自动候选范围；C3：三支决策门（代价矩阵 / 阈值 / 原因码 / 阈值扫描）。WMC 仍是后续。
> 定位：用三进制/三值逻辑做**可解释、可复算**的数据清洗，不做“再训一个模型猜值”。

## 0. 结论（先给判断）

三进制在数据清洗里真正独特的不是“省一位存储”，而是**三值语义本身就是“未知/矛盾/待定”的表示**：
SQL 的 `NULL`、缺失值、多源冲突、待人工复核——清洗的每一步都是三值判定。
按“独特性 × 现有底座可复用度”排序，主线选两条：

1. **三值约束检查 + 精确修复计数**（`trit-clean`）：把业务规则编译成三值电路/CNF，
   用本仓已有点 counting（`trit-count` 的 #SAT/BDD）算“这一行有多少种最小修复、坏得有多彻底”，
   得到**可解释的清洁度分数**，而不是模型给的置信度。
2. **三支决策动作门**（accept / reject / defer）：Yao 的三支决策（TAO：trisecting–acting–outcome）
   正好对应三值输出；`defer` 不是失败，而是把“证据不足”显式交给人工队列。

其余方向（Belnap 四值多源合并、三值去重、缺失插补、三值规则挖掘）按需排队，见 §1。

## 1. 方向清单（三进制在数据清洗的九条路）

| # | 方向 | 三进制独特性 | 现有底座 | 判据（可复算） | 优先级 |
|---|---|---|---|---|---|
| A | **三值约束检查**（CHECK/FK/UNIQUE/NULL 传播） | `bool3` 位平面批量求值；UNKNOWN 是一等公民，不是补 0 | `trit-core` 的 bool3；位平面 ALU | 与 SQLite/Postgres 三值语义逐条对拍（NULL 传播一致） | **P0** |
| B | **精确修复计数 / 清洁度分数** | 同一行“有 0/1/n 种最小修复”是计数问题，不是概率猜测 | `trit-count`：#SAT、BDD 计数、`crosscheck`；`trit-sat` 反例 | 小规模与穷举对拍；大规模 BDD×DPLL 交叉校验 | **P0** |
| C | **三支决策动作门**（收/拒/转人工） | 三值输出天然对应三种动作；阈值可由代价矩阵算出 | 无（需新写）；论文成熟（3WD/TAO） | 阈值扫描的人工工作量–准确率曲线，对照二值基线 | **P0** |
| D | **Belnap 四值多源合并**（真/假/矛盾/未知） | 四值比三值多一档“矛盾”，正是多源冲突的形态 | `trit-belnap` 双格算子已建（2026-11-11：共识/轻信/共轭/合并） | 与一致性查询（CQA）语义对拍 | P1 |
| E | **三值指纹去重**（近重复照片/文本） | `{-1,0,+1}` 签名 + popcount 内核；SimHash 的三值推广 | `trit-dot`（掩码/tryte 内核）、`trit-ops` | precision/recall 对拍 MinHash/SimHash | P1 |
| F | **缺失值插补**（缺失=unknown，不补均值） | 粗糙集正域/边界域/负域 ↔ 三支聚类/属性约简 | 无；`trit-learn` 有回归底座 | 插补误差对拍 MICE/KNN（同一留出集） | P2 |
| G | **三值规则挖掘**（不完美数据上的关联规则） | Łukasiewicz t-norm 做置信度；缺列不丢整条 | `trit-mv` 已建（2026-11-11：Łukasiewicz 链 + 4 种 t-norm + residuum） | 完备数据上与 Apriori 一致；不完备数据召回提升 | P2 |
| H | **数据契约编译器**（规则→三值 IR→电路） | 把业务规则编译成可计数电路；与 A/B 联动 | 无；前置是 `trit-ir` pass 框架 | 编译前后语义等价（真值表穷举） | P2 |
| I | **文本/日志清洗**（三值模式匹配） | `trit-re` 的 YARA 子集 + 三值匹配 | `trit-re` | 与正则/YARA 对拍 | P3 |

**为什么先做 A+B+C**：三者共用一条数据流（规则 → 三值求值 → 计数 → 三支动作），
底座已有大半（bool3、#SAT、BDD、反例生成），且**判据最硬**（能对拍 SQL 语义与穷举计数）。

## 2. 主线 `trit-clean`：架构与差异化

```
规则 DSL（JSON/文本） ──编译──▶ 三值电路 / CNF
                                   │
                         bool3 批量求值（A）
                                   │
                    精确计数：#SAT/BDD（B，trit-count）
                                   │
             清洁度分数（每行/每列/每档案）+ 最小修复集
                                   │
                 三支决策门（C）：accept / reject / defer
                                   │
                    审计解释：哪条规则、哪种修复（provenance 方向）
```

**差异化**（相对 HoloClean/NADEEF 类与 LLM 清洗）：

- **确定性**：同一输入同一读数；分数来自模型计数，不是抽样。
- **可解释**：能点出“违反哪条规则、有几种最小修复、为什么 defer”。
- **可复算**：判据是对拍（SQL 三值语义、穷举计数），不是“看起来更干净”。
- **三值语义对齐 SQL 标准**：`NULL` 传播逐条与 SQLite/Postgres 对拍——这是清洗结果可信的前提。

**可复用资产**：`trit-count`（DPLL 计数 + BDD + 交叉校验）、`trit-bdd`（等价/计数/反例）、
`trit-sat`（最小反例）、`trit-belnap`（四值语义）、`trit-dot`（签名内核）。
另有未合并分支 `feat/exact-propagation-perf`（三值拓扑两趟 BFS 传播 + 通过/未定/否定验证证书），
是 A/B 的直接燃料——**启动前要先决定：合入还是重写**（分支无合并证据，需作者确认）。

## 3. 底层缺口（“还有什么没做的”）

按“对清洗主线的必要性”排序；每项给出缺什么、前置关系与验收口径。

| 层 | 缺什么 | 为什么卡清洗 | 验收口径 |
|---|---|---|---|
| 计数/推理 | **加权模型计数（WMC）**：现在只有 #SAT（计数），没有按权重求和 | 清洁度的“概率版”（按来源可信度/修复代价加权）需要 WMC | 小规模与穷举加权和一致；与 #SAT 在权重=1 时逐位一致 |
| 计数/推理 | CDCL 与预处理（BVE/BCE）、MaxSAT | DPLL 计数在中等规模会慢；MaxSAT 是“最少改几处”的直接求解器 | 标准集上与 DPLL 计数一致、超时率下降 |
| 代数 | ~~`trit-mv`~~ **已建（2026-11-11）**：Łukasiewicz 链 + 4 种 t-norm + residuum | G 的前置已解除；D 的格论口径已具备 | 链上公理穷举 + `trit-core` Kleene 对拍 |
| 代数 | ~~`trit-belnap` 的算子面~~ **已建（2026-11-11）**：双格 + 共识/轻信/共轭/合并 | 多源合并（D）的算子已具备；剩“合并策略接入 trit-clean” | 双格公理穷举 + 已知序关系 |
| IR | ~~`trit-ir` pass 框架~~ **已建（2026-11-11）**：DAG + 常量折叠/化简/CSE + 双 oracle 验证 | 数据契约编译器（H）的前置已解除；剩“规则→IR”的编译前端 | 每个 pass 以 `trit-sat`+`trit-bdd` 验证（`trit-count` 可作第三 oracle） |
| 数值 | ~~`trit-num`~~ **已建（2026-11-12）**：补偿求和 / Welford / OLS+岭回归 / 中位数 / 均值·中位数·线性插补 | F 的关键件已具备（F 的“三支聚类/粗糙集”仍是后续） | 精确直线恢复 + f64 vs f32 同题对拍 + 变异 7/7 |
| 存储 | `trit-export`（量化容器）；`trit-vault`（隐私，已立项） | 清洗结果的归档/导出 | 与 `trit-codec` 打包逐位一致 |
| 运行时 | ~~mmap 与 SIMD 抽象~~ **已建（2026-11-12）**：`trit-mmap` 只读映射 + `trit-simd` popcount/Hamming 运行时派发（AVX512/POPCNT/NEON/标量）；**async 仍缺** | 大文件吞吐与指纹去重（E）的底座已具备 | 逐字节对拍 + 全长度标量参考对拍 + 变异 6/6 |
| 观测 | 无 tracing/metrics/错误分类 crate | 清洗任务要能解释“慢在哪/为什么 defer” | 每条 defer 带结构化原因码 |
| 工具链 | 无批量格式转换/内置存储管线（隐私任务要）；无清洗 CLI | 落地形态 | 见 PRIVACY-PLAN §3.6 的判据 |
| 语料 | 无“数据清洗对拍语料”（生成器 + 小规模穷举集） | 判据需要标准输入 | 生成器可复现（固定种子），小规模可穷举 |

**结论**：清洗主线不需要等所有底层补齐；A/B/C 只依赖已有的 bool3 + #SAT + BDD。
WMC 与 `trit-mv`/`trit-belnap` 算子面是 P1 的“第二步”，`trit-ir` 是 P2。

## 4. 里程碑（后期任务，带判据）

| 阶段 | 交付 | 判据 |
|---|---|---|
| C0 | 本文（方向评估 + 判据） | 评审通过；主线与取舍写死 |
| C1 | ~~`trit-clean` M0：规则 DSL + bool3 批量求值~~ **已交付（2026-11-11）** | SQLite 对拍 6 比较符 × 5 取值 + TEXT + NOT NULL + CHECK/UNKNOWN + UNIQUE/NULL；`cargo test -p trit-clean` 34 条 |
| C2 | ~~M1：最小修复计数 + 清洁度分数~~ **已交付（M0 范围，2026-11-11）** | 与暴力枚举逐例一致（无解/两处改动/三行唯一冲突）；上限（120 变量、10 候选格）超限结构化报错 |
| C2b | ~~BDD×DPLL 交叉校验 + 超限抽样 + UNIQUE 自动范围~~ **已交付（2026-11-11）** | `crosscheck` 逐例一致（不一致 ⇒ 结构化错误；超 200k 节点标“未校验”）；抽样同种子可复算且 CI95 覆盖真值；`minimal_repairs_auto` 对三行 UNIQUE 冲突给出深度 1、条数 2 |
| C3 | ~~M2：三支决策门（代价矩阵 + 阈值扫描）~~ **已交付（2026-11-11）** | `gate` 逐行给动作 + 结构化原因码（6 种）；代价矩阵平局默认 defer（容差 1e-12）；`scan_confidence` 三动作计数随阈值单调不增、给出期望总代价；55 条测试、变异 16/16 |
| C4 | M3：Belnap 四值合并（先补算子面） | 与 CQA 语义对拍 |
| C5 | 可选：去重/插补/规则挖掘 | 各自的对拍基线（§1 表） |

红线：不把 LLM 猜值当清洗结论；算不动必须响亮报错，不许静默降级；清洗语料不含真实个人数据。

**C1/B 实测口径**（2026-11-11）：`cargo test -p trit-clean` = 22 单测 + 6 SQLite 对拍 + 5 暴力对拍 + 1 doctest；
`python scripts/mutation_check.py --pkg trit-clean` = 8/8 抓住、0 存活。
**C2b 实测口径**（2026-11-11）：45 条测试（30 单测 + 3 BDD 交叉 + 6 SQLite + 5 暴力 + 1 doctest）；
变异 12/12 抓住；抽样 20,000 次在已知 0.2 比例上点估计误差 <0.02、CI95 覆盖真值。
**C3 实测口径**（2026-11-11）：55 条测试；变异 16/16；三支门在默认代价下 p=1/0.95→accept、
p=0.9（理论平局）→defer、p=0.1/0→reject；阈值扫描 accept 数随置信阈值单调不增。

## 5. 论文与参考

- **Relational Data Cleaning Meets Artificial Intelligence: A Survey**（2025）——错误检测/修复/插补三任务全景。
  [Springer](https://link.springer.com/article/10.1007/s41019-024-00266-7)
- **A Survey of LLM × DATA**（清华，2025）——LLM 做数据管理的现状与边界。 [PDF](https://dbgroup.cs.tsinghua.edu.cn/ligl/papers/DataAI-2025.pdf)
- **Database Repairs and Consistent Query Answering**（ACM 2019）——最小修复与 CQA 的语义基础。 [ACM](https://dl.acm.org/doi/10.1145/3294052.3322190)
- **Provenance Semirings**（Green et al., PODS'07）——“为什么这条数据被清洗”的半环框架。 [PDF](https://web.cs.ucdavis.edu/~green/papers/pods07.pdf)
- **Model Counting and Sampling via Semiring Extensions**（AAAI 2024）——计数与抽样统一到半环扩展。 [AAAI](https://ojs.aaai.org/index.php/AAAI/article/view/30022/31797)
- **三支决策（3WD/TAO）**：Yao 的三支决策体系；不完整信息下的 3WD 2025 综述。 [arXiv](https://arxiv.org/abs/2512.21421) · [TAO 模型](https://journals.sagepub.com/doi/10.3233/JIFS-202207)
- **Belnap–Dunn 四值与不一致库查询**：BD⊥ 扩展用于带 NULL 的不一致数据库。 [SACS](https://publications.info.uaic.ro/scientific-annals-of-computer-science/sacs-volumes/xxxiii-2/belnap-dunn-logic-and-query-answering-in-inconsistent-databases-with-null-values/)
- **Paraconsistent Logic**（SEP 条目）——矛盾容忍的逻辑背景。 [SEP](https://plato.stanford.edu/archives/fall2006/entries/logic-paraconsistent/)

## 6. 风险与边界

- **规模**：精确计数最坏指数级；必须组件分解 + 上限 + 抽样兜底，且“算不动”要显式报错（本仓口径：查不到≠没问题）。
- **语义**：三值语义先与 SQL 标准对拍，再谈清洗效果；否则只是“另一种脏”。
- **数据**：清洗语料用生成器 + 公开小样本；真实个人数据不进仓（与 SCOPE.md 一致）。
- **定位**：`defer`（转人工）是设计的一部分，不追求“全自动洗完”。
