# WSP-BUDGET：同字节预算分配与块交换（L12 / STS 1,379 对）

> 状态：M1.46 完成（§11.41–§11.46）；STS 全量 + SciFact 检索 + SST-2/CoLA/TweetEval-hate 线性 probe / 线性 SVM + 几何/剂量/交叉迁移/残差诊断 + 直接量化负对照 + 残差线性算子 + nested CV 全矩阵 + Hate 域/阈值审计与目标先验阈值全臂复验 + 预注册 Hate test 半分割 confirmatory + 第二个 hate 语料 HateXplain 的预注册 confirmatory + HateXplain 域感知先验与 fit-only 阈值搬运 + HateXplain 三分类探索 + 静态字节–质量 Pareto + Davidson 第三语料 + fit-balanced prior-free 决策规则。
>
> **M1.46 读法**：把 `class_weight="balanced"` 当作 inductive、
> prior-free 的决策规则（只用 fit 标签估权重、固定默认阈值），
> 三个 hate 语料 12 个 cell 里只有 Davidson L6 SVC 过预注册 5/5
> （macro-F1 `+6.90pp`，平均 CI `[+4.05,+9.62]pp`）；总计
> 1/12，HateXplain 0/4、TweetEval 0/4、Davidson 1/4，远低于
> `≥10/12` 且每语料 `≥3/4` 的门槛。TweetEval 四个 cell 的 CI
> 整段为负（`−3.89~−4.87pp`），HateXplain 四个点估计全负但 CI
> 跨 0，Davidson L12 logreg 4/5 负 CI。`balanced` 的稳定效应是
> hate recall `+3.4~+68.1pp`，代价是 non-hate recall
> `−7.4~−18.1pp`、accuracy `−2.7~−13.4pp`；Davidson 预测正例率
> 被抬到 `0.215`（真实 `0.0575`）。**它是 recall tilt / 操作点
> 移动，不是 macro-F1 改进，也不是校准；candidate 线仍关闭。**
> 另外，本轮 `5/5` 是同一组预测的 5 次 bootstrap RNG，
> 不是 5 次独立拟合（§11.46.6）。见 §11.46。
>
> **M1.45 读法**：Davidson 第三语料按 default 阈值做 descriptive
> transport：test `4,956` 行里只有 `285` 条 hate（`5.75%`）。
> accuracy `0.942–0.945` 看着高，但预测正例率只有 `0.38–2.08%`，
> hate recall `3.9–20.0%`，macro-F1 `0.52–0.63`；四个 family 的
> accuracy `arm−f32` 全部 0/5 正 CI，macro-F1 三负一微负。L12 SVC
> 的 `+0.1211pp` accuracy 伴随 hate recall 下降，不是质量提升。
> **default 阈值的 accuracy 被 94% 多数类撑高；第三语料仍不能升级
> candidate。** 见 §11.45。
>
> **M1.43 读法**：HateXplain 三分类下，四个 family 的 accuracy
> `arm − f32` 全部 0/5 正 CI；L12 logreg `+0.156pp`、L12 SVC
> `−0.323pp`、L6 logreg `+0.469pp`、L6 SVC `+0.073pp`。三分类
> accuracy 只有 `0.649–0.656`，`offensive` recall 仅 `0.354–0.426`，
> 是主要错误来源；binary 的 `0.82` 主要来自 normal-vs-rest。**三分类
> 只作 exploratory，不能升级 candidate。** 见 §11.43。
>
> **M1.44 读法**：静态工件字节 vs 7 个任务指标的 Pareto。没有跨任务
> 统一 winner：L12 STS Spearman 是 `R12` 支配 f32/int4（10.78MB、
> `0.8354`），L12 CoLA 是 `int4` 支配 f32（13.44MB、`0.5612`），
> L6 STS 是 `rescan` 支配 f32（5.39MB、`0.8313`），L6 Hate 是
> `int3` 支配 f32（5.39MB、`0.5505`），L6 SciFact 则是 `int4`
> 最高。**成本–质量是任务相关的；最低字节有时也是最高质量，
> 但不能从一个任务外推。** 见 §11.44。
>
> **M1.42 读法**：把 M1.38 的 `target_prior` 从 test 分数分位点换成
> fit OOF 分位点后，TweetEval/hate 的 candidate gain 没有保留：L12
> `R12` 从 `+1.83/+1.68pp` 翻成 `−1.77/−1.59pp`，L6 `rescan` 从
> `+2.87/+2.55pp` 掉到 `+0.02/+0.58pp`；四个 family 全部
> `not_retained`。fit-only 的 f32 accuracy 比 test-prior 低
> `7.3–11.0pp`，预测正例率是 `0.76–0.80`，而 test 真实先验 `0.42`。
> **M1.38 的 rescue 依赖 test 分数分布，是 transductive 现象，不是
> 可部署阈值。** 见 §11.42。
>
> **M1.41 读法**：HateXplain 的 Gab/Twitter hate 先验是 `0.502/0.078`，
> 一个全局 prior 阈值必然错配。域感知阈值 `domain_test_prior` 相对
> global 在 L12 两个 probe 5/5、L6 logreg 4/5 上 supported，L6 SVC
> 0/5；但它仍低于 default：`0.815/0.816`（L12）、`0.818/0.814`（L6）
> vs default `0.823/0.822`。所以域拆开只解释了一部分伤害；匹配边际
> 先验不等于匹配最优决策阈值。fit-only 在 L12 略好、L6 更差。
> **candidate 没有被域规则救回，target_prior 仍不能当默认协议。**
> 见 §11.41。
>
> **M1.40 HateXplain 独立语料 confirmatory（2026-11-26）**：在第二个 hate
> 语料 HateXplain 的官方 `train+val`/`test` 上预注册 confirmatory；清洗后
> fit `17,270`、test `1,918`，hate 先验 `0.308975/0.309698`。Primary
> accuracy（full test、target_prior、matched-C）：L12 `R12` logreg
> **0/5** 正 CI（`+0.1043pp`）、SVC **0/5**（`−0.2920pp`）；L6 `rescan`
> logreg **0/5**（`−0.1043pp`）、SVC **0/5**（`+0.3545pp`）。两个候选都被
> 预注册判据判为 **rejected**；macro-F1 与 default 0.5 也全 0/5。更关键的是
> target_prior 在该语料上把 f32 的绝对 accuracy 从约 `0.823` 降到
> `0.808`（L12）/`0.811`（L6），量化臂也降 `0.80–1.09pp`；匹配先验把
> 预测正例率抬到 `0.3092`（default `0.2388–0.2607`），但 accuracy 反而下降。
> **M1.38/M1.39 的 `+1.6~+2.8pp` 没有复现；`R12` 和 `rescan` 都不能升级，
> target_prior 也不能当默认协议。**
>
> **M1.39 半分割 confirmatory（2026-11-26）**：在既有 TweetEval/hate
> 官方 test 上预注册 50/50 分层半分割（split seed 101），一半 test 分数
> 校准 target_prior、另一半标签评估，双向交叉；5 seeds × 2 probes ×
> 4 臂 × 2 规则。Primary accuracy：L12 `R12` logreg 5/10 正
> （`+1.90pp`）、SVC 4/10（`+1.62pp`），**inconclusive**；L6 `rescan`
> logreg 10/10（`+2.82pp`）**confirmed**、SVC 7/10（`+2.45pp`）
> **inconclusive**。Macro-F1 次要口径：L12 `R12` 5/10、4/10；
> L6 `rescan` 两 probe 均 10/10。Default 0.5 下 L12 `R12` 仍 0 正 /
> 5 负 / 5 跨（两 probe 均值 `−1.35/−1.44pp`）。**半分割没有确认
> `R12`；`rescan` 只在 L6/target_prior/logreg accuracy 过预注册门槛。
> target_prior 仍是 transductive；这是同一语料的标签半分割，不是
> 新数据集，不能替代独立语料 confirmatory。**
>
> **M1.26 基线审计（2026-11-26）**：SST-2 L12 的 `l12-pure-f32` 实为
> `TRIT_WEIGHT_BITS=2` 的 ternary/absmean 输出，不是 f32；§11.12–§11.25
> 中所有 SST-2 L12 的 `f32` 读数改读 §11.26。CoLA/hate L12 的
> `l12-pure-f32` 与真 f32 在 `2.1e-7` 舍入误差内一致，其旧结论不变。
> **M1.28 全量复核推翻 M1.26 的 L6 边界**：SST-2 L6 的 `l6-pure-f32`
> 也是 ternary（对 L6 真 ternary 余弦 `0.99999976`，对 L6 真 f32 只有
> `0.373686`）；官方 `l6-int4` 与真 bits4 在 `1.83e-7` 内一致。L6 的旧
> `int4−f32` 正向同样要按 ternary→4-bit 恢复读，详见 §11.28。
>
> **M1.27 残差矩阵复核（2026-11-26）**：用真 f32/int4 重跑 M1.23 的 32 臂；
> 真 f32 下没有任何合法残差臂超过 f32，`hybrid_rest_k16` 从旧表 `+3.95pp`
> 变为 `−0.66pp`。M1.23 的 `hybrid_rest_noclassmean` / `hybrid_rest_perm_class`
> 在 test 侧使用真标签，属泄漏/标签依赖对照；严格模式读数见 §11.27。
> 逐块 `3→4` 全量表（72 块 × 1,379 对）已跑完；把 `3→2` 供体表和 `3→4` 升级表合起来做同字节重排，得到 `R12`：6 int2 / 54 int3 / 12 int4，
> 总字节仍 `10,782,720 B`，全量 Pearson `0.833671`、Spearman `0.835411`。
> 相对 `F5F7V7` `+0.003287 / +0.006733`；Spearman 50k CI
> `[+0.002300, +0.011308]` 过 0，Pearson CI `[−0.000560, +0.007191]` 跨 0。
> 相对纯 int4：Pearson `−0.003834`、Spearman `+0.003364`；50k 两个 CI 都跨 0。
> 因此只能写“`R12` 的 Spearman 点估计反超 int4”，不能写“3-bit 追平/超过 int4”。
> 按 6 个来源拆开后，`R12` 对 int4 是 **0/6 胜**（全部持平或更低，MSRvid/
> headlines 各低 `−0.017`）；总体反超更像跨来源排序/校准效应（§9.4），不是
> 每个来源内部都更好。
> **128 仍反向**：`R12` 在 128 上是 `0.899405 / 0.929708`，低于 `K10K11`
> 的 `0.904783 / 0.937186`（`−0.005378 / −0.007479`）；128 只做 smoke。
> **M1.5 三套外部数据（SICK-R/STS12/STS13）没有单一赢家**：已测 6 个同字节
> 配置里，`v6e`/`I10` 三套外部点估计全正（`I10` 只有 SICK-R 双 10k CI 过 0）；
> `K10K11` 在 STS13 轻微负，`R12`/`R10`/`F5F7V7` 在 SICK-R 负。STS 全量仍是
> `R12 > K10K11 > I10`。所以 `I10` 是外部保守默认，`K10K11`/`R12` 是 STS
> in-domain 选择；不能再写“K10K11 在所有外部数据上最稳”（§10）。
> **M1.12 分类头（SST-2，冻结编码器 + 线性 probe）同样是负向**：L12 `R12`
> 对 int3 点估计低 `−0.008842`、CI 跨 0，对 int4 整段为负；L6 `rescan-k2`
> 对 int3/int4 的 CI 都跨 0。STS 的块级增益没有转成检索或分类增益，
> `R12`/`rescan-k2` 只能留在 STS in-domain（§11.12）。
> **M1.13 用第二种线性头（LinearSVC）复验**：L12 `int4−f32 = +0.043783`
> （CI `[+0.025261,+0.062603]`），`int3−f32 = +0.020769`（CI 全正），
> `R12−int3 = −0.004502`（CI 跨 0），`R12−int4 = −0.027516`（CI 整段为负）；
> L6 各档仍无显著差异。**L12 均匀 int4 高于 f32 不是 logistic 特有，但仍是
> 量化正则化/表示变化，不是 `R12` 的块级重排增益**（§11.13）。
> **M1.14 换第二个分类集（GLUE CoLA，dev 作 test）没有复现 int4 的正向**：
> CoLA dev 为 69/31，主口径改成 balanced class weight + macro-F1 选 C；
> L12 logreg `int4−f32 = +0.013341`（CI `[−0.006711,+0.033557]`），SVC
> `+0.008467`（CI `[−0.011505,+0.028763]`），L6 int4 与 f32 打平或略低。
> SST-2 的 +0.040/+0.044 同量级效应已落在 CoLA 的 95% CI 之外；`R12`/
> `rescan-k2` 仍无分类增益。**SST-2 的 int4 正向是单数据集/单 probe 家族的
> 观察，不能写成量化普遍提升线性分类**（§11.14）。
> **M1.15 第三个分类集（TweetEval/hate）也没有复现 int4 的正向**：
> 清洗后 8,985 train / 2,734 test；主口径与 SST-2 一致（无类别权重 +
> accuracy 选 C），L12 logreg `int4−f32 = −0.003679`
> （CI `[−0.012802,+0.005486]`），SVC `−0.004826`
> （CI `[−0.015362,+0.005852]`），点估计略负、CI 都跨 0；balanced
> 口径（macro-F1 选 C）反转为 `−0.012694`（logreg）/ `−0.013028`（SVC），
> 两个 CI 整段为负；fixed C=1 仍为 `−0.009860`。L6 balanced 两 probe
> 也没有一致优势（logreg `+0.006576`、CI 跨 0；SVC `−0.009173`、
> CI 整段为负）。hate 的 test 有 2,734 条，无权重口径 CI 上界只有
> 约 `+0.006`，SST-2 的 `+0.040/+0.044` 同量级效应被更严格地排除。
> **SST-2 的 int4 正向是数据集/任务/probe 特定的观察，不是量化普遍
> 提升线性分类；`R12`/`rescan-k2` 在第三个分类集上也没有任务头增益**
> （§11.15）。
> **M1.16 固定官方 test、重复从 train 抽 80% 重训：SST-2 的 L12 int4 正向
> 是训练子样本稳定的**：5 折 × 5 种子 = 25 次重训，L12 logreg
> `int4−f32` 均值 `+0.042987`（min `+0.028556`，25/25 为正），SVC
> `+0.044437`（min `+0.034596`，25/25 为正），与全量 test 的
> `+0.040/+0.044` 一致。L6 的 int4 只有 `+0.0063/+0.0036`，且分别
> 22/25、19/25 为正，仍有负值；`rescan-k2` 更接近 0。所以 SST-2 的
> 正向是 **L12 在该数据集上的稳定表示效应**，不是单次划分偶然；但它仍
> 没有跨到 CoLA/hate，也不是 L6 的普遍规律（§11.16）。
> **M1.28 L6 基线审计**：SST-2 L6 的官方 `l6-pure-f32` 也是 ternary，
> 不是 f32；真 f32 / 真 bits4 / 官方 ternary / 官方 int4 分别为
> `0.811642 / 0.806700 / 0.795717 / 0.806700`。真 bits4−真 f32 为
> `−0.004964`（CI `[−0.013729,+0.003844]`），官方 int4−官方 ternary 为
> `+0.010811`（CI `[−0.009336,+0.030752]`）；官方 int4 与真 bits4 逐条
> 预测完全相同。旧 L6 `int4−f32` 的正向同样是 ternary→4-bit 恢复，不是
> `int4` 超过真 f32（§11.28）。
> **M1.29 全臂矩阵**：把 L12 的 `int3` / `R12` 和 L6 的 `int3` / `rescan-k2`
> 也在真 f32 上重跑。结果没有任何量化档超过真 f32：L12 `int3` 为
> `−1.97pp`（CI 整段为负），`R12` 为 `−2.30pp`（CI 整段为负），`int4`
> 为 `−0.26pp`（CI 跨 0）；L6 的 `int3/int4/rescan` 分别为
> `−0.33/−0.50/−0.83pp`，CI 都跨 0。M1.12 的“量化档高于 f32”不只
> int4，int3/R12/rescan 也是 ternary 基线的恢复（§11.29）。
> **M1.30 L6 标签收口**：CoLA/hate 的 L6 `l6-pure-f32` 已用全量
> 9,594 / 11,719 条逐行确认是真 f32（余弦 `1.00000000`，max abs diff
> `1.8e-7 / 1.9e-7`），不是 ternary；官方 `l6-int4` 在 512 行上已与真
> bits4 逐位一致，M1.32 又补成全量（§11.32）。SST-2 是唯一 L12 与 L6 的 `pure-f32` 都错标成 ternary
> 的数据集；CoLA/hate 的 L6 分类结论无需重算（§11.30）。
> **M1.31 CoLA/hate 全臂矩阵**：用同一套 5 折选 C + test 只评一次 +
> 10k paired bootstrap，把 CoLA/hate 的 L12/L6 全部量化臂相对真 f32
> 重跑（logreg + LinearSVC）。**四个数据集×深度组合里没有任何量化臂
> 显著超过真 f32。** CoLA L12 的 int4 / true bits4 点估计为正
> （macro-F1 `+1.24pp` / `+1.20pp`），但 CI 跨 0；其余 CoLA L6、
> hate L12/L6 全为负或零，hate 的 `int3`/`R12`/`int4` 多处 CI 整段
> 为负。SST-2 的 `int4` 正向不是跨任务规律（§11.31）。
> **M1.32 L6 全量 bits4 收口**：CoLA/hate 的 L6 官方 `l6-int4` 已用
> 全量 9,594 / 11,719 行与真 bits4 逐行对齐（余弦 `1.00000000`，
> max abs diff `1.64e-7 / 1.53e-7`，全部行 ≤`1e-6`）。把 `true-bits4`
> 作为独立臂重跑后，CoLA L6 `−0.94pp`、hate L6 `−1.55pp`（logreg）
> 的结论不变，hate 的 CI 仍整段为负。M1.30 的 512 行抽检升级为全量，
> L6 不再有“官方 int4 只验证了首块”的边界（§11.32）。
> **M1.33 跨任务判决**：把 SST-2 / CoLA / hate 的 L12/L6 线性头结果
> 按“每个量化档只计一次”合成 36 个 `dataset×depth×probe×arm` 比较。
> **0 个的 95% CI 整段大于 0，9 个整段小于 0，27 个跨 0。** 最大正向
> 点估计仍是 CoLA L12 的 int4 `+1.20pp`（CI 跨 0）；SST-2 L12 的
> `int3`/`R12`、hate L12 的 `R12`、hate L6 的 `int4` 都是显著负向。
> 分类头没有可提升的量化档，f32 继续是默认锚点（§11.33）。
> **M1.38 域/阈值审计**：Hate 的 train→test 文本域分类器 AUC `0.921`
> （SST-2 `0.491`、CoLA `0.687`），长度单特征只有 `0.561`，长度分桶
> 准确率 `0.577/0.574/0.567/0.500`，>64 token 与 ≤64 只差 `1.13pp`：
> 20–22pp 差距的主因是文本域迁移，不是截断。把默认 0.5 阈值改成
> “目标分数分位匹配 train 先验”（不用 test 标签），Hate L12 f32
> `0.574→0.634`、L6 `0.546→0.634`；20pp 里约 `6–9pp` 是阈值漂移，
> 校准后仍差 `12–13pp`。全臂 36 比较在默认阈值下仍是 0 正 / 41 负 /
> 139 跨；目标先验阈值下变成 23 正 / 30 负 / 127 跨，6 个比较出现
> 正 CI，全部在 Hate：L12 `R12` logreg 5/5（`+1.83pp`）、SVC 4/5；
> L6 `rescan` 两 probe 5/5（`+2.55/+2.72pp`），`int4` SVC 3/5、
> `int3` logreg 1/5。SST-2/CoLA 仍 0 正。**默认 0.5 的官方 test 主判决
> 不变（f32 锚点）；目标先验阈值只给出 Hate-only、后验校准的条件性
> 候选，不能当通用量化增益。** 见 §11.38。


## 1. 结论先行

WSP-BUDGET 的第一版规划器按“权重重建误差代理”分配，在 128 对上反而低于纯 int3；
但全量 1,379 对是 `+0.001033 / +0.002893`，说明 128 子集不足以判死。

真正严格同字节、且全量更好的第一版是 `v6e`：

- 9 张 384×384 的 q/k 从 int3 升到 int4：层 0–3 的 query/key，加层 4 的 key；
- 3 张矩阵从 int3 降到 int2：层 9 的 `attention.output.dense`、层 11 的
  `intermediate.dense` 与 `output.dense`；
- 其余 60 张保持 int3；
- 总字节 `10,782,720`，与纯 int3 完全相等。

全量结果 `0.820686 / 0.818137`，相对纯 int3 `+0.002246 / +0.004775`。
配对 bootstrap 的 Spearman 下界 `+0.002017`，Pearson 下界 `-0.000307`；
所以 `v6e` 只能写 **Spearman-only 同字节小胜**。

随后新增 `scripts/wsp_budget_swap.py`，把 `v6e` 的 3 个降级块在同类同形状块之间
平移，保持总字节严格不变。128 对筛完 33 个交换后，全量验证得到 `I10`：

- 9 张 q/k 的 int4 升级与 `v6e` 相同；
- 层 9 `attention.output.dense` 与层 11 `output.dense` 仍降 int2；
- 把层 11 `intermediate.dense` 的 int2 平移到层 10 同名矩阵；
- 总字节仍为 `10,782,720`，`3 int2 / 60 int3 / 9 int4`。

`I10` 全量 `0.821621 / 0.818898`，相对纯 int3 两个指标都显著；
相对 `v6e` 点估计更高，但差值不显著。

M1.2 继续扫 int4 升级的位置：把 I10 的 Q3、K4 两个升级平移到层 10/11 的 key，
得到 `K10K11`：

- int4 保留 Q0–Q2、K0–K3，另加 K10、K11；
- 层 9 `attention.output.dense`、层 10 `intermediate.dense`、层 11 `output.dense`
  仍降 int2；
- 其余 60 张 int3，总字节仍 `10,782,720`。

`K10K11` 全量 `0.823479 / 0.820549`，相对纯 int3 `+0.005039 / +0.007187`；
相对 I10 `+0.001858 / +0.001651`，50k bootstrap 两个下界都勉强过 0
（Spearman 下界 `+0.000048`）。因此：

> **同字节下块的位置比权重重建误差更重要；位置不是靠代理猜，而是靠全量交换
> 消融测。`K10K11` 是 M1.2 的严格同字节优先点；它对 I10 的 Spearman 优势很薄，
> 对 v6e 的 Spearman 仍不显著，更不是“3-bit 追平 int4”。**

M1.3 不再手调单块，而是把纯 int3 的每一张线性矩阵单独降到 int2，跑全量
1,379 对，得到 72 行逐块敏感度表（§8）。这张表回答的是“该块单独降级会伤多少”，
不是“多块同时降级可以相加”：

- 29 块双指标上升、38 块双指标下降、5 块混合；`Δ字节` 为 `−18,432`（384×384）
  或 `−73,728`（384×1536）。
- 最耐降级的是层 5–9 的 `output.dense` / `value` / `intermediate.dense`：
  `7f +0.005461`、`7v +0.004741`、`8v +0.004321`、`5f +0.004127`（ΔSpearman）。
- 最不能碰的是 late key：`11k −0.009777`、`11v −0.008801`、`10k −0.004831`、
  `3k −0.004096`；`K10K11` 把 int4 放在 K10/K11，与这张表的敏感度方向一致。
- 原供体 `O9/I10/F11` 都是双升（`+0.001008 / +0.001834 / +0.001619`），
  确实耐降级，但不是最耐降级的三块。

按表把供体换成 `F5/F7/V7`（降 int2），int4 仍是 Q0–Q2、K0–K3、K10/K11，
其余 60 张 int3，得到 `F5F7V7`。总字节仍严格等于 `10,782,720 B`，计数仍是
`3 int2 / 60 int3 / 9 int4`。全量：

- `F5F7V7`：`0.830384 / 0.828678`；
- 相对 `K10K11`：`+0.006905 / +0.008129`，10k CI
  `[+0.002167, +0.011700]` / `[+0.002507, +0.013961]`，候选更好
  99.86% / 99.80%；
- 相对纯 int3：`+0.011944 / +0.015317`，10k CI
  `[+0.006778, +0.017179]` / `[+0.009036, +0.021733]`，候选更好 100%；
- 距纯 int4：点估计 `−0.007121 / −0.003369`，50k CI
  `[−0.015523, +0.001475]` / `[−0.013271, +0.006435]`，仍跨 0。

最关键的反例是 128 与全量**符号相反**：`F5F7V7` 在 128 上是
`0.898538 / 0.928299`，低于 `K10K11` 的 `0.904783 / 0.937186`，
即 `−0.006245 / −0.008888`；全量却高 `+0.006905 / +0.008129`。
如果按 128 排名，`F5F7V7` 会被直接淘汰。另一条独立证据是代理：
`F5F7V7` 与 `K10K11` 的 `proxy_err_ratio` 同为 `0.9155208682173497`，
位宽计数与形状完全相同，只换三个同形状供体的位置，全量 Spearman 却差
`+0.008129`。**代理看不见位置，128 还会反向排序；两者都不能当位置判据。**

> **M1.3 的严格同字节优先点更新为 `F5F7V7`。它是逐块全量敏感度表的第一个
> 可复算产物，不是“3-bit 追平 int4”：距 int4 点估计仍差
> `−0.007121 / −0.003369`，50k CI 跨 0。**

M1.4 把同一套逐块方法反过来做 `3→4`，得到 72 行升级表（§9）：45 块双升、
17 块双降、10 块混合。升级位置同样很不均匀：`K11` 升 int4 只有
`+0.000207` ΔSpearman，而 `10v +0.002110`、`8o +0.001236`、`1k +0.001106`、
`6o +0.000982`、`11v +0.000948` 更值；384×1536 里 `7i +0.002763` 最强，但
一个槽要花 `73,728 B`。**更多 int4 不等于更好：`R15`（15 int4）比
`R12`（12 int4）在两个指标上都低，说明位置比数量重要。**

把 `3→2` 供体表和 `3→4` 升级表合起来做同字节重排：

| 配置 | int2 | int3 | int4 | 全量 Pearson | 全量 Spearman | 距 int4 P / S |
|---|---:|---:|---:|---:|---:|---:|
| `F5F7V7` | 3 | 60 | 9 | 0.830384 | 0.828678 | −0.007121 / −0.003369 |
| `R9` | 3 | 60 | 9 | 0.832876 | 0.831677 | −0.004629 / −0.000370 |
| `R10` | 4 | 58 | 10 | 0.833002 | 0.834377 | −0.004503 / +0.002329 |
| **`R12`** | **6** | **54** | **12** | **0.833671** | **0.835411** | **−0.003834 / +0.003364** |
| `R15` | 9 | 48 | 15 | 0.832161 | 0.835268 | −0.005344 / +0.003221 |
| 纯 int4 | 0 | 0 | 72 | 0.837505 | 0.832047 | — |

`R12` 的组成：供体 F5/F7（两张 f）+ V7/V8/V9/V6（四张 v）降 int2；
int4 是 `1k,10v,8o,6o,11v,9o,7o,0o,5k,4v,6q,0v`，其余 54 张 int3。
字节账严格相等：2 张 f × `73,728` + 4 张 384×384 × `18,432` = `221,184`；
12 张 384×384 int4 = `221,184`；实际总字节仍是 `10,782,720 B`。

`R12` 相对 `F5F7V7` 是 `+0.003287 / +0.006733`；50k 配对 bootstrap 的
Spearman CI `[+0.002300, +0.011308]` 过 0，Pearson CI
`[−0.000560, +0.007191]` 跨 0。相对纯 int4 是
Pearson `−0.003834`、Spearman `+0.003364`；50k 两个 CI 都跨 0。
所以当前只能写“`R12` 的 Spearman 点估计反超 int4、Pearson 仍低”，
不能写“3-bit 全面追平/超过 int4”。按 6 个来源拆开后，`R12` 对 int4 是
**0/6 胜**（全部持平或更低，MSRvid/headlines 各低 `−0.017`），总体反超更像是
跨来源的排序/校准效应（§9.4），不是逐来源都更好。`R12` 对 `R10` 的增量也只有点估计
`+0.000670 / +0.001035`，50k CI 跨 0，不能称已证优于 R10。

M1.5 换到三套外部数据（SICK-R/STS12/STS13，各前 1,000 对）后，结论不是
“某个候选全面最稳”：`R12` 在 SICK-R 双负（`−0.004961 / −0.004868`），但在
STS12/STS13 都正（STS13 `+0.021556 / +0.017861`，双 10k CI 过 0）；`K10K11`
在 SICK-R/STS12 正，在 STS13 轻微负（`−0.000308 / −0.000291`）；只有
`v6e`/`I10` 三套外部点估计全正。**所以 `R12` 只能算 STS benchmark 的
in-sample 点估计，`K10K11` 是 STS in-domain 优先点，`I10` 是外部保守默认；
没有单一候选同时赢两边（§10）。**

M1.9 的相对深度规则在 L12→L6 上看起来可用（`0.828707 / 0.824484`，对 int3
双 CI 过 0），但 M1.10 用两个新模型把这条规则打回“模型对特定”：同一套 L12 块
布局在 3 层 L3 上比纯 int3 低 `−0.012450 / −0.010778`（CI 整段为负），在
同深度不同训练目标的 par-L6 上低 `−0.016161 / −0.007308`；连 L6 自己的最优
`6/24/6` 布局投到 par-L6 也是 `−0.011203 / −0.008646`。**同一布局在两个 6 层
模型上相对各自 int3 的 Δ 从 `+0.014/+0.014` 翻成 `−0.016/−0.007`；L12→L6
的正向不能写成跨模型规则，只能当这对模型的候选生成器（§11.10）。**

M1.11 按这个方向转到检索任务头：`R12` 在 STS 上相对 int3 是
`+0.015231 / +0.022050`，L6 `rescan-k2` 是 `+0.016710 / +0.014032`，都是各自
STS 的强候选；但换到 SciFact 检索后，`R12` 的 MRR/nDCG@10 点估计低于 int3，
对 int4/f32 的 MRR/nDCG CI 基本整段为负；`rescan-k2` 对 int3 的 MRR/nDCG@10
CI 也整段为负。**STS 相关性的块级增益没有转到检索；任务头和 STS 不是同一个
判据（§11.11）。** 分类头见 §11.12–§11.21。

## 2. 字节账

预算固定为纯 int3 的**实际工件字节**，不是 weight-only：

| 项 | 字节 |
|---|---:|
| 权重（int3，`ceil(rows*cols*3/8)`） | 7,962,624 |
| per-group f32 scale + per-row f32 bias | 2,820,096 |
| **实际总字节** | **10,782,720** |
| 纯 int4 实际总字节 | 13,436,928 |

字节公式（`group=32`）：

```text
weight_bytes = ceil(rows * cols * bits / 8)
metadata     = rows * (ceil(cols / 32) * 4 + 4)
actual       = weight_bytes + metadata
```

`v6e` / `I10` / `K10K11` 的逐块变化：

| 变化 | 块数 | 单块字节变化 | 合计 |
|---|---:|---:|---:|
| int3 → int4（384×384 q/k，共 9 张） | 9 | +18,432 | +165,888 |
| int3 → int2（384×384 o，层 9） | 1 | −18,432 | −18,432 |
| int3 → int2（1536×384 i，层 10/11） | 1 | −73,728 | −73,728 |
| int3 → int2（384×1536 f，层 11） | 1 | −73,728 | −73,728 |
| **净变化** | | | **0** |

M1.3 的 `F5F7V7` 把供体换成层 5 `output.dense`、层 7 `output.dense` 与层 7
`value`：两张 384×1536 f 各省 `73,728`，一张 384×384 v 省 `18,432`，
合计 `165,888`；9 张 384×384 q/k 升 int4 仍花 `165,888`，净变化为 0。
逐块字节：

| 变化 | 块数 | 单块字节变化 | 合计 |
|---|---:|---:|---:|
| int3 → int4（384×384 q/k，共 9 张） | 9 | +18,432 | +165,888 |
| int3 → int2（384×1536 f，层 5/7） | 2 | −73,728 | −147,456 |
| int3 → int2（384×384 v，层 7） | 1 | −18,432 | −18,432 |
| **净变化** | | | **0** |

M1.4 的 `R12` 用 2 张 384×1536 f（F5/F7）+ 4 张 384×384 v（V7/V8/V9/V6）作供体，
共省 `221,184 B`；12 张 384×384 int4 正好花 `221,184 B`，净变化仍为 0。

| 变化 | 块数 | 单块字节变化 | 合计 |
|---|---:|---:|---:|
| int3 → int4（384×384） | 12 | +18,432 | +221,184 |
| int3 → int2（384×1536 f，层 5/7） | 2 | −73,728 | −147,456 |
| int3 → int2（384×384 v） | 4 | −18,432 | −73,728 |
| **净变化** | | | **0** |

单块字节：384×384 的 int2/int3/int4 分别是 `56,832 / 75,264 / 93,696`；
1536×384 的 int3/int2 是 `301,056 / 227,328`；384×1536 的 int3/int2 是
`296,448 / 222,720`。

| 方案 | int2 | int3 | int4 | int5 | 实际字节 | 相对 int3 |
|---|---:|---:|---:|---:|---:|---:|
| 纯 int3 | 0 | 72 | 0 | 0 | 10,782,720 | 0 |
| 规划器 v1 | 3 | 60 | 9 | 0 | 10,782,720 | 0 |
| **v6e** | **3** | **60** | **9** | **0** | **10,782,720** | **0** |
| **I10** | **3** | **60** | **9** | **0** | **10,782,720** | **0** |
| **K10K11** | **3** | **60** | **9** | **0** | **10,782,720** | **0** |
| **F5F7V7** | **3** | **60** | **9** | **0** | **10,782,720** | **0** |
| **R9** | **3** | **60** | **9** | **0** | **10,782,720** | **0** |
| **R10** | **4** | **58** | **10** | **0** | **10,782,720** | **0** |
| **R12** | **6** | **54** | **12** | **0** | **10,782,720** | **0** |
| **R15** | **9** | **48** | **15** | **0** | **10,782,720** | **0** |
| v6（非严格） | 3 | 59 | 10 | 0 | 10,801,152 | +18,432 |
| 纯 int4 | 0 | 0 | 72 | 0 | 13,436,928 | +2,654,208 |

`v6e` 与规划器 v1 的区别只有一处：v1 把层 11 的 `attention.output.dense` 降 int2，
v6e 把这一个降级挪到层 9 的同一类矩阵。两者形状、字节、代理误差比完全相同
（`proxy_err_ratio=0.91552`），但 STS 不同——这正是代理看不见位置。
`I10` 与 `v6e` 的区别也只有一处：把层 11 的 `intermediate.dense` 平移到层 10，
形状、字节、代理误差比仍然相同。
`K10K11` 再把 Q3、K4 两个 int4 升级平移到 K10、K11；全部 9 张 int4 仍是同形状，
总字节、`proxy_err_ratio=0.91552` 仍然不变，只有位置变了。
`F5F7V7` 保持同样的 9 张 int4，只把三个供体换成 F5、F7、V7；字节与代理比仍然
完全相同，但全量 Spearman 比 K10K11 高 `+0.008129`，而 128 反而低
`−0.008888`。这组对照把“代理看不见位置”和“128 会反向排序”同时钉在同一对预算上。
`R9`/`R10`/`R12`/`R15` 再把 int4 槽按 3→4 表迁到更值的位置，同时增加供体数；
四者的 `proxy_err_ratio` 仍全部是 `0.9155208682173497`，字节也全部是
`10,782,720 B`，但全量从 `0.830384` 一路到 `0.833671`，说明代理对
“位数 + 位置”这一族完全不敏感。

运行时报告现在也按预算图复算实际字节（`crates/trit-wspace/examples/bert6/run/weight_format/`）：
`TRIT_BUDGET` 存在时 `weight_bytes` 改为 `actual_bytes_with_metadata`，
并额外给出 `budget_actual_bytes / budget_weight_bytes / budget_metadata_bytes /
mixed_bits / budget_group`。L12/K10K11 的 128 smoke 报告为：
`budget_actual_bytes=10,782,720`、`budget_weight_bytes=7,962,624`、
`budget_metadata_bytes=2,820,096`、`mixed_bits={2:3,3:60,4:9}`、
`ratio_vs_f32=0.126953125`。用这个新报告重跑 K10K11 全量，STS 仍是
`0.823478805209325 / 0.820549002888985`，与旧结果逐位一致。
`F5F7V7` 全量的 `weight_format` 也报同一组字节：`budget_actual_bytes=10,782,720`、
`budget_weight_bytes=7,962,624`、`budget_metadata_bytes=2,820,096`、
`mixed_bits={2:3,3:60,4:9}`、`ratio_vs_f32=0.126953125`；STS 为
`0.830384279266153 / 0.828678339743033`。`R12` 全量的 `weight_format` 为
`budget_actual_bytes=10,782,720`、`budget_weight_bytes=7,962,624`、
`budget_metadata_bytes=2,820,096`、`mixed_bits={2:6,3:54,4:12}`、
`ratio_vs_f32=0.126953125`；STS 为
`0.8336713586694353 / 0.8354114671958606`。注意 `weight_format.bits` 顶层仍是
`TRIT_WEIGHT_BITS=3`（全局默认），真实分布只看 `mixed` 与 `mixed_bits`。

## 3. 读数

128 对（用于 smoke，不作最终判据）：

| 配置 | Pearson | Spearman | 实际字节 |
|---|---:|---:|---:|
| 纯 int3 | 0.897582 | 0.927222 | 10,782,720 |
| 规划器 v1 | 0.885716 | 0.922785 | 10,782,720 |
| **v6e** | **0.901878** | **0.931000** | 10,782,720 |
| **I10** | **0.903645** | **0.935691** | 10,782,720 |
| **K10K11** | **0.904783** | **0.937186** | 10,782,720 |
| **F5F7V7** | **0.898538** | **0.928299** | 10,782,720 |
| **R9** | **0.898744** | **0.928969** | 10,782,720 |
| **R10** | **0.895217** | **0.927459** | 10,782,720 |
| **R12** | **0.899405** | **0.929708** | 10,782,720 |
| F3（v6e 的对照，f 降到层 3） | 0.905404 | 0.934399 | 10,782,720 |
| 纯 int4 | 0.928494 | 0.950720 | 13,436,928 |

全量 1,379 对：

| 配置 | Pearson | Spearman | 实际字节 | 同字节？ |
|---|---:|---:|---:|---|
| 纯 int3 | 0.818440 | 0.813362 | 10,782,720 | 是 |
| 规划器 v1 | 0.819473 | 0.816255 | 10,782,720 | 是 |
| **v6e** | **0.820686** | **0.818137** | 10,782,720 | 是 |
| **I10** | **0.821621** | **0.818898** | **10,782,720** | **是** |
| K4→K11（只换一个升级） | 0.821805 | 0.818940 | 10,782,720 | 是 |
| Q3→K11（只换一个升级） | 0.822164 | 0.819658 | 10,782,720 | 是 |
| **K10K11** | **0.823479** | **0.820549** | **10,782,720** | **是** |
| **F5F7V7** | **0.830384** | **0.828678** | **10,782,720** | **是** |
| **R9** | **0.832876** | **0.831677** | **10,782,720** | **是** |
| **R10** | **0.833002** | **0.834377** | **10,782,720** | **是** |
| **R12** | **0.833671** | **0.835411** | **10,782,720** | **是** |
| **R15** | **0.832161** | **0.835268** | **10,782,720** | **是** |
| F3（只换 f） | 0.822474 | 0.817666 | 10,782,720 | 是 |
| I10+F3（两个交换） | 0.823251 | 0.817221 | 10,782,720 | 是 |
| v6（非严格） | 0.820745 | 0.818020 | 10,801,152 | 否，+18,432 |
| 纯 int4 | 0.837505 | 0.832047 | 13,436,928 | 否 |
| 纯 f32 | 0.837595 | 0.830894 | 84,934,656 | 否 |

相对纯 int3 的全量增量：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| v1 | +0.001033 | +0.002893 |
| v6e | +0.002246 | +0.004775 |
| **I10** | **+0.003180** | **+0.005536** |
| **K10K11** | **+0.005039** | **+0.007187** |
| **F5F7V7** | **+0.011944** | **+0.015317** |
| **R9** | **+0.014436** | **+0.018316** |
| **R10** | **+0.014562** | **+0.021015** |
| **R12** | **+0.015231** | **+0.022050** |
| **R15** | **+0.013721** | **+0.021906** |
| F3 | +0.004033 | +0.004304 |
| I10+F3 | +0.004811 | +0.003859 |
| v6（非严格） | +0.002304 | +0.004659 |

相对 `v6e` 的增量：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| **I10** | **+0.000934** | **+0.000761** |
| F3 | +0.001787 | −0.000471 |
| I10+F3 | +0.002565 | −0.000916 |

相对 `I10` 的增量：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| K4→K11 | +0.000185 | +0.000042 |
| Q3→K11 | +0.000543 | +0.000759 |
| **K10K11** | **+0.001858** | **+0.001651** |

相对 `K10K11` 的增量：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| **F5F7V7** | **+0.006905** | **+0.008129** |

相对 `F5F7V7` 的增量（M1.4 同字节重排）：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| **R9** | **+0.002492** | **+0.002999** |
| **R10** | **+0.002618** | **+0.005698** |
| **R12** | **+0.003287** | **+0.006733** |
| **R15** | **+0.001777** | **+0.006590** |

相对纯 int4 的差值（负表示低于 int4）：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| **R9** | **−0.004629** | **−0.000370** |
| **R10** | **−0.004503** | **+0.002329** |
| **R12** | **−0.003834** | **+0.003364** |
| **R15** | **−0.005344** | **+0.003221** |

`K10K11` 距纯 int4：Pearson `−0.014026`、Spearman `−0.011498`；距 f32：
Pearson `−0.014116`、Spearman `−0.010345`。`F5F7V7` 距纯 int4：
Pearson `−0.007121`、Spearman `−0.003369`；50k CI 仍跨 0。
`R12` 距纯 int4：Pearson `−0.003834`、Spearman `+0.003364`；50k CI 仍都跨 0。
**预算分配把 Pearson 差距缩小到 `−0.0038`，并让 Spearman 点估计反超
`+0.0034`，但两个指标都还没有统计分离，不能写“追平/超过 int4”。**

`F5F7V7` 在 128 上是 `0.898538 / 0.928299`，比 K10K11 低
`−0.006245 / −0.008888`；全量却高 `+0.006905 / +0.008129`。
`R12` 在 128 上是 `0.899405 / 0.929708`，仍比 K10K11 低
`−0.005378 / −0.007479`，全量 Spearman 却高 `+0.014862`（对 K10K11）。
128 同一个方向再次把候选排到 K10K11 之后，不能当位置判据。

F3 在 128 对上是 `+0.003526 / +0.003400`（相对 v6e），全量 Pearson 仍为正，
Spearman 却变成 `−0.000471`；I10+F3 也是 Pearson 更高、Spearman 更低。
这说明“把 128 对的单块增量相加”不能预测全量组合。

## 4. 配对 bootstrap

统一输入：`l12-int3-preds-full.json` / `l12-budget-v6e-preds-full.json` /
`l12-budget-i10-preds-full.json` 作 baseline，`l12-budget-i10-preds-full.json` /
`l12-budget-k10k11-preds-full.json` 作 candidate；标签是
`miniLM-l12-sts-full/pairs.json` 的 `pairs[].score`；预测路径
`modes.f32.sts.preds`；默认 `10,000` 次按 pair 有放回重采样，seed `20261124`。
`K10K11` 对 I10 的对比额外跑 `50,000` 次，因为 Spearman 下界很薄。

### 4.1 v6e vs 纯 int3

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.818440 | 0.820686 | +0.002246 | +0.002263 | [−0.000307, +0.004941] | 95.71% |
| **Spearman** | **0.813362** | **0.818137** | **+0.004775** | **+0.004808** | **[+0.002017, +0.007754]** | **99.98%** |

### 4.2 I10 vs 纯 int3

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| **Pearson** | **0.818440** | **0.821621** | **+0.003180** | **+0.003191** | **[+0.000157, +0.006370]** | **97.99%** |
| **Spearman** | **0.813362** | **0.818898** | **+0.005536** | **+0.005565** | **[+0.002192, +0.009004]** | **99.92%** |

### 4.3 I10 vs v6e

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.820686 | 0.821621 | +0.000934 | +0.000928 | [−0.001124, +0.002988] | 80.69% |
| Spearman | 0.818137 | 0.818898 | +0.000761 | +0.000757 | [−0.001653, +0.003163] | 73.76% |

### 4.4 K10K11 vs 纯 int3

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| **Pearson** | **0.818440** | **0.823479** | **+0.005039** | **+0.005054** | **[+0.001919, +0.008219]** | **99.89%** |
| **Spearman** | **0.813362** | **0.820549** | **+0.007187** | **+0.007219** | **[+0.003737, +0.010819]** | **99.99%** |

### 4.5 K10K11 vs I10（50k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.821621 | 0.823479 | +0.001858 | +0.001859 | [+0.000591, +0.003120] | 99.77% |
| Spearman | 0.818898 | 0.820549 | +0.001651 | +0.001653 | [+0.000048, +0.003272] | 97.82% |

### 4.6 K10K11 vs v6e（10k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.820686 | 0.823479 | +0.002793 | +0.002791 | [+0.000420, +0.005209] | 98.91% |
| Spearman | 0.818137 | 0.820549 | +0.002412 | +0.002411 | [−0.000466, +0.005287] | 95.08% |

### 4.7 F5F7V7 vs K10K11（10k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.823479 | 0.830384 | +0.006905 | +0.006892 | [+0.002167, +0.011700] | 99.86% |
| **Spearman** | **0.820549** | **0.828678** | **+0.008129** | **+0.008109** | **[+0.002507, +0.013961]** | **99.80%** |

### 4.8 F5F7V7 vs 纯 int4（50k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.837505 | 0.830384 | −0.007121 | −0.007075 | [−0.015523, +0.001475] | 5.22% |
| Spearman | 0.832047 | 0.828678 | −0.003369 | −0.003331 | [−0.013271, +0.006435] | 25.35% |

### 4.9 R12 vs F5F7V7（50k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.830384 | 0.833671 | +0.003287 | +0.003305 | [−0.000560, +0.007191] | 95.25% |
| **Spearman** | **0.828678** | **0.835411** | **+0.006733** | **+0.006746** | **[+0.002300, +0.011308]** | **99.86%** |

### 4.10 R12 vs R10（50k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.833002 | 0.833671 | +0.000670 | +0.000676 | [−0.001485, +0.002863] | 72.73% |
| Spearman | 0.834377 | 0.835411 | +0.001035 | +0.001042 | [−0.001341, +0.003415] | 80.53% |

### 4.11 R12 vs 纯 int4（50k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.837505 | 0.833671 | −0.003834 | −0.003770 | [−0.013283, +0.005974] | 21.87% |
| Spearman | 0.832047 | 0.835411 | +0.003364 | +0.003415 | [−0.007538, +0.014370] | 72.93% |

### 4.12 R12 vs 纯 int3（10k）

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.818440 | 0.833671 | +0.015231 | +0.015233 | [+0.008269, +0.022347] | 100% |
| Spearman | 0.813362 | 0.835411 | +0.022050 | +0.022058 | [+0.013825, +0.030437] | 100% |

判据（见 `WSP-ALT.md` §5.4）是“Pearson/Spearman 不低于纯 int3，且至少一个指标
10k CI 下界 > 0”。`v6e` 只有 Spearman 过；`I10` 两个指标都过；`K10K11` 对纯 int3
两个指标都过，对 I10 的 50k CI 两个下界也过 0。`F5F7V7` 对纯 int3 与 K10K11
都是双指标 10k CI 下界过 0；但它对纯 int4 的两个 50k CI 仍跨 0。
`R12` 对纯 int3 双指标显著；对 `F5F7V7` 只有 Spearman 过 0；对 `R10` 两个
50k CI 都跨 0；对纯 int4 也是两个 CI 都跨 0（Spearman 点估计为正）。
所以当前写法是：

> `R12` 是 M1.4 同字节点估计最好的配置：Pearson `0.833671`、Spearman `0.835411`；
> 对 F5F7V7 的 Spearman 优势显著，Pearson 只是点估计更高；对 R10、对 int4 都
> 没有统计分离。它可以写“Spearman 点估计反超纯 int4”，不能写
> “3-bit 全面追平/超过 int4”。128 上它还低于 K10K11，不能拿 128 排名否定它。

## 5. 解释与失败记录

1. **128 子集不可靠**：v1 在 128 上是 `0.885716`，低于 int3 的 `0.897582`；
   全量却是 `0.819473`，高于 int3。若只看 128 会误杀一个全量为正的方案。
   后续所有 BUDGET 变体至少跑全量，128 只做编译/smoke。
2. **代理与 STS 不同向**：规划器按权重重建 MSE 选块，把层 11 的 attention output
   降 int2；`proxy_err_ratio=0.91552`（−8.45%），但端到端没有同步收益。v6e 与 v1
   代理比完全相同，仅换一个同形状块的位置，全量 Spearman 差 `0.001882`。
   这说明代理至少缺两项：块在残差流中的位置，以及它对最终句向量的影响。
3. **不要过度归因“层 11 o 有毒”**：v1 全量仍为正，128 的坏读数不能证明这一点；
   目前只能证明“在这组 9 升 3 降的配置里，把 int2 放在层 9 o 比层 11 o 的全量结果好”。
   单块机制需要逐块 STS 扰动表才能确认。
4. **层 4 query 的 int4 升级没有收益**：v6 多花 18,432 B，128/全量都不比 v6e 好；
   在预算紧时优先删这类“同形状、非末端”的升级。
5. **第一版规划器的失败不是实现 bug**：它的字节账、分配、运行时接线都能复算；
   失败在敏感度代理的语义，不在加载器。运行时能按 `TRIT_BUDGET` 逐块选 2/3/4 位，
   已编译并跑通两档数据。
6. **块交换的 128 排名也会高估**：I10 在 128 上是 `+0.001766 / +0.004692`，
   全量只有 `+0.000934 / +0.000761`；F3 在 128 上双指标为正，
   全量 Spearman 转负。K10K11 在 128 上是 `+0.001138 / +0.001495`，
   全量 `+0.001858 / +0.001651`，这次 128 反而低估。128 只能筛候选，不能定胜负。
7. **单块表补上了，但仍不是多块模型**：M1.3 已把 72 个块逐个做 `3→2` 全量，
   覆盖了全部线性矩阵；但仍只测“单独降一块”，没有覆盖多块同时降级的交互。
   不能把表里的 `Δ` 直接相加来预测组合；组合必须像 `F5F7V7` 一样整包跑全量。
   另外，M1.3 的 2-bit 走运行时三值 `QuantLinear`，不是 uniform 2-bit；表测的是
   三值路径的实际伤害，不能当 uniform 2-bit 的误差。
8. **K10K11 的 Spearman 优势很薄**：对 I10 的 50k CI 下界 `+0.000048`，
   对 v6e 的 10k CI 跨 0；它更像“late key 位置略好”的点估计，不是强机制结论。
   “K9 再加一个 late key”在 128 上只有 `+0.0002–0.0003`，没有继续全量追。

## 6. 边界

- **单模型、单数据**：all-MiniLM-L12-v2，STS 1,379 对；128 对只作 smoke。
  没有第二模型、没有检索/分类任务头，不能外推“BUDGET 普遍有效”。
- **非 sentence-disjoint**：BUDGET 代理只用权重，不用校准标签；但评测对与之前
  BIAS/SCALE 实验复用同一 STS 集，仍属探索性读数，不是留出验证。
- **2-bit 语义不完全一致**：规划器按“uniform 2-bit + per-group scale”计费；
  运行时 `bits==2` 实际走已有的三值 `QuantLinear`（absmean/GPTQ 三值），
  两者字节数相同，但误差语义不同。2-bit 块的代理误差不能当三值路径真实误差。
- **字节账不含容器对齐/打包**：只算线性权重 + scale + bias；实际格式若加头、
  对齐、索引，总字节会变。运行时报告已把 mixed 的 `actual_bytes_with_metadata`
  接进 `weight_format`（`budget_actual_bytes` / `mixed_bits`），但仍是离线公式值，
  不是容器序列化后的字节。
- **只有一种分配族**：9 升 3 降；128 上扫了 O/I/F 的 33 个降级交换、Q3/K4 各 15 个
  升级目标。M1.3 与 M1.4 的 72 块单块表覆盖了全部线性矩阵，但整包只验证了
  `F5F7V7` / `R9` / `R10` / `R12` / `R15` 等少数组合；没有穷举 72 块的组合，
  也没有 int5/f32/侧车候选。
- **单块 Δ 不能相加**：`F5F7V7` 的三块单独 ΔSpearman 为
  `+0.004127 / +0.005461 / +0.004741`，合计 `+0.014329`；整包相对纯 int3 是
  `+0.015317`，看似接近，但这是单个组合的一次读数，不能当可加性证明。
  多块组合必须整包跑全量，不能拿 72 行表做线性预测。M1.4 的 `R12`/`R15`
  也是整包验证；3→4 表的单块 ΔSpearman 同样不可相加。
- **`F5F7V7` 与 `R12` 的 128 都低于 K10K11**：`F5F7V7`
  `−0.006245 / −0.008888`、`R12` `−0.005378 / −0.007479`，全量却分别高
  `+0.006905 / +0.008129` 与 `+0.010192 / +0.014862`；128 只能 smoke。
- **`R12` 没有追平 int4**：STS 1,379 对上 Pearson 仍差 `−0.003834`，Spearman
  点估计高 `+0.003364`，但 50k 两个 CI 都跨 0；候选更好比例 `21.9% / 72.9%`。
  按 6 个来源拆开，`R12` 对 int4 是 **0/6 胜**，MSRvid/headlines 各低 `−0.017`；
  总体 Spearman 反超更像跨来源排序/校准效应，不是逐来源更好。不能写
  “3-bit 全面追平/超过 int4”。
- **外部三套数据推翻“单一最稳”**：SICK-R 上 `R12`/`R10`/`F5F7V7` 负，
  `K10K11`/`I10`/`v6e` 正；STS12 上六套多为正；STS13 上 `R12`/`R10`/`F5F7V7`
  显著正、`K10K11` 轻微负。已测 6 个同字节配置里只有 `v6e`/`I10` 三套点估计
  全正；但 `I10` 只有 SICK-R 双 CI 过 0，STS12/13 仍跨 0。三套都有句子跨
  pair 复用，不是 sentence-disjoint；不能写“K10K11 在所有外部数据上最稳”（§10）。
- **`R12` 对 `R10` 也没有统计分离**：点估计 `+0.000670 / +0.001035`，
  50k 两个 CI 都跨 0；`R12` 只是当前点估计最好，不能称“已证优于 R10”。
- **`R12` 是 in-sample 搜索**：供体与 int4 槽都在同一 1,379 对上挑出，又在同一批
  pairs 上评测；没有 held-out、sentence-disjoint 或第二模型。它不能当留出验证。
- **升级收益不均匀且非单调**：`11k` 升 int4 只有 `+0.000207`，`8v`/`3v` 等块
  升 int4 反而双降；`R15` 的 15 个 int4 在两个指标上都低于 `R12` 的 12 个。
  这些只是 L12/STS 单模型观察，不能外推成“int4 数量规律”。
- **I10 与 v6e 的差距不显著**：两者只差一个同形状 i 块的位置，STS 全量 10k
  bootstrap 两个指标都跨 0；外部三套 I10 在 SICK-R/STS12 高于 v6e，STS13
  略低，但没做 I10-vs-v6e 的 bootstrap，不能写“I10 已证优于 v6e”。
- **K10K11 是 in-domain 优先点，不是外部全胜**：STS 全量对 I10
  `+0.001858 / +0.001651`，50k CI 下界都过 0；外部 SICK-R 也显著正。但 STS13
  上 I10 对 K10K11 `+0.002435 / +0.002197`，双 10k CI 过 0；SICK-R/STS12
  两者 CI 都跨 0。所以 K10K11 只在 STS in-domain 更稳，不能当外部默认。
- 没有公开文献逐篇核对；混合精度、预算分配都不是新原理。本文只声明本仓同口径的
  L12/STS 读数，不声明“首次/独有”。

## 7. 复算

```bash
# 1) 规划器：预算 = 纯 int3 的实际字节
python scripts/wsp_budget.py \
  --model /path/all-MiniLM-L12-v2.safetensors \
  --budget 10782720 --out budget-l12-int3.json

# 2) 严格同字节 v6e：9 个 q/k 升 int4，3 个块降 int2；其余重置为 int3
python scripts/wsp_budget_variant.py \
  --base budget-l12-int3.json \
  --up 0:q,0:k,1:q,1:k,2:q,2:k,3:q,3:k,4:k \
  --down 9:o,11:i,11:f \
  --require-exact --out budget-l12-int3-v6e.json

# 3) 同字节交换 I10：把层 11 i 的 int2 平移到层 10 i
python scripts/wsp_budget_swap.py \
  --base budget-l12-int3-v6e.json \
  --move 11:i>10:i --out-dir swap-i10
# 产物 swap-i10/swap-11ito10i.json，actual_bytes 仍为 10,782,720

# 4) 跑全量并导出 preds（Windows PowerShell 先逐条 $env:TRIT_...）
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=swap-i10/swap-11ito10i.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-full-preds.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full --activation-mode f32 \
  --out l12-budget-i10-preds-full.json

# 5) 配对 bootstrap：I10 vs 纯 int3 / I10 vs v6e
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-int3-preds-full.json \
  --candidate l12-budget-i10-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out bootstrap-i10-vs-int3.json
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-budget-v6e-preds-full.json \
  --candidate l12-budget-i10-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out bootstrap-i10-vs-v6e.json

# 6) K10K11：在 I10 上把 Q3、K4 两个 int4 升级移到 K10、K11
python scripts/wsp_budget_swap.py \
  --base swap-i10/swap-11ito10i.json \
  --move 3:q>10:k --move 4:k>11:k --out-dir k10k11
# 产物 k10k11/swap-3qto10k-4kto11k.json，actual_bytes 仍为 10,782,720

# 7) 跑 K10K11 全量并导出 preds
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=k10k11/swap-3qto10k-4kto11k.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-full-preds.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full --activation-mode f32 \
  --out l12-budget-k10k11-preds-full.json

# 8) K10K11 的配对 bootstrap
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-int3-preds-full.json \
  --candidate l12-budget-k10k11-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out bootstrap-k10k11-vs-int3.json
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-budget-i10-preds-full.json \
  --candidate l12-budget-k10k11-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json --iters 50000 \
  --out bootstrap-k10k11-vs-i10-50k.json
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-budget-v6e-preds-full.json \
  --candidate l12-budget-k10k11-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out bootstrap-k10k11-vs-v6e.json

# 9) M1.3：纯 int3 上逐块 3→2，跑 72 个全量变体并汇总
python scripts/wsp_budget_perturb.py \
  --base budget-l12-pure-int3.json --bits 2 \
  --out-dir perturb-full-3to2 \
  --base-result l12-int3-preds-full.json \
  --run --exe target/release/examples/bert6.exe \
  --model all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full \
  --f32-cache l12-f32-cache-full-preds.json \
  --threads 6 --jobs 4
# 产物 perturb-full-3to2/perturb-summary.json，72 行；Δ 已按实际字节复算

# 10) F5F7V7：F5/F7 两张 384×1536、V7 一张 384×384 降 int2；
#     int4 仍是 Q0–Q2、K0–K3、K10/K11，总字节严格不变
python scripts/wsp_budget_variant.py \
  --base budget-l12-pure-int3.json \
  --up 0:q,0:k,1:q,1:k,2:q,2:k,3:k,10:k,11:k \
  --down 5:f,7:f,7:v \
  --require-exact --out f5f7v7/swap-f5f7v7.json

# 11) 跑 F5F7V7 全量并导出 preds
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=f5f7v7/swap-f5f7v7.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-full-preds.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full --activation-mode f32 \
  --out f5f7v7/f5f7v7-full.json

# 12) F5F7V7 的配对 bootstrap：对 int3 / K10K11 / int4
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-int3-preds-full.json \
  --candidate f5f7v7/f5f7v7-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out f5f7v7/bootstrap-f5f7v7-vs-int3.json
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-budget-k10k11-preds-full.json \
  --candidate f5f7v7/f5f7v7-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out f5f7v7/bootstrap-f5f7v7-vs-k10k11.json
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-int4-preds-full.json \
  --candidate f5f7v7/f5f7v7-full.json \
  --pairs miniLM-l12-sts-full/pairs.json --iters 50000 \
  --out f5f7v7/bootstrap-f5f7v7-vs-int4-50k.json

# 13) M1.4：纯 int3 上逐块 3→4，跑 72 个全量变体并汇总
python scripts/wsp_budget_perturb.py \
  --base budget-l12-pure-int3.json --bits 4 \
  --out-dir perturb-full-3to4 \
  --base-result l12-int3-preds-full.json \
  --run --exe target/release/examples/bert6.exe \
  --model all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full \
  --f32-cache l12-f32-cache-full-preds.json \
  --threads 6 --jobs 4
# 产物 perturb-full-3to4/perturb-summary.json，72 行；45 双升 / 17 双降 / 10 混合

# 14) R12：2 张 f + 4 张 v 降 int2，12 张 384×384 升 int4，字节严格不变
python scripts/wsp_budget_variant.py \
  --base budget-l12-pure-int3.json \
  --up 1:k,10:v,8:o,6:o,11:v,9:o,7:o,0:o,5:k,4:v,6:q,0:v \
  --down 5:f,7:f,7:v,8:v,9:v,6:v \
  --require-exact --out r12/swap-r12.json

# 15) 跑 R12 全量并导出 preds
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=r12/swap-r12.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-full-preds.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full --activation-mode f32 \
  --out r12/r12-full.json

# 16) R12 的配对 bootstrap：对 F5F7V7 / R10 / int4（50k）
python scripts/wsp_budget_bootstrap.py \
  --baseline f5f7v7/f5f7v7-full.json \
  --candidate r12/r12-full.json \
  --pairs miniLM-l12-sts-full/pairs.json --iters 50000 \
  --out r12/bootstrap-r12-vs-f5f7v7-50k.json
python scripts/wsp_budget_bootstrap.py \
  --baseline r10/r10-full.json \
  --candidate r12/r12-full.json \
  --pairs miniLM-l12-sts-full/pairs.json --iters 50000 \
  --out r12/bootstrap-r12-vs-r10-50k.json
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-int4-preds-full.json \
  --candidate r12/r12-full.json \
  --pairs miniLM-l12-sts-full/pairs.json --iters 50000 \
  --out r12/bootstrap-r12-vs-int4-50k.json

# 17) 按来源拆解（只读 preds，不重跑模型）
#     sts-sources.json 从 mteb/stsbenchmark-sts test 的 dataset 字段导出，
#     与 miniLM-l12-sts-full/pairs.json 逐行对应
python scripts/wsp_budget_source_eval.py \
  --preds int3=l12-int3-preds-full.json \
  --preds K10K11=l12-budget-k10k11-preds-full.json \
  --preds F5F7V7=f5f7v7/f5f7v7-full.json \
  --preds R12=r12/r12-full.json \
  --preds int4=l12-int4-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --sources sts-sources.json --out r12-by-source.json
# 产物 r12-by-source.json：ALL + 6 个来源的 Pearson/Spearman

# 18) 外部 SICK-R 前 1,000 对导出（需要 torch/transformers/datasets 的 Python）
python scripts/wsp_export_sts_pairs.py \
  --dataset mteb/sickr-sts --split test --pairs 1000 \
  --out miniLM-l12-sickr-1000 \
  --model sentence-transformers/all-MiniLM-L12-v2 --local-only
# 自检只查 WSPACT1/WSPMSK1 header，不加载模型：
python scripts/wsp_export_sts_pairs.py --self-test

# 19) 外部评测：同一 f32 cache，逐预算跑 int3/K10K11/F5F7V7/R12/int4
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=<budget.json> \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-sickr-1000.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sickr-1000 --activation-mode f32 \
  --out sickr/<name>.json

# 20) 同法导出 STS12/STS13 各前 1,000 对（输出目录可换）
python scripts/wsp_export_sts_pairs.py \
  --dataset mteb/sts12-sts --split test --pairs 1000 \
  --out miniLM-l12-sts12-1000 \
  --model sentence-transformers/all-MiniLM-L12-v2 --local-only
python scripts/wsp_export_sts_pairs.py \
  --dataset mteb/sts13-sts --split test --pairs 1000 \
  --out miniLM-l12-sts13-1000 \
  --model sentence-transformers/all-MiniLM-L12-v2 --local-only

# 21) 外部配置：v6e / I10 / K10K11 / R10 / F5F7V7 / R12 各用自己的预算；
#     int4 不设 TRIT_BUDGET，只用 TRIT_WEIGHT_BITS=4。其余环境变量同上。
#     v6e  = budget-l12-int3-v6e.json
#     I10  = swap-11ito10i.json
#     K10K11 = k10k11/swap-3qto10k-4kto11k.json
#     R10  = r10/swap-r10.json
#     F5F7V7 = f5f7v7/swap-f5f7v7.json
#     R12  = r12/swap-r12.json

# 22) 外部配对 bootstrap（10k；baseline/candidate 换成对应 JSON）
python scripts/wsp_budget_bootstrap.py \
  --baseline int3.json --candidate i10.json \
  --pairs <dataset>/pairs.json --iters 10000 \
  --out bootstrap-i10-vs-int3.json
python scripts/wsp_budget_bootstrap.py \
  --baseline k10k11.json --candidate i10.json \
  --pairs <dataset>/pairs.json --iters 10000 \
  --out bootstrap-i10-vs-k10k11.json
```

`wsp_budget_variant.py` 默认把非 up/down 块重置为 int3，避免把规划器原来的
int2/int4 带进消融；要保留 base 的非 3-bit 块，加 `--keep-base`。
`wsp_budget_swap.py` 默认 `--require-exact`，只允许同形状交换；要接受字节变化
加 `--allow-bytes-change`。

## 8. 逐块 3→2 全量敏感度（M1.3）

做法：以纯 int3 预算为 base，除目标块外 71 块保持 int3，只把目标块改成
2-bit；每个变体单独跑 L12/STS 1,379 对，记录 Pearson/Spearman 与实际总字节。
72 行覆盖 12 层 × q/k/v/o/i/f 六类。`Δ字节` 是这一块降级省下的字节
（384×384 为 18,432，384×1536 为 73,728），不是组合预算。

基线是纯 int3 `0.818440283 / 0.813361562`，实际 `10,782,720 B`。每个变体用
`TRIT_BUDGET` + `TRIT_WEIGHT_BITS=3`、`TRIT_THREADS=24`，并命中全量 f32 缓存。
表按 `ΔSpearman` 从高到低排；`双升` 表示降级后两个指标都上升，`双降` 表示都下降，
`混合` 表示方向不一致。

### 8.1 读数怎么读

- 29 双升 / 38 双降 / 5 混合。
- 平均 `ΔSpearman` 按类：`f +0.000922`、`v +0.000021`、`o −0.000183`、
  `i −0.000286`、`q −0.000737`、`k −0.002715`。late key 最敏感。
- 层 5–9 平均为正，层 0–4 和层 11 平均为负；层 11 的 `k/v/o` 三块最不能碰。
- 单块收益/伤害不能相加；组合必须整包跑。`F5F7V7` 是第一个整包验证。
- 这张表测的是运行时 `bits==2` 的三值路径，不是 uniform 2-bit；两者字节相同，
  误差语义不同（见 §6）。

### 8.2 72 块全量表

| 块 | ΔPearson | ΔSpearman | Δ字节 | 判读 |
|---|---:|---:|---:|---|
| `7f` | +0.004574491 | +0.005461199 | −73,728 | 双升 |
| `7v` | +0.003205461 | +0.004741458 | −18,432 | 双升 |
| `8v` | +0.002435711 | +0.004320844 | −18,432 | 双升 |
| `5f` | +0.004547167 | +0.004127294 | −73,728 | 双升 |
| `7i` | +0.004815798 | +0.003849298 | −73,728 | 双升 |
| `9v` | +0.002138135 | +0.003340010 | −18,432 | 双升 |
| `8i` | +0.003174934 | +0.002895320 | −73,728 | 双升 |
| `8o` | +0.001371961 | +0.002764140 | −18,432 | 双升 |
| `7o` | +0.000968558 | +0.002742680 | −18,432 | 双升 |
| `6v` | +0.002640445 | +0.002640964 | −18,432 | 双升 |
| `9f` | +0.002129583 | +0.002422553 | −73,728 | 双升 |
| `6q` | +0.001966248 | +0.002132800 | −18,432 | 双升 |
| `6k` | +0.002361690 | +0.002061007 | −18,432 | 双升 |
| `11i` | +0.000844669 | +0.002006680 | −73,728 | 双升 |
| `0o` | +0.001472584 | +0.001843989 | −18,432 | 双升 |
| `10i` | +0.000535638 | +0.001834128 | −73,728 | 双升 |
| `10o` | +0.000385160 | +0.001663048 | −18,432 | 双升 |
| `11f` | +0.000654924 | +0.001618915 | −73,728 | 双升 |
| `10v` | +0.001576093 | +0.001589340 | −18,432 | 双升 |
| `6o` | +0.001721330 | +0.001557226 | −18,432 | 双升 |
| `5i` | +0.001395184 | +0.001523857 | −73,728 | 双升 |
| `9i` | −0.000221506 | +0.001463369 | −73,728 | 混合 |
| `8f` | −0.000220141 | +0.001348501 | −73,728 | 混合 |
| `9q` | +0.001380272 | +0.001208334 | −18,432 | 双升 |
| `9o` | +0.001062531 | +0.001008302 | −18,432 | 双升 |
| `6f` | +0.001405089 | +0.000916602 | −73,728 | 双升 |
| `5q` | +0.001146729 | +0.000863723 | −18,432 | 双升 |
| `6i` | +0.000978662 | +0.000829138 | −73,728 | 双升 |
| `8q` | −0.000006967 | +0.000786627 | −18,432 | 混合 |
| `3f` | +0.002615372 | +0.000520909 | −73,728 | 双升 |
| `10f` | +0.000189084 | +0.000168228 | −73,728 | 双升 |
| `1f` | +0.000784588 | +0.000079866 | −73,728 | 双升 |
| `9k` | −0.000120195 | −0.000195077 | −18,432 | 双降 |
| `3i` | +0.001347066 | −0.000249072 | −73,728 | 混合 |
| `2v` | −0.000114803 | −0.000254386 | −18,432 | 双降 |
| `0v` | −0.000103995 | −0.000418117 | −18,432 | 双降 |
| `4q` | −0.000196916 | −0.000530459 | −18,432 | 双降 |
| `7q` | −0.000785439 | −0.000819632 | −18,432 | 双降 |
| `2q` | −0.000613555 | −0.000862587 | −18,432 | 双降 |
| `4o` | −0.000676609 | −0.000879156 | −18,432 | 双降 |
| `4v` | −0.000045058 | −0.001125486 | −18,432 | 双降 |
| `5k` | −0.000872466 | −0.001144742 | −18,432 | 双降 |
| `2o` | −0.000698246 | −0.001154786 | −18,432 | 双降 |
| `5o` | −0.000804148 | −0.001203398 | −18,432 | 双降 |
| `5v` | −0.000760333 | −0.001328244 | −18,432 | 双降 |
| `3q` | −0.000459816 | −0.001356104 | −18,432 | 双降 |
| `8k` | −0.001987112 | −0.001378830 | −18,432 | 双降 |
| `3o` | −0.000888359 | −0.001439515 | −18,432 | 双降 |
| `7k` | −0.001436447 | −0.001454261 | −18,432 | 双降 |
| `4f` | −0.000030432 | −0.001613145 | −73,728 | 双降 |
| `1q` | −0.001184203 | −0.001627952 | −18,432 | 双降 |
| `0k` | −0.001017907 | −0.001661822 | −18,432 | 双降 |
| `10q` | −0.000633872 | −0.001699081 | −18,432 | 双降 |
| `1k` | +0.000176757 | −0.001828816 | −18,432 | 混合 |
| `4i` | −0.000559518 | −0.001883133 | −73,728 | 双降 |
| `0f` | −0.000103226 | −0.001954257 | −73,728 | 双降 |
| `3v` | −0.000894463 | −0.001963212 | −18,432 | 双降 |
| `2f` | −0.001008694 | −0.002037314 | −73,728 | 双降 |
| `1o` | −0.001644365 | −0.002430328 | −18,432 | 双降 |
| `1v` | −0.001458136 | −0.002489495 | −18,432 | 双降 |
| `0q` | −0.001968942 | −0.002693547 | −18,432 | 双降 |
| `1i` | −0.002647648 | −0.003223419 | −73,728 | 双降 |
| `4k` | −0.003328005 | −0.003991459 | −18,432 | 双降 |
| `3k` | −0.001088225 | −0.004095717 | −18,432 | 双降 |
| `11q` | −0.005002498 | −0.004245255 | −18,432 | 双降 |
| `2k` | −0.002008492 | −0.004282951 | −18,432 | 双降 |
| `10k` | −0.002351682 | −0.004830986 | −18,432 | 双降 |
| `2i` | −0.004079297 | −0.005646531 | −73,728 | 双降 |
| `11o` | −0.009017617 | −0.006670027 | −18,432 | 双降 |
| `0i` | −0.001951217 | −0.006834398 | −73,728 | 双降 |
| `11v` | −0.010828769 | −0.008801160 | −18,432 | 双降 |
| `11k` | −0.009527993 | −0.009776769 | −18,432 | 双降 |

表里的正 Δ 不是“降级有益”的普遍结论，而是“这一块在 int3 下本身就拖后腿，
换成三值路径反而更稳”。它只说明位置敏感度，不说明 uniform 2-bit 更好。

## 9. 逐块 3→4 全量敏感度与同字节重排（M1.4）

做法：以纯 int3 预算为 base，除目标块外 71 块保持 int3，只把目标块改成
4-bit；每个变体单独跑 L12/STS 1,379 对。`Δ字节` 是这一块升级多花的字节
（384×384 为 `+18,432`，384×1536 为 `+73,728`）。72 行覆盖全部线性矩阵；
这是“单块升级收益”，不是“多块组合可相加”。

### 9.1 读数

- 45 块双升、17 块双降、10 块混合。大多数块升 int4 有正收益，但收益极不均匀。
- 按每字节收益，384×384 最好的是 `10v +0.002110`、`8o +0.001236`、
  `1k +0.001106`、`6o +0.000982`、`11v +0.000948`；384×1536 的 `7i`
  虽然 ΔSpearman `+0.002763` 最高，但一个槽要花 `73,728 B`。
- late key 的升级收益很低：`11k` 只有 `+0.000207`，`10k +0.001186`；
  这与 3→2 表里“late key 最不能降”并不对称。**降级的伤害和升级的收益不是
  同一张曲线，不能拿 3→2 表直接推 3→4。**
- 也有升级后双降的块：`8v −0.000899`、`3v −0.001958`、`8f −0.000694`、
  `6f −0.000568`；不是所有 int4 都值得。
- 更多 int4 不等于更好：`R15`（15 int4）全量 `0.832161 / 0.835268`，
  两个指标都低于 `R12`（12 int4）的 `0.833671 / 0.835411`。位置比数量重要。

### 9.2 72 块全量表

| 块 | ΔPearson | ΔSpearman | Δ字节 | 判读 |
|---|---:|---:|---:|---|
| `7i` | +0.002266026 | +0.002762951 | +73728 | 双升 |
| `10v` | +0.001369996 | +0.002109918 | +18432 | 双升 |
| `3f` | +0.001399843 | +0.001763743 | +73728 | 双升 |
| `10i` | +0.001258262 | +0.001575767 | +73728 | 双升 |
| `3i` | +0.000333557 | +0.001571974 | +73728 | 双升 |
| `9i` | +0.001861910 | +0.001529417 | +73728 | 双升 |
| `4i` | +0.000250053 | +0.001498009 | +73728 | 双升 |
| `1f` | +0.001168667 | +0.001441692 | +73728 | 双升 |
| `5i` | +0.000930806 | +0.001364116 | +73728 | 双升 |
| `8o` | +0.000850177 | +0.001236008 | +18432 | 双升 |
| `10k` | +0.001383537 | +0.001185560 | +18432 | 双升 |
| `1k` | +0.000629536 | +0.001106374 | +18432 | 双升 |
| `10f` | +0.000828255 | +0.001065265 | +73728 | 双升 |
| `6o` | +0.000887540 | +0.000982101 | +18432 | 双升 |
| `6i` | +0.000721662 | +0.000979279 | +73728 | 双升 |
| `11v` | +0.000767455 | +0.000948235 | +18432 | 双升 |
| `7o` | +0.000710212 | +0.000904423 | +18432 | 双升 |
| `9o` | +0.000737924 | +0.000893198 | +18432 | 双升 |
| `2f` | +0.001064344 | +0.000848886 | +73728 | 双升 |
| `0o` | +0.000243511 | +0.000683593 | +18432 | 双升 |
| `1i` | +0.000595671 | +0.000675161 | +73728 | 双升 |
| `2i` | +0.000608802 | +0.000674499 | +73728 | 双升 |
| `4v` | -0.000198982 | +0.000649257 | +18432 | 混合 |
| `10o` | +0.000472162 | +0.000624367 | +18432 | 双升 |
| `5k` | +0.000416348 | +0.000611316 | +18432 | 双升 |
| `9v` | +0.000884153 | +0.000543365 | +18432 | 双升 |
| `2o` | +0.000148938 | +0.000503307 | +18432 | 双升 |
| `5q` | +0.000326919 | +0.000487780 | +18432 | 双升 |
| `0v` | +0.000420818 | +0.000483032 | +18432 | 双升 |
| `6q` | +0.000175696 | +0.000477671 | +18432 | 双升 |
| `9q` | +0.000075492 | +0.000440812 | +18432 | 双升 |
| `7f` | +0.000262378 | +0.000438328 | +73728 | 双升 |
| `5f` | +0.000207682 | +0.000431644 | +73728 | 双升 |
| `0i` | +0.000833634 | +0.000428785 | +73728 | 双升 |
| `6k` | +0.000209003 | +0.000421056 | +18432 | 双升 |
| `0q` | +0.000315600 | +0.000413605 | +18432 | 双升 |
| `11o` | -0.000013648 | +0.000410292 | +18432 | 混合 |
| `9k` | +0.000045266 | +0.000318978 | +18432 | 双升 |
| `1v` | +0.000288813 | +0.000311778 | +18432 | 双升 |
| `1q` | +0.000042220 | +0.000308401 | +18432 | 双升 |
| `6v` | +0.000915082 | +0.000283279 | +18432 | 双升 |
| `11k` | +0.000122658 | +0.000207417 | +18432 | 双升 |
| `0k` | +0.000020003 | +0.000191719 | +18432 | 双升 |
| `4f` | +0.000320271 | +0.000146782 | +73728 | 双升 |
| `2q` | -0.000155107 | +0.000137196 | +18432 | 混合 |
| `3k` | +0.000041942 | +0.000135356 | +18432 | 双升 |
| `4k` | +0.000063329 | +0.000104470 | +18432 | 双升 |
| `2v` | -0.000124234 | +0.000077133 | +18432 | 混合 |
| `3o` | -0.000266880 | +0.000070595 | +18432 | 混合 |
| `8k` | -0.000340464 | +0.000022525 | +18432 | 混合 |
| `11f` | -0.000055425 | +0.000011605 | +73728 | 混合 |
| `11i` | +0.000027979 | +0.000000215 | +73728 | 双升 |
| `11q` | -0.000230509 | -0.000067086 | +18432 | 双降 |
| `4o` | +0.000140320 | -0.000081139 | +18432 | 混合 |
| `0f` | +0.000721076 | -0.000119676 | +73728 | 混合 |
| `9f` | -0.000319976 | -0.000147023 | +73728 | 双降 |
| `4q` | -0.000231586 | -0.000173174 | +18432 | 双降 |
| `7q` | -0.000075384 | -0.000184870 | +18432 | 双降 |
| `8q` | -0.000365815 | -0.000203349 | +18432 | 双降 |
| `10q` | +0.000020406 | -0.000252877 | +18432 | 混合 |
| `7k` | -0.000486377 | -0.000278721 | +18432 | 双降 |
| `1o` | -0.000136538 | -0.000293216 | +18432 | 双降 |
| `2k` | -0.000430562 | -0.000332298 | +18432 | 双降 |
| `5o` | -0.000287492 | -0.000449780 | +18432 | 双降 |
| `8i` | -0.002366043 | -0.000461655 | +73728 | 双降 |
| `6f` | -0.000571528 | -0.000567978 | +73728 | 双降 |
| `7v` | -0.000257240 | -0.000576562 | +18432 | 双降 |
| `5v` | -0.000438151 | -0.000661476 | +18432 | 双降 |
| `8f` | -0.001011940 | -0.000694433 | +73728 | 双降 |
| `3q` | -0.000395815 | -0.000719736 | +18432 | 双降 |
| `8v` | -0.001852801 | -0.000899282 | +18432 | 双降 |
| `3v` | -0.001736820 | -0.001958035 | +18432 | 双降 |

### 9.3 从表到 R9 / R10 / R12 / R15

四组候选都用同一组“供体降 int2、接收块升 int4、其余 int3”的严格同字节结构；
实际总字节全部 `10,782,720 B`，`proxy_err_ratio` 全部
`0.9155208682173497`。差别只在供体和 int4 槽的位置/数量。

- `R9`：供体 F5/F7/V7；9 个 int4 为 `1k,10v,8o,6o,11v,9o,7o,0o,5k`；
  全量 `0.832876 / 0.831677`。
- `R10`：供体 F5/F7/V7/V8；在 R9 上多一个 `4v` int4；全量
  `0.833002 / 0.834377`。两个指标的点估计都高于 R9；未单独对 R9 做 bootstrap，
  不写成“已证优于 R9”。
- **`R12`**：供体 F5/F7/V7/V8/V9/V6；12 个 int4 为
  `1k,10v,8o,6o,11v,9o,7o,0o,5k,4v,6q,0v`；全量
  `0.833671 / 0.835411`。对 F5F7V7 的 Spearman 50k CI 过 0、Pearson 跨 0；
  对 R10 两个 50k CI 都跨 0。
- `R15`：供体 F5/F7/V7/V8/V9/V6/O8/O7/Q6；15 个 int4；全量
  `0.832161 / 0.835268`。它在两个指标上都低于 R12，用来证明“不是 int4 越多越好”。

**结论：`R12` 是 M1.4 的点估计最优同字节配置；`R15` 证明增加 int4 数量本身
不保证更好。`R12` 的 Spearman 点估计高于纯 int4，但 Pearson 仍低，且对 int4
的 50k CI 两个都跨 0；不能写“3-bit 全面追平/超过 int4”。**

### 9.4 按来源拆解（事后诊断）

`mteb/stsbenchmark-sts` test 的 1,379 对可按 `dataset` 字段拆成 6 个来源
（MSRpar/MSRvid 各 250、answer-answer 254、headlines 250、images 250、
track5.en-en 125）。用已保存的全量 preds 做事后诊断（**不参与 R12 选择**）：

| 来源 | n | R12 P/S | F5 P/S | int4 P/S | R12−F5 S | R12−int4 S |
|---|---:|---:|---:|---:|---:|---:|
| MSRpar | 250 | 0.584/0.560 | 0.576/0.550 | 0.594/0.563 | +0.009 | −0.004 |
| MSRvid | 250 | 0.903/0.925 | 0.904/0.925 | 0.928/0.942 | −0.000 | −0.017 |
| answer-answer | 254 | 0.743/0.739 | 0.742/0.737 | 0.744/0.741 | +0.002 | −0.002 |
| headlines | 250 | 0.823/0.838 | 0.818/0.833 | 0.835/0.855 | +0.006 | −0.017 |
| images | 250 | 0.917/0.906 | 0.915/0.901 | 0.918/0.907 | +0.004 | −0.001 |
| track5.en-en | 125 | 0.856/0.855 | 0.858/0.858 | 0.871/0.864 | −0.003 | −0.009 |

- `R12` 相对 `F5F7V7` 是 4 胜 1 平 1 负，提升不是每个来源都为正；
  track5.en-en 上还低 `−0.003`。
- `R12` 相对纯 int4 在 **6 个来源里全部持平或更低**：MSRvid/headlines 都低
  `−0.017`，track5 低 `−0.009`。总体的 Spearman 点估计反超更像是跨来源的
  排序/校准效应，而不是每个来源内部都更好；这是事后推断，需用 leave-one-source-out
  或第二数据集验证。
- 每个来源只有 125–254 对，单来源 Spearman 噪声很大；这张表只用于说明
  “总体增益不均匀”，不作为 R12 的入选依据。

### 9.5 留一来源敏感性（事后诊断）

只从评测里去掉一个来源、保留其余 5 个来源，用已保存的 preds 重算总体指标；
**不重新选块、不重训**。这回答“总体优势是否依赖某一个来源”：

| 留出来源 | n | R12 P/S | F5 P/S | int4 P/S | R12−F5 S | R12−int4 S |
|---|---:|---:|---:|---:|---:|---:|
| MSRpar | 1129 | 0.842/0.858 | 0.839/0.852 | 0.847/0.855 | +0.005 | +0.002 |
| MSRvid | 1129 | 0.810/0.807 | 0.806/0.799 | 0.807/0.798 | +0.008 | +0.009 |
| answer-answer | 1125 | 0.865/0.855 | 0.860/0.848 | 0.870/0.859 | +0.007 | −0.004 |
| headlines | 1129 | 0.837/0.835 | 0.837/0.832 | 0.847/0.837 | +0.004 | −0.002 |
| images | 1129 | 0.815/0.818 | 0.811/0.811 | 0.818/0.813 | +0.007 | +0.005 |
| track5.en-en | 1254 | 0.831/0.831 | 0.828/0.822 | 0.834/0.825 | +0.008 | +0.006 |

- `R12` 相对 `F5F7V7` 的 Spearman 优势在 6 次留一中都是正的
  （`+0.004` 到 `+0.008`）：这条优势比较稳。
- `R12` 相对纯 int4 的符号会翻转：去掉 MSRvid 后变成 `+0.009`，去掉 track5
  后 `+0.006`；但去掉 answer-answer/headlines 仍是 `−0.004/−0.002`。
  这说明总体 Spearman 反超依赖来源构成，不是稳定胜过 int4。
- 这不是严格留出：`R12` 仍是在全部 1,379 对上选的；它只做敏感性诊断。
  真正的留出验证仍需按 pair/source 切分后重新选块并只跑 test。
- 上面两张表由 `scripts/wsp_budget_source_eval.py` 生成，命令见 §7 第 17 条；
  输入是已保存的全量 preds，不重跑模型。

## 10. 外部三套数据（M1.5）

M1.4 的来源拆解说明 `R12` 的总体优势可能依赖 STS benchmark 的来源构成，所以补
三套外部数据：`mteb/sickr-sts`、`mteb/sts12-sts`、`mteb/sts13-sts` 的 test 前
1,000 对，仍用同一 L12 权重、seq 64、f32 基线。三套的 `f32_vs_reference` 都
对齐到 `rel_l2≈6e-7`。

| 数据集 | pairs | 唯一句 / 句槽 | f32 `rel_l2` |
|---|---:|---:|---:|
| SICK-R | 1,000 | 1,088 / 2,000 | 6.18e-7 |
| STS12 | 1,000 | 1,891 / 2,000 | 6.34e-7 |
| STS13 | 1,000 | 1,898 / 2,000 | 6.15e-7 |

三套都存在句子跨 pair 复用，**都不是 sentence-disjoint**；这里只做外部数据集
复验，不是严格留出。

### 10.1 绝对读数

| 配置 | SICK-R P | SICK-R S | STS12 P | STS12 S | STS13 P | STS13 S |
|---|---:|---:|---:|---:|---:|---:|
| 纯 int3 | 0.794947 | 0.713724 | 0.740258 | 0.704207 | 0.786142 | 0.802354 |
| v6e | 0.798030 | 0.716413 | 0.742603 | 0.706158 | 0.788400 | 0.804811 |
| **I10** | **0.799249** | **0.718066** | **0.745268** | **0.708730** | 0.788268 | 0.804260 |
| K10K11 | 0.800229 | 0.719099 | 0.744718 | 0.707304 | 0.785834 | 0.802063 |
| R10 | 0.788063 | 0.710254 | 0.753168 | 0.718903 | 0.807752 | 0.819653 |
| F5F7V7 | 0.790981 | 0.710513 | 0.742075 | 0.706376 | 0.801351 | 0.814645 |
| R12 | 0.789986 | 0.708856 | 0.747434 | 0.713242 | 0.807698 | 0.820215 |
| 纯 int4 | 0.819722 | 0.754058 | 0.759021 | 0.720020 | 0.809436 | 0.824798 |
| 纯 f32 | 0.815566 | 0.752769 | 0.765221 | 0.726309 | 0.816495 | 0.830425 |

逐行原始读数（24 行 = 3 套 × 8 配置）在
`docs/research/data/wsp-budget-external-3set.tsv`；10k bootstrap 的 54 行
（3 套 × 9 个对比 × 2 指标）在
`docs/research/data/wsp-budget-external-bootstrap.tsv`。两张表都是从结果 JSON
直接生成，表内 `delta_*_vs_int3` 与上面两张表同口径；`actual_bytes` 里
int3/同字节候选为 `10,782,720`、int4 为 `13,436,928`。

### 10.2 相对纯 int3 的方向

| 配置 | SICK-R ΔP / ΔS | STS12 ΔP / ΔS | STS13 ΔP / ΔS | 三套方向 |
|---|---:|---:|---:|---|
| v6e | +0.003083 / +0.002689 | +0.002345 / +0.001951 | +0.002259 / +0.002458 | 3/3 正 |
| **I10** | **+0.004302 / +0.004342** | **+0.005010 / +0.004523** | **+0.002127 / +0.001906** | **3/3 正** |
| K10K11 | +0.005282 / +0.005375 | +0.004460 / +0.003097 | −0.000308 / −0.000291 | 2 正 1 负 |
| R10 | −0.006884 / −0.003471 | +0.012910 / +0.014696 | +0.021610 / +0.017299 | 1 负 2 正 |
| F5F7V7 | −0.003966 / −0.003211 | +0.001817 / +0.002169 | +0.015209 / +0.012291 | 1 负 2 正 |
| R12 | −0.004961 / −0.004868 | +0.007176 / +0.009035 | +0.021556 / +0.017861 | 1 负 2 正 |
| 纯 int4 | +0.024775 / +0.040334 | +0.018763 / +0.015813 | +0.023294 / +0.022444 | 3/3 正 |

在已测的 6 个同字节配置里，只有 `v6e` / `I10` 三套外部点估计都为正；R9/R15
没有跑外部，不能把“只有”外推到全部候选。

### 10.3 10k 配对 bootstrap

| 对比 | 数据集 | ΔP | 95% CI | ΔS | 95% CI |
|---|---|---:|---:|---:|---:|
| I10 vs int3 | SICK-R | +0.004302 | [+0.000916, +0.007724] | +0.004342 | [+0.000433, +0.008253] |
| I10 vs int3 | STS12 | +0.005010 | [−0.000170, +0.010408] | +0.004523 | [−0.001873, +0.010982] |
| I10 vs int3 | STS13 | +0.002127 | [−0.001003, +0.005437] | +0.001906 | [−0.001377, +0.005265] |
| K10K11 vs int3 | SICK-R | +0.005282 | [+0.001728, +0.008880] | +0.005375 | [+0.001208, +0.009640] |
| K10K11 vs int3 | STS12 | +0.004460 | [−0.000944, +0.009917] | +0.003097 | [−0.003521, +0.009858] |
| K10K11 vs int3 | STS13 | −0.000308 | [−0.003794, +0.003253] | −0.000291 | [−0.003831, +0.003323] |
| R12 vs int3 | SICK-R | −0.004961 | [−0.009995, −0.000004] | −0.004868 | [−0.011309, +0.001526] |
| R12 vs int3 | STS12 | +0.007176 | [−0.001509, +0.016013] | +0.009035 | [−0.001650, +0.019852] |
| R12 vs int3 | STS13 | +0.021556 | [+0.015341, +0.027923] | +0.017861 | [+0.012148, +0.023809] |
| I10 vs K10K11 | SICK-R | −0.000980 | [−0.002281, +0.000294] | −0.001033 | [−0.002921, +0.000850] |
| I10 vs K10K11 | STS12 | +0.000550 | [−0.001672, +0.002809] | +0.001426 | [−0.001472, +0.004476] |
| I10 vs K10K11 | STS13 | +0.002435 | [+0.001008, +0.003898] | +0.002197 | [+0.000636, +0.003749] |

补充：

- `v6e` vs int3：SICK-R `+0.003083 / +0.002689`，Pearson CI
  `[+0.000568, +0.005649]` 过 0，Spearman CI 刚好跨 0；STS12/13 两个 CI 都跨 0。
- `R10` vs int3：SICK-R 双负（Pearson CI 过 0），STS12/13 双正且两个 CI 都过 0。
- `F5F7V7` vs int3：SICK-R 双负（Pearson CI 上界 `+0.000014` 刚好跨 0，
  Spearman 跨 0）；STS12 双正但 CI 跨 0；STS13 双正且 CI 过 0。
- `R12` vs K10K11：SICK-R 双负、STS13 双正，两个 CI 都过 0；STS12 双正但 CI 跨 0。
- `int4` vs int3：SICK-R/STS13 两个 CI 都过 0；STS12 Pearson CI 过 0、
  Spearman CI 仍跨 0。

### 10.4 结论与选择

- **没有单一候选同时赢 in-domain 和外部**。STS benchmark 全量仍是
  `R12 0.833671 / 0.835411 > K10K11 0.823479 / 0.820549 > I10 0.821621 / 0.818898`；
  但外部三套里 R12/R10/F5F7V7 都在 SICK-R 负迁移，K10K11 在 STS13 轻微负。
- **in-domain 的“优于 K10K11/F5F7V7”由 `R12` 给出**：全量对 K10K11
  `+0.010192 / +0.014862`，对 F5F7V7 `+0.003287 / +0.006733`（对 F5 的
  Spearman 50k CI 过 0，Pearson 跨 0）；但它在 SICK-R 双负，所以“优于”只
  成立于 STS benchmark 这批分布，不是跨分布结论。
- **`I10` 是外部保守默认**：三套外部点估计全正，SICK-R 双 10k CI 过 0；
  STS12/STS13 仍为正但 CI 跨 0。它对 K10K11 在 SICK-R 低 `−0.00098 / −0.00103`
  （CI 跨 0）、STS12 高 `+0.00055 / +0.00143`（CI 跨 0）、STS13 高
  `+0.00244 / +0.00220`（CI 过 0）——没有一套外部数据是 K10K11 显著优于 I10。
  代价是 STS 全量比 K10K11 低 `−0.00186 / −0.00165`，且 K10K11 对 I10 的
  50k CI 下界都过 0。
- **`K10K11` 是 in-domain 优先点**：STS 全量比 I10 高、SICK-R 也显著正；
  但 STS13 双指标轻微负，不能再说“K10K11 在所有外部数据上最稳”。
- **`R12` / `R10` / `F5F7V7` 是 STS 分布专用**：在 STS12/13 上很强
  （R10/R12 的 STS13 双 CI 过 0），但在 SICK-R 上负；R12 的 SICK-R Pearson
  CI 上界 `−0.000004` 刚好不过 0。不能当通用默认。
- **`v6e` 是更保守的基线**：三套外部也全正，但 2/3 的增益小于 I10；I10 在
  SICK-R/STS12 比 v6e 高，STS13 略低 `−0.00013 / −0.00055`，两者没有做
  bootstrap 对比，不能写“I10 已证优于 v6e”。
- **int4 仍赢外部**：三套外部 int4 都高于所有 3-bit 方案（SICK-R
  `+0.0248 / +0.0403`、STS13 `+0.0233 / +0.0224`；STS12 Spearman CI 跨 0）。
  3-bit 与 4-bit 的差距没有被外部数据关闭。

### 10.5 边界

- 三套都只取前 1,000 对，且 STS12/13/SICK-R 都有句子跨 pair 复用；
  SICK-R 1,088 唯一句、STS12 1,891、STS13 1,898。都不是 sentence-disjoint。
- 只有一个模型（L12）、一个 STS 家族、一个 seq 64 设置；没有检索/分类任务头。
- 三套外部数据的来源构成与 STS benchmark 不同，但仍是语义相似度任务；
  不能外推到检索、问答或生成。
- 外部结论只覆盖已测 6 个同字节配置；R9/R15 未跑，不能声称“全部候选”。

## 11. 第二模型 L6 复验（M1.6）

这一节回答 §10.5 留下的问题：L12 上的块表/候选能不能跨模型用。
结论先写：**不能直接搬。** 纯 int3 在 L6 上确实低于纯 int4，但 4-bit 优势
只在 Pearson 上过 10k CI；把 `I10`/`K10K11`/`R12` 的位宽布局按绝对层号搬到
L6，三个都低于 L6 纯 int3；按输出端对齐搬 `R12` 尾段，也只做到统计中性。
L6 自己重新扫表后能选出高于纯 int3、点估计高于 int4 的同字节配置，但那是
in-sample 搜索，不能当跨模型结论。

### 11.1 设置与字节账

- 模型 `all-MiniLM-L6-v2`：6 层、hidden 384、12 heads；全量 STS 1,379 对、
  seq 64；L6 自己的 `f32` 缓存对齐 `rel_l2=6.30e-7`、`cosine=1.0`。
- L6 f32 基线：Pearson `0.827406416`、Spearman `0.820324673`。
- L6 纯 int3（36 块）：Pearson `0.814593812`、Spearman `0.810424771`；
  相对 f32 `−0.012812605 / −0.009899902`；含 metadata 实际
  `5,391,360 B`（weight `3,981,312 B` + metadata `1,410,048 B`）。
- L6 纯 int4：Pearson `0.826060679`、Spearman `0.819353538`；相对 f32
  `−0.001345737 / −0.000971135`。按预算账（含 metadata）是 `6,718,464 B`；
  运行时 `weight_bytes=5,308,416 B` 只算权重、不含 metadata，两个数不能混用。
  纯 int3 是纯 int4 预算账的 80.2%。
- L6 的 3-bit 相对 f32 掉点比 L12 小：L12 纯 int3 是
  `−0.019155 / −0.017532`，L6 是 `−0.012813 / −0.009900`。但这是模型内
  Δ，L6/L12 的 f32 绝对值不能横向比较。

### 11.2 36 块双向敏感度

`3→2` 与 `3→4` 各 36 个单块变体，每个跑全量 1,379 对；全表见
`data/wsp-budget-l6-perturb.tsv`。L6 最不能降 int2 的 5 块：

| 块 | ΔPearson | ΔSpearman |
|---|---:|---:|
| 5 key | −0.004322 | −0.009418 |
| 1 key | −0.006376 | −0.009369 |
| 2 key | −0.007246 | −0.008284 |
| 5 output | −0.010595 | −0.008170 |
| 1 intermediate | −0.005254 | −0.006109 |

L6 升 int4 收益最大的 5 块：

| 块 | ΔPearson | ΔSpearman |
|---|---:|---:|
| 0 intermediate | +0.003072 | +0.003091 |
| 0 output | +0.003850 | +0.002304 |
| 2 output | +0.002786 | +0.002286 |
| 4 intermediate | +0.001997 | +0.001382 |
| 4 output | +0.002003 | +0.001281 |

和 L12 的表对照，**`3→2` 的伤害有输出端对齐的共性，`3→4` 的收益没有**：

| 对齐方式 | 3→2 Pearson / Spearman | 3→4 Pearson / Spearman |
|---|---:|---:|
| L12 前 6 层 vs L6 前 6 层 | 0.020 / 0.162 | 0.375 / 0.127 |
| L12 后 6 层 vs L6 前 6 层 | 0.514 / **0.597** | −0.158 / **−0.182** |

L6 最怕降 int2 的是末层 key/output 和早层 key/FFN；L12 最怕的是 11k/11v/
11o、0i/2i，按“距输出端的相对深度”对得上。升 int4 则相反：L6 最受益的是
0i/0f/2f/4i/4f，L12 是 7i/10v/3f/10i/3i/4i，层位置对不上。**所以块表不能
按绝对层号平移，也不能假设“升级表”跟着降级表一起搬。**

### 11.3 把 L12 候选搬到 L6

所有配置都严格同字节 `5,391,360 B`；`I10-abs`/`K10K11-abs`/`R12-abs` 是把
L12 的位宽块按**绝对层号**投影到 L6，落在 L6 层 0–5 之外的块不存在，再按
字节配平到 `1 int2 / 31 int3 / 4 int4`；`R12-tail` 是把 R12 后 6 层的 v/o
位宽按输出端对齐缩放到 L6，`3 int2 / 30 int3 / 3 int4`。逐项读数：

| 配置 | 分布 | 实际字节 | Pearson | Spearman | ΔP vs int3 | ΔS vs int3 |
|---|---|---:|---:|---:|---:|---:|
| L6 纯 int3 | 0/36/0 | 5,391,360 | 0.814594 | 0.810425 | — | — |
| L6 纯 int4 | 0/0/36 | 6,718,464 | 0.826061 | 0.819354 | +0.011467 | +0.008929 |
| I10-abs | 1/31/4 | 5,391,360 | 0.810442 | 0.809316 | −0.004152 | −0.001109 |
| K10K11-abs | 1/31/4 | 5,391,360 | 0.810173 | 0.809124 | −0.004421 | −0.001300 |
| R12-abs | 1/31/4 | 5,391,360 | 0.807656 | 0.807164 | −0.006938 | −0.003261 |
| R12-tail | 3/30/3 | 5,391,360 | 0.814494 | 0.809723 | −0.000100 | −0.000702 |
| L6-native | 5/26/5 | 5,391,360 | **0.828687** | **0.821084** | +0.014093 | +0.010660 |

**三个绝对层号投影都低于 L6 纯 int3**；输出端对齐的 `R12-tail` 是
`−0.000100 / −0.000702`，10k CI `[−0.004402, +0.004150]` /
`[−0.005731, +0.004212]`，统计上只是中性。也就是说，L12 上已经赢过 int3 的
候选，在 L6 上没有一个还能保住增益；`R12` 的位宽布局本身不是跨模型规则。

`L6-native` 是按 L6 自己的 36 块表选的：`3→2` 选 4v/3v/1o + 2i/2f，`3→4`
选 4k/1v/5q + 0i/0f（每个大小组内 int2/int4 配平，字节严格相等）。它相对
纯 int3 `+0.014093 / +0.010660`（10k CI `[+0.007392, +0.020620]` /
`[+0.003408, +0.017886]`，双指标过 0），点估计也高于纯 int4
`+0.002626 / +0.001731`；但对 int4 的 CI 是 `[−0.006531, +0.012206]` /
`[−0.008080, +0.011898]`，**都跨 0，不能说“3-bit 超过 int4”**。而且它和
`R12` 一样是在同一批 1,379 对上选块再评测，属于 in-sample 搜索。

### 11.4 两折 held-out：块选择信号有，但不够强

把 L6 的 1,379 对随机切成 A/B 两折（各约 690 对，seed `20261124`）。在 A 上
选 5 个 int2/5 个 int4 块、在 B 上评；再反过来。选择只用训练折的单块 Δ，
测试折不参与选块。结果：

| 训练/测试 | ΔP | 95% CI | ΔS | 95% CI |
|---|---:|---:|---:|---:|
| A→B | +0.010384 | [+0.001281, +0.019690] | +0.009694 | [−0.000930, +0.020739] |
| B→A | +0.004804 | [−0.004866, +0.014233] | +0.000707 | [−0.010593, +0.011956] |

两折 Pearson 都为正、A→B 的下界过 0；Spearman 两折都为正但 CI 都跨 0；
B→A 的 Spearman 只有 `+0.0007`，基本是零。**块级敏感度在 L6 内有可重复
信号，但按 36 块选 10 个块的搜索方差很大，690 对的测试折还不足以把它变成
统计结论。** 这也解释了为什么 L6-native 的 in-sample 数字不能直接外推。

### 11.5 结论与边界

- **第二模型没有验证 L12 的候选可迁移**：`I10`/`K10K11`/`R12` 三种布局搬到
  L6 都低于 L6 纯 int3；输出端对齐的 `R12` 尾段也只打平。`I10` 作为
  “外部保守默认”仍只在 L12 的三套外部数据上成立，不能写成跨模型默认。
- **L6 上 int4 相对 int3 的优势也比 L12 弱**：Pearson 10k CI 过 0，
  Spearman CI `[−0.000482, +0.018222]` 跨 0（候选更好 96.9%）。L6 的
  f32→int3 掉点本身也更小（`−0.0128 / −0.0099`），3-bit 的绝对损失没有
  L12 那么严重。
- **L6-native 只是“这个模型上重扫一次”的证据**：它说明逐块分配在 L6 也能
  把同字节配置抬到 int4 点估计之上，但不说明存在一个不需要重扫的通用 JSON。
  要跨模型，需要的是**与模型无关的选择规则**（例如按相对深度+块类型+代理
  误差的规则），或者每个模型重跑 36/72 块表，而不是搬 L12 的位宽。M1.9 已
  试过“相对深度 + 块类型 + ΔSpearman”的单向规则，L12→L6 可用、L6→L12 对
  int4 明显为负（§11.9）；**这条规则也不是对称的跨模型默认。**
- **固定结构的选择方差真实存在，但不是“完全过拟合”**：两折 held-out 对
  int3 的 CI 全跨 0、对 int4 两折转负；§11.7 的 5 折 × 2 种子交叉拟合把
  procedure 均值估出来：OOF 均值 `0.825731 / 0.819179`，相对 int3
  `+0.011137 / +0.008754`（10 折中 P 9/10、S 8/10 为正），相对 int4
  `−0.000330 / −0.000175`（10 折中只有 5/10 为正）。**选择器 procedure
  稳定优于纯 int3，但没有稳定超过纯 int4；仍不能当默认规则。**
- **边界**：只有一个第二模型（L6）、一个 STS 全量集、一个 seq 64；两折不是
  sentence-disjoint（L6 的句子跨 pair 复用未单独统计）；L6 与 L12 的 f32
  绝对值不可横向比较；`L6-native` 和 `R12` 都是 in-sample 选块。
- **运行时默认不动**：没有把 `I10` 或任何 L6 候选写进默认配置。文档层仍是
  `I10` 外部保守、`K10K11`/`R12` STS in-domain 选择；跨模型默认待定。

逐配置读数、36×2 行块表和 bootstrap 见
`data/wsp-budget-l6-candidates.tsv`、`data/wsp-budget-l6-perturb.tsv`、
`data/wsp-budget-l6-bootstrap.tsv`。

### 11.6 重扫选择器与 held-out 反证

新增 `scripts/wsp_budget_rescan.py`：读 `3→2`/`3→4` 两张表，按
`(rows, cols, group)` 分形状组；同一形状组内 3→2 省下的字节与 3→4 花掉的
字节相等，所以每组各选同样数量的降/升块，整包严格同字节。降级取
`delta_spearman` 最大（伤害最小），升级也取最大（收益最大），并避开已选
降级块；默认每个形状组 2 降 + 2 升。它不重新跑扫描，只把已有块表变成
可复算的候选，避免手写 `--up/--down` 列表。`--self-test` 用合成块表守住
配平、排序和“同一块不能既 up 又 down”。

L6 上按默认 `per_shape=2` 选出 `6 int2 / 24 int3 / 6 int4`（`0i/0f/1v/4k/
4i/4f` 升 int4，`2i/2f/3v/3i/3f/4v` 降 int2），字节仍 `5,391,360 B`：

| 配置 | 分布 | Pearson | Spearman | ΔP vs int3 | ΔS vs int3 |
|---|---|---:|---:|---:|---:|
| L6 纯 int3 | 0/36/0 | 0.814594 | 0.810425 | — | — |
| L6 纯 int4 | 0/0/36 | 0.826061 | 0.819354 | +0.011467 | +0.008929 |
| **rescan-k2** | **6/24/6** | **0.831304** | **0.824457** | **+0.016710** | **+0.014032** |

rescan-k2 相对纯 int3 的 10k CI 是 `[+0.009081, +0.024205]` /
`[+0.005365, +0.022586]`，双指标过 0；相对纯 int4 是
`+0.005244 / +0.005103`，但 CI `[−0.004134, +0.015053]` /
`[−0.005315, +0.016090]` 都跨 0。**全量点估计高于 int4，仍不能写“超过”。**

更关键的是两折 held-out。用同样的 `per_shape=2` 结构在训练折上选块，
测试折不参与选块：

| 训练/测试 | ΔP vs int3 | 95% CI | ΔS vs int3 | 95% CI | ΔP vs int4 | ΔS vs int4 |
|---|---:|---:|---:|---:|---:|---:|
| A→B | +0.007193 | [−0.005616, +0.019827] | +0.002260 | [−0.011473, +0.015970] | −0.002826 | −0.007214 |
| B→A | +0.007985 | [−0.001130, +0.016948] | +0.006351 | [−0.004302, +0.016880] | −0.004958 | −0.002192 |

两折相对 int3 的点估计都为正，但 CI 全跨 0；相对 int4 的点估计两折都转负。
**固定“每形状组 2 降 2 升”只减少了手工自由度，没有消除块选择的搜索方差。
全量 `0.831304 / 0.824457` 是 in-sample 选块的结果，held-out 上并不优于
int4。** 这个选择器可以继续用作“从表到候选”的复现工具（见
`scripts/wsp_budget_rescan.py --self-test`），但不能当已验证的
默认规则；5 折交叉拟合（§11.7）已经把选择本身留出，结论是稳定优于 int3、
只打平 int4。若还要选 `per_shape` 本身，才需要 nested CV。

### 11.7 5 折 × 2 种子交叉拟合

新增 `scripts/wsp_budget_cv.py`：每个 repeat 把 pair 随机切成 5 折，每折只用
其余 4 折的单块 ΔSpearman 排序选块（`per_shape=2` 不变），候选只在测试折上
计分；5 个测试折的预测拼成一条 OOF 向量，再和纯 int3/int4 做 10k 配对
bootstrap。它不引入新超参，只把“选择”本身放进交叉拟合。

| repeat | OOF Pearson | OOF Spearman | ΔP vs int3（95% CI） | ΔS vs int3（95% CI） | ΔP vs int4 | ΔS vs int4 |
|---|---:|---:|---:|---:|---:|---:|
| s20261124 | 0.827074 | 0.821085 | +0.012481 [+0.004413, +0.020486] | +0.010660 [+0.001475, +0.019960] | +0.001014 | +0.001732 |
| s20261125 | 0.824388 | 0.817272 | +0.009794 [+0.002098, +0.017471] | +0.006847 [−0.001636, +0.015561] | −0.001673 | −0.002081 |
| **均值** | **0.825731** | **0.819179** | **+0.011137** | **+0.008754** | **−0.000330** | **−0.000175** |

10 个测试折的折级结果与 OOF 拼接一致：对 int3 的 ΔP 折级均值
`+0.011044`、范围 `[−0.000307, +0.025213]`，9/10 折为正；ΔS 均值
`+0.008464`、范围 `[−0.012480, +0.028375]`，8/10 折为正。对 int4 的 ΔP
折级均值 `−0.000548`、范围 `[−0.016462, +0.018940]`，只有 5/10 折为正；
ΔS 均值 `−0.000316`、范围 `[−0.026806, +0.021527]`，也是 5/10。OOF 与
折级均值差小于 `0.0005`，说明这个结论不是把 5 个模型拼成一条向量造成的。

**解释**：两折是 50% 训练、每折 690 对测试，选择方差压过了均值；5 折是
80% 训练、每折约 276 对测试，procedure 的均值才显出来。选择器稳定优于
纯 int3，但相对纯 int4 仍是掷硬币（两折点估计一正一负，10 折 5/10 正，
均值略负），不能写成“3-bit 超过 int4”。全量 `0.831304` 仍是 in-sample
选块点估计，高于 OOF 均值 `0.825731`，不能拿它当泛化读数。

### 11.8 M1.7/M1.8 复算

```bash
# 1) L6 纯 int3 预算：预算等于 L6 int3 的含 metadata 实际字节
python scripts/wsp_budget.py \
  --model /path/all-MiniLM-L6-v2.safetensors --layers 6 \
  --budget 5391360 --out budget-l6-pure-int3.json

# 2) 36 块 3→2 / 3→4 全量（每个变体 1,379 对）
python scripts/wsp_budget_perturb.py \
  --base budget-l6-pure-int3.json --bits 2 --out-dir perturb-l6-3to2 \
  --run --resume --layers 6 --threads 6 --jobs 4 \
  --exe target/release/examples/bert6.exe \
  --model all-MiniLM-L6-v2.safetensors --data-dir miniLM-bert6-sts-full \
  --f32-cache l6-f32-cache-full.json --base-result l6-int3-result.json

python scripts/wsp_budget_perturb.py \
  --base budget-l6-pure-int3.json --bits 4 --out-dir perturb-l6-3to4 \
  --run --resume --layers 6 --threads 6 --jobs 4 \
  --exe target/release/examples/bert6.exe \
  --model all-MiniLM-L6-v2.safetensors --data-dir miniLM-bert6-sts-full \
  --f32-cache l6-f32-cache-full.json --base-result l6-int3-result.json

# 3) 纯 int4：不设 TRIT_BUDGET，只把全局位宽设为 4
TRIT_LAYERS=6 TRIT_WEIGHT_BITS=4 TRIT_THREADS=24 TRIT_SKIP_DIAG=1 \
TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 TRIT_F32_CACHE=l6-f32-cache-full.json \
bert6 --file all-MiniLM-L6-v2.safetensors \
  --data-dir miniLM-bert6-sts-full --activation-mode f32 --out l6-int4.json

# 4) L6-native：4v/3v/1o + 2i/2f 降 int2；4k/1v/5q + 0i/0f 升 int4
python scripts/wsp_budget_variant.py \
  --base budget-l6-pure-int3.json \
  --up 4:k,1:v,5:q,0:i,0:f --down 4:v,3:v,1:o,2:i,2:f \
  --require-exact --out budget-l6-native.json

# 5) 从两张表选严格同字节候选（默认每形状组 2 降 + 2 升）
python scripts/wsp_budget_rescan.py \
  --base budget-l6-pure-int3.json \
  --summary-3to2 perturb-l6-3to2/perturb-summary.json \
  --summary-3to4 perturb-l6-3to4/perturb-summary.json \
  --per-shape 2 --out budget-l6-rescan.json \
  --report budget-l6-rescan.report.json

# 5b) 选择器自带评测路径的端到端复算（可选）
python scripts/wsp_budget_rescan.py \
  --base budget-l6-pure-int3.json \
  --summary-3to2 perturb-l6-3to2/perturb-summary.json \
  --summary-3to4 perturb-l6-3to4/perturb-summary.json \
  --per-shape 2 --out budget-l6-rescan-runcheck.json \
  --report budget-l6-rescan-runcheck.report.json \
  --run --exe target/release/examples/bert6.exe \
  --model all-MiniLM-L6-v2.safetensors --data-dir miniLM-bert6-sts-full \
  --layers 6 --threads 24 --f32-cache l6-f32-cache-full.json \
  --result l6-rescan-k2-runcheck-result.json

# 6) 配对 bootstrap
python scripts/wsp_budget_bootstrap.py \
  --baseline l6-int3.json --candidate l6-native-result.json \
  --pairs miniLM-bert6-sts-full/pairs.json \
  --out bootstrap-l6-native-vs-int3.json

# 7) 5 折 × 2 种子交叉拟合：先只选块，再复用已跑结果评测试折
python scripts/wsp_budget_cv.py \
  --base budget-l6-pure-int3.json \
  --perturb-3to2 perturb-l6-3to2 --perturb-3to4 perturb-l6-3to4 \
  --pairs miniLM-bert6-sts-full/pairs.json \
  --int3 int3.json --int4 int4.json \
  --folds 5 --repeats 2 --per-shape 2 --out-dir cv5 --select-only

python scripts/wsp_budget_cv.py \
  --base budget-l6-pure-int3.json \
  --perturb-3to2 perturb-l6-3to2 --perturb-3to4 perturb-l6-3to4 \
  --pairs miniLM-bert6-sts-full/pairs.json \
  --int3 int3.json --int4 int4.json \
  --folds 5 --repeats 2 --per-shape 2 --out-dir cv5 --resume --run \
  --exe target/release/examples/bert6.exe \
  --model all-MiniLM-L6-v2.safetensors --data-dir miniLM-bert6-sts-full \
  --layers 6 --threads 24 --f32-cache l6-f32-cache-full.json
```

`wsp_budget_rescan.py --self-test` 输出 `self-test: ok`；`wsp_budget_cv.py --self-test`
校验折划分覆盖和 OOF 拼接；`--run` 路径已在 L6 上做过端到端
复算，候选预算和 report 与手工选择逐字段一致，评测输出
`0.8313043073454619 / 0.8244569451453719`，与 `l6-rescan-k2-result.json`
相同。

两折 held-out 的切分固定 seed `20261124`、按 pair 随机打乱后前/后各半；选块只在
训练折的单块 ΔSpearman 上排序，降级取最大（最不伤）、升级取最大（最受益），
每个大小组内 int2/int4 数量配平以保证字节相等。该切分脚本是临时分析脚本，
没有新增仓库入口；分折读数和 CI 已落在 `data/wsp-budget-l6-bootstrap.tsv`。

### 11.9 跨模型相对深度规则的双向检验（M1.9）

前两节都在目标模型自己的块表上选块；M1.9 反过来试“完全不看目标模型块表”的
规则。新增 `scripts/wsp_budget_rule.py`：对源表的每个 `kind`，把各层
`delta_spearman` 看成相对深度 `layer/(L_src-1)` 的折线；目标块按
`layer/(L_tgt-1)` 线性插值，端点外 clamp。降级取插值 ΔS 最大（伤害最小），
升级也取最大（收益最大），再复用 `wsp_budget_rescan.select_blocks` 按
`(rows, cols, group)` 配平、避开 up/down 重叠，`per_shape=2`，整包严格同字节。
**选块时只读源表和目标 base budget，目标模型的 36/72 块表完全不参与。**
`--table` 同时接受 L12 的宽表和 L6 的长表（后者按 `(layer, kind)` 折叠），
所以两个方向都能直接用仓库里的表复算；`--self-test` 守插值、端点 clamp 和
字节配平。

#### 11.9.1 L12 → L6

源表是 `data/wsp-budget-l12-perturb.tsv`（72 行，从 §8.2/§9.2 的文档表格
解析，不是重跑 L12）；目标 base 是 L6 的纯 int3。规则选出：

- 升 int4：`1f / 2i / 2f / 4o / 4i / 5v`
- 降 int2：`3v / 3i / 3f / 4v / 4f / 5i`
- 分布 `6/24/6`，字节 `5,391,360 B`，与 L6 纯 int3 严格同字节。

| 配置 | 分布 | Pearson | Spearman | ΔP vs int3 | ΔS vs int3 |
|---|---|---:|---:|---:|---:|
| L6 纯 int3 | 0/36/0 | 0.814594 | 0.810425 | — | — |
| L6 纯 int4 | 0/0/36 | 0.826061 | 0.819354 | +0.011467 | +0.008929 |
| L6-native | 5/26/5 | 0.828687 | 0.821084 | +0.014093 | +0.010660 |
| rescan-k2 | 6/24/6 | 0.831304 | 0.824457 | +0.016710 | +0.014032 |
| **rule-L12-prior** | **6/24/6** | **0.828707** | **0.824484** | **+0.014113** | **+0.014060** |

规则候选相对纯 int3 的 10k CI 是 `[+0.006967, +0.021106]`（Pearson，更好
100.00%）和 `[+0.006284, +0.021773]`（Spearman，99.96%），双指标过 0。相对
纯 int4 是 `+0.002646 / +0.005131`，但 CI `[−0.006347, +0.012006]` /
`[−0.004494, +0.015281]` 都跨 0（更好 71.52% / 84.87%）。相对 L6-native 的
ΔP `+0.000020`、ΔS `+0.003400`，CI 都跨 0；相对 rescan-k2 的 ΔP
`−0.002598`、ΔS `+0.000027`，也都在噪声内。**它没有读 L6 表，却把 L6 抬到
和重扫候选同一档；这是“候选生成器”的证据，不是“规则能稳定超过 int4”的
证据（对 int4 的 10k CI 仍跨 0）。**

#### 11.9.2 L6 → L12

把方向反过来：源表是 `data/wsp-budget-l6-perturb.tsv`（36 块 × 2 方向的长表），
目标 base 是 L12 的纯 int3（72 块）。为做这一半，重新导出了 L12 全量
STS-B 1,379 对，并复算 L12 的 f32/int3/int4：

- L12 f32 `0.837595 / 0.830894`；纯 int3 `0.818440 / 0.813362`；
  纯 int4 `0.837505 / 0.832047`。int3/int4 与 §4 的旧读数一致。
- 规则选出升 int4：`0i / 0f / 1i / 2v / 4f / 9k`；降 int2：
  `5i / 5f / 6i / 6f / 7v / 8v`；分布 `6/60/6`，字节 `10,782,720 B`。
- 规则候选 `0.827813 / 0.823365`，相对纯 int3 `+0.009373 / +0.010004`，
  10k CI `[+0.002681, +0.016081]` / `[+0.002001, +0.018079]`，双指标过 0；
  但相对纯 int4 是 `−0.009692 / −0.008682`，Pearson CI
  `[−0.018357, −0.000968]` **整段为负**，Spearman CI
  `[−0.019226, +0.001648]` 跨 0（更好 1.40% / 5.22%）。
- 与 L12 既有配置比：`0.827813 / 0.823365` 高于 I10
  `0.821621 / 0.818898`、K10K11 `0.823479 / 0.820549`，但低于 F5F7V7
  `0.830384 / 0.828678`、R9 `0.832876 / 0.831677`、R10
  `0.833002 / 0.834377`、R12 `0.833671 / 0.835411`、R15
  `0.832161 / 0.835268`，也低于纯 int4。

#### 11.9.3 结论与边界

- **同一条“相对深度 + 块类型 + ΔS”规则，两个方向不对称**：L12 表给 L6 选
  出的候选和 L6 自己的重扫候选同档；L6 表给 L12 选出的候选却明显低于 int4
  （Pearson CI 整段为负）。所以“从任意源模型学一条规则就能投到目标模型”不
  成立，至少不能只凭 L12→L6 的单向结果写。
- **两个方向都稳定优于纯 int3，都没有统计上超过纯 int4**：L12→L6 是点估计
  为正、CI 跨 0；L6→L12 是点估计为负、Pearson CI 为负。**规则只能当候选
  生成器/消融工具，不能当运行时默认。**
- **边界**：只有 L6/L12 两个模型、一个 STS-B 全量集；L12 的 72 块表仍是
  §8.2/§9.2 的文档表，没有重跑 72 个变体；本轮只重跑了 L12 的 base/int4/rule
  三个整包。没有第三模型，没有 sentence-disjoint 切分，也没有把规则候选跑
  三套外部数据。10k CI 是 pair-level bootstrap，不是句子级。
- **运行时默认不动**：仍是 `I10` 外部保守、`K10K11`/`R12` STS in-domain；
  没有把 `rule-L12-prior` 或 `rule-L6-prior` 写进默认配置。下一步若继续这条
  线，先找第三模型；否则应停止调规则，转任务头/检索，而不是再改
  `per_shape`。

#### 11.9.4 复算

```bash
# L12 -> L6：源表是 L12 宽表，目标完全不用 L6 块表
python scripts/wsp_budget_rule.py \
  --base budget-l6-pure-int3.json \
  --table docs/research/data/wsp-budget-l12-perturb.tsv \
  --target-layers 6 --per-shape 2 \
  --out budget-l6-rule.json --report budget-l6-rule.report.json

# L6 -> L12：L6 长表会被按 (layer, kind) 折叠，不需要另存宽表
python scripts/wsp_budget_rule.py \
  --base budget-l12-pure-int3.json \
  --table docs/research/data/wsp-budget-l6-perturb.tsv \
  --target-layers 12 --per-shape 2 \
  --out budget-l12-rule.json --report budget-l12-rule.report.json

# L12 纯 int3 base 必须把 72 块都设成 bits=3；
# 直接 --budget 10782720 会选到 3/60/9 的同字节混合，不是纯 int3。
# 评测（复用已生成的 l12-f32-cache-full.json）
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l12-rule.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-full.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full --activation-mode f32 \
  --out rule-l12-result.json
```

`python scripts/wsp_budget_rule.py --self-test` 输出 `self-test: ok`。表见
`data/wsp-budget-rule-candidates.tsv`（6 行，含 M1.10 的四个新方向）和
`data/wsp-budget-rule-bootstrap.tsv`（12 行）；L6 的对比行追加在
`data/wsp-budget-l6-candidates.tsv`。

### 11.10 第三模型与同深度交叉检验（M1.10）

§11.9.3 的边界只有一句话最重要：**只有 L12/L6 一对模型，L12→L6 的正向
不能排除是这对模型的偶然。** 本轮把同一条规则投到两个新模型：

- **L3**：`sentence-transformers/paraphrase-MiniLM-L3-v2`，3 层、384 hidden、
  12 heads、intermediate 1536；与 L12/L6 深度不同，是“规则能否跨深度”的边界；
- **par-L6**：`sentence-transformers/paraphrase-MiniLM-L6-v2`，6 层、384 hidden、
  12 heads、intermediate 1536；与 all-MiniLM-L6 **同深度、同架构、不同训练目标**，
  是“L12→L6 的正向是不是模型对特定”的直接对照。

规则仍然是 `scripts/wsp_budget_rule.py`：只读源表和目标 base budget，目标模型的
块表完全不参与；`per_shape=2`，整包严格同字节。

#### 11.10.1 3 层模型的可行性边界与选择器收缩

L3 的 18 块分成三个形状组：`384×384` 12 块、`384×1536` 3 块、`1536×384` 3 块。
“每形状组 2 降 2 升”要求每组至少 4 块，后两组只有 3 块，物理上不可行。
本轮把 `wsp_budget_rescan.select_blocks` 的口径改成：

- `per_shape` 是**每组上限**，不是硬性配额；
- 每组实际对数 `effective = min(per_shape, floor(n/2))`；
- 每个形状组的 `requested_per_shape/effective_per_shape` 写进报告，
  预算 `variant.per_shape_effective` 也保留实际值。

对 L3，实际是 `2 / 1 / 1` 对，共 4 降 4 升，分布 `4/10/4`。对 L6/L12，
三个形状组都 ≥6 块，`effective` 仍为 2；把 L12→L6 的规则候选重跑一遍，
`6/24/6`、`5,391,360 B`、上下选块与 §11.9.1 完全一致，旧结果不受影响。

#### 11.10.2 L3 基线：规则候选两个方向都显著低于纯 int3

L3 的 STS-B 全量 1,379 对、seq 64，f32/int3/int4 基线：

| 配置 | 分布 | Pearson | Spearman | ΔP vs int3 | ΔS vs int3 |
|---|---|---:|---:|---:|---:|
| L3 纯 f32 | — | 0.823637 | 0.824059 | +0.003946 | +0.001281 |
| L3 纯 int3 | 0/18/0 | 0.819691 | 0.822778 | — | — |
| L3 纯 int4 | 0/0/18 | 0.823378 | 0.825063 | +0.003687 | +0.002285 |
| **rule-L12-prior** | **4/10/4** | **0.807241** | **0.812000** | **−0.012450** | **−0.010778** |
| **rule-L6-prior** | **4/10/4** | **0.808890** | **0.809610** | **−0.010801** | **−0.013168** |
| rescan-k1（原生） | 3/12/3 | 0.819736 | 0.825566 | +0.000045 | +0.002787 |
| rescan-k2（原生） | 4/10/4 | 0.814996 | 0.823230 | −0.004695 | +0.000452 |

两条规则候选相对纯 int3 的 10k bootstrap：

- `rule-L12-prior`：Pearson `−0.012454`，CI `[−0.017612, −0.007431]`，
  候选更好 `0.00%`；Spearman `−0.010809`，CI `[−0.017126, −0.004982]`，
  更好 `0.03%`。
- `rule-L6-prior`：Pearson `−0.010838`，CI `[−0.016771, −0.005033]`，
  更好 `0.01%`；Spearman `−0.013212`，CI `[−0.020570, −0.006132]`，
  更好 `0.01%`。

对纯 int4 也是双负：L12 先验 `−0.016160 / −0.013096`，L6 先验
`−0.014544 / −0.015499`，四个 CI 都整段为负。**这不是“没赢 int4”，
是“连纯 int3 都显著更差”。**

失败原因可以落到块上。L3 自己的 36 行单块表（`data/wsp-budget-l3-perturb.tsv`）
显示：

- 降 int2 最伤的是 `1i −0.006620 / −0.007579`、`0v −0.005989 / −0.007134`、
  `0o −0.006315 / −0.006519`、`0k −0.005865 / −0.006009`；最耐降的是
  `1k −0.001718 / +0.000430`、`1o −0.003511 / +0.000408`、`1q −0.001518 / +0.000342`。
- 升 int4 最受益的是 `1f +0.000601 / +0.000941`、`1q +0.000549 / +0.000880`、
  `1o +0.000313 / +0.000850`、`0f +0.001047 / +0.000731`；`2q −0.000119 / −0.000227`、
  `2v −0.000308 / −0.000152` 是负收益。
- 两个规则方向**都选了 `0o` 降 int2**；L3 上它是 layer 0 最伤的降级块之一。
  L6 先验还选了 `1i`、`1f`，正好是 L3 最伤降级块的第 1、第 8 名；
  L12 先验的升级里 `1i`、`2v`、`2o` 也是负收益。**源模型上的“最安全/最受益”，
  到 L3 变成了“最伤/负收益”，排序整体反转。**

这不是“规则插值算错了”，而是**块级敏感度的排序本身不跨模型守恒**：
同一块在 L3 上甚至两个方向都小幅为正（`1k` 降 `+0.000430`、升 `+0.000362`；
`1o` 降 `+0.000408`、升 `+0.000850`），源表的 `kind` 均值无法预测这种符号。

#### 11.10.3 L3 原生重扫对照：3 对只能打平 int3，4 对 Pearson 翻负

为了区分“规则选错块”和“L3 本身没有同字节空间”，本轮对 L3 的 18 块 × 2
方向全部跑了全量单块评测（36 个变体，`perturb-l3-3to2/3to4`），再用
`wsp_budget_rescan.py` 在目标自己的表上选块：

- `rescan-k1`：每形状组 1 对，`3/12/3`，`2,695,680 B`；
  选块降 `1k / 2i / 2f`，升 `0i / 1q / 1f`；
  全量 `0.819736 / 0.825566`。相对纯 int3 `+0.000045 / +0.002787`，
  10k CI Pearson `[−0.003241, +0.003351]`（更好 `51.92%`）、
  Spearman `[−0.000918, +0.006489]`（更好 `92.98%`）；相对纯 int4
  `−0.003641 / +0.000503`，CI `[−0.007748, +0.000548]` / `[−0.004002, +0.005122]`
  都跨 0。**3 对原生混合只能和 int3 打平，Pearson 点估计几乎为 0。**
- `rescan-k2`：`per_shape=2` 自动收缩成 `4/10/4`；
  选块降 `1o / 1k / 2i / 2f`，升 `0o / 0i / 1q / 1f`（与 k1 同一批块，
  只是 attention 组多 1 对）；全量 `0.814996 / 0.823230`。相对纯 int3
  `−0.004695 / +0.000452`，CI Pearson `[−0.009505, +0.000189]`（更好 `2.93%`）、
  Spearman `[−0.005153, +0.005989]`（更好 `56.80%`）；相对纯 int4
  `−0.008382 / −0.001833`，Pearson CI `[−0.013627, −0.002990]` 整段为负。
  **4 对原生混合在 L3 上把 Pearson 推负，Spearman 只打平。**

所以 L3 的可用空间很窄：3 对能吸收，4 对就翻负。但**规则候选比原生 k2 还差**：
`rule-L12` 是 `−0.007755 / −0.011230`，`rule-L6` 是 `−0.006106 / −0.013621`。
这说明规则不是“输给了 L3 的容量边界”，而是把最伤的块当成最安全的块。
即使 L3 完全没有 headroom，也不能替规则候选解释掉这 0.6–0.8 个百分点（P）
/ 1.0–1.4 个百分点（S）的额外损失。

#### 11.10.4 同深度 paraphrase-L6：同一布局的符号翻转

par-L6 与 all-MiniLM-L6 同为 6 层、384 hidden、12 heads、intermediate 1536，
但训练目标不同。它的 STS-B 全量基线是：

| 配置 | 分布 | Pearson | Spearman | ΔP vs int3 | ΔS vs int3 |
|---|---|---:|---:|---:|---:|
| par-L6 纯 f32 | — | 0.836131 | 0.841234 | +0.011089 | +0.014458 |
| par-L6 纯 int3 | 0/36/0 | 0.825042 | 0.826777 | — | — |
| par-L6 纯 int4 | 0/0/36 | 0.830598 | 0.837303 | +0.005556 | +0.010527 |
| **rule-L12-prior** | **6/24/6** | **0.808881** | **0.819468** | **−0.016161** | **−0.007308** |
| **rule-L6-prior** | **6/24/6** | **0.813839** | **0.818131** | **−0.011203** | **−0.008646** |

- L12 先验选出的块与 §11.9.1 的 L12→L6 **完全相同**（同一目标深度、同一 base
  形状，规则只读源表）：升 `1f / 2i / 2f / 4o / 4i / 5v`，降
  `3v / 3i / 3f / 4v / 4f / 5i`。在 all-MiniLM-L6 上它是
  `+0.014113 / +0.014060`（对 int3）；在 par-L6 上是
  `−0.016161 / −0.007308`。**同一布局、同一字节账、同为 6 层，Δ 双指标符号翻转。**
- L6 先验选出的块与 L6 自己的 rescan-k2 **完全相同**（升
  `0i / 0f / 1v / 4k / 4i / 4f`，降 `2i / 2f / 3v / 3i / 3f / 4v`）；
  在 all-MiniLM-L6 上它是 `+0.016710 / +0.014032`，在 par-L6 上是
  `−0.011203 / −0.008646`，同样双负。
- par-L6 的 10k bootstrap：L12 先验对 int3 Pearson CI
  `[−0.023405, −0.009015]`（更好 `0.00%`）、Spearman
  `[−0.015931, +0.001055]`（更好 `4.48%`）；对 int4
  `−0.021716 / −0.017835`，两个 CI 整段为负。L6 先验对 int3
  Pearson CI `[−0.019033, −0.003352]`（更好 `0.26%`）、Spearman
  `[−0.018165, +0.000603]`（更好 `3.50%`）；对 int4
  `−0.016759 / −0.019173`，两个 CI 整段为负。

par-L6 没有做 36 块原生表，所以**不能**说“par-L6 没有 headroom”。但它也不需要
这个前提：本轮要证的是“布局/规则可迁移”，而**L6 自己在 all-MiniLM-L6 上的
最优布局，原样投到同架构的 par-L6 就双负**。这已经足以否定“L12→L6 的正向
是可复用的跨模型规则”，并把它归为**模型对特定**。

#### 11.10.5 结论与边界

- **M1.9 的 L12→L6 正向是模型对特定的，不是可迁移规则。** 同一套 L12 规则/
  L6 原生布局，换到同深度的 par-L6 后，相对各自 int3 的 ΔP/ΔS 从
  `+0.0141/+0.0141`（L6-L12 先验）、`+0.0167/+0.0140`（L6 原生）翻成
  `−0.0162/−0.0073`、`−0.0112/−0.0086`；在 3 层的 L3 上，两条规则候选
  相对 int3 的 CI 都整段为负（`−0.0125/−0.0108` 与 `−0.0108/−0.0132`）。
- **规则只能当“某一对源/目标模型”的候选生成器，不能当运行时默认。**
  它没有读目标块表，这一点在 L12→L6 上像优点；但 L3/par-L6 证明它也会
  把源模型的排序原样搬错。**跨模型块级敏感度排序不守恒。**
- **L3 的原生重扫只打平 int3，不能反推“3-bit 追平 int4”。** k1 `3/12/3`
  对 int3 是 `+0.000045 / +0.002787`（CI 都跨 0），k2 `4/10/4` 是
  `−0.004695 / +0.000452`（Pearson CI 跨 0、Spearman CI 跨 0），对 int4
  都没有统计优势。3 层模型的同字节块重排没有可用 headroom；这与 L6 的
  in-sample `0.831304` 不能互相外推。
- **运行时默认不动**：仍是 `I10` 外部保守、`K10K11`/`R12` STS in-domain；
  L3/par-L6 的新候选都不进默认。
- **边界**：两套新模型都只跑 STS-B 1,379 对、seq 64；L3 做了 18 块 × 2 方向
  全量单块表，par-L6 没有做 36 块原生表；par-L6 的结论是“布局不迁移”，
  不是“par-L6 无 headroom”。bootstrap 是 pair-level，不是句子级；两套新数据
  的 2,758 个句子 ID 都唯一、不跨 pair 复用，但没有做按句子切分的 held-out；
  也没有把新候选跑 SICK-R/STS12/STS13 三套外部数据。
- **下一步优先级**：**停止调跨模型规则和 `per_shape`**。要么接受“每个模型
  自己重扫 36/72 块表”，要么转检索/分类任务头；只有在这两类证据上仍为正的
  块组合，才进入 int5/f32/侧车或 MXQ。不要再拿 L12→L6 的单向正向当通用结论。

#### 11.10.6 复算

```bash
# L3 数据：STS-B 全量 1,379 对，seq 64
python scripts/wsp_export_sts_pairs.py \
  --dataset mteb/stsbenchmark-sts --split test --pairs 1379 \
  --out paraphrase-MiniLM-L3-sts-full \
  --model paraphrase-MiniLM-L3-v2 --seq 64 --local-only

# L3 纯 int3 base：18 块全部 bits=3（2,695,680 B）
# L3 单块表：18 块 × 3->2 / 3->4，各 1,379 对
python scripts/wsp_budget_perturb.py \
  --base budget-l3-pure-int3-all3.json --bits 2 \
  --out-dir perturb-l3-3to2 --run --resume --layers 3 --threads 6 --jobs 4 \
  --exe target/release/examples/bert6.exe \
  --model paraphrase-MiniLM-L3-v2.safetensors \
  --data-dir paraphrase-MiniLM-L3-sts-full \
  --f32-cache l3-f32-cache-full.json --base-result l3-int3-result.json

python scripts/wsp_budget_perturb.py \
  --base budget-l3-pure-int3-all3.json --bits 4 \
  --out-dir perturb-l3-3to4 --run --resume --layers 3 --threads 6 --jobs 4 \
  --exe target/release/examples/bert6.exe \
  --model paraphrase-MiniLM-L3-v2.safetensors \
  --data-dir paraphrase-MiniLM-L3-sts-full \
  --f32-cache l3-f32-cache-full.json --base-result l3-int3-result.json

# L3 原生 k1/k2（per_shape=2 会被收缩成 2/1/1）
python scripts/wsp_budget_rescan.py \
  --base budget-l3-pure-int3-all3.json \
  --summary-3to2 perturb-l3-3to2/perturb-summary.json \
  --summary-3to4 perturb-l3-3to4/perturb-summary.json \
  --per-shape 1 --out budget-l3-rescan-k1.json
python scripts/wsp_budget_rescan.py \
  --base budget-l3-pure-int3-all3.json \
  --summary-3to2 perturb-l3-3to2/perturb-summary.json \
  --summary-3to4 perturb-l3-3to4/perturb-summary.json \
  --per-shape 2 --out budget-l3-rescan-k2.json

# L3 两条规则候选：源表 L12 / L6，目标 3 层
python scripts/wsp_budget_rule.py \
  --base budget-l3-pure-int3-all3.json \
  --table docs/research/data/wsp-budget-l12-perturb.tsv \
  --target-layers 3 --per-shape 2 --out budget-l3-rule-l12.json
python scripts/wsp_budget_rule.py \
  --base budget-l3-pure-int3-all3.json \
  --table docs/research/data/wsp-budget-l6-perturb.tsv \
  --target-layers 3 --per-shape 2 --out budget-l3-rule-l6.json

# par-L6 数据：同 STS-B 1,379 对，seq 64
python scripts/wsp_export_sts_pairs.py \
  --dataset mteb/stsbenchmark-sts --split test --pairs 1379 \
  --out paraphrase-MiniLM-L6-sts-full \
  --model paraphrase-MiniLM-L6-v2 --seq 64 --local-only

# par-L6 纯 int3 / 纯 int4（复用 L6 的架构预算；int4 只设全局位宽 4）
TRIT_LAYERS=6 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l6-pure-int3.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=par-l6-f32-cache-full.json \
bert6 --file paraphrase-MiniLM-L6-v2.safetensors \
  --data-dir paraphrase-MiniLM-L6-sts-full --activation-mode f32 \
  --out par-l6-int3.json
TRIT_LAYERS=6 TRIT_WEIGHT_BITS=4 TRIT_THREADS=24 TRIT_SKIP_DIAG=1 \
TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 TRIT_F32_CACHE=par-l6-f32-cache-full.json \
bert6 --file paraphrase-MiniLM-L6-v2.safetensors \
  --data-dir paraphrase-MiniLM-L6-sts-full --activation-mode f32 \
  --out par-l6-int4.json

# par-L6 两条规则候选
python scripts/wsp_budget_rule.py \
  --base budget-l6-pure-int3.json \
  --table docs/research/data/wsp-budget-l12-perturb.tsv \
  --target-layers 6 --per-shape 2 --out budget-par-l6-rule-l12.json
python scripts/wsp_budget_rule.py \
  --base budget-l6-pure-int3.json \
  --table docs/research/data/wsp-budget-l6-perturb.tsv \
  --target-layers 6 --per-shape 2 --out budget-par-l6-rule-l6.json
```

L3 的表：`data/wsp-budget-l3-perturb.tsv`（36 行）、
`data/wsp-budget-l3-candidates.tsv`（7 行）、`data/wsp-budget-l3-bootstrap.tsv`
（16 行）；par-L6 的表：`data/wsp-budget-parl6-candidates.tsv`（5 行）、
`data/wsp-budget-parl6-bootstrap.tsv`（8 行）；四个新方向的规则候选追加在
`data/wsp-budget-rule-candidates.tsv`（6 行）。

### 11.11 检索任务头：STS 最优布局在 SciFact 上不成立（M1.11）

§11.10.5 的下一步写的是“停止跨模型规则和 `per_shape`，转检索/分类任务头”。
本轮先做检索头，不做分类头：检索不需要训练新头，直接检验量化后的句向量余弦
排序是否还成立，和 MiniLM 的部署用法一致；分类头在 M1.12 单独做（§11.12）。

#### 11.11.1 口径

- 数据集 `mteb/scifact`：corpus 5,183 篇、queries 1,109 条、test qrels 339 行；
  去重后 300 个带 qrels 的 query。corpus 取 `title + text`，query 取 `text`，
  `seq=64`（截断），hidden 384；共 6,292 行、`402,688×384 = 154.6M` 个 f32。
- 八个 embedding 都由同一 `sentence_vectors` 生成：mask mean pooling + L2
  normalize，与 STS 的 `sentence_vectors` 完全同一份。corpus/query 共享一个
  矩阵，检索只做 query×corpus 余弦排序。
- 指标：MRR@10、Recall@10/100、nDCG@10/100；qrels 按二值相关算。300 个 query
  做 10k query-level bootstrap（seed 7）；没有 corpus bootstrap。
- 配置：L12 `f32/int3/int4/R12`，L6 `f32/int3/int4/rescan-k2`。`R12` 从
  `budget-l12-pure-int3.json` 复算，分布 `6/54/12`、`10,782,720 B`；L6
  `rescan-k2` 是 `6/24/6`、`5,391,360 B`。两者都与各自纯 int3 严格同字节。
- 为任务头新增 `WSPEMB1` 落盘：`TRIT_EMBED_DUMP=<dir>` +
  `TRIT_EMBED_TAG=<name>`，文件头 `WSPEMB1\0` + rows:u32 + cols:u32 +
  row-major f32；纯 f32 由 `f32_base` 落为 `pure-f32.f32`，量化档由 `modes`
  按 tag 落盘。`data.rs` 的矩阵元素上限从 `1<<27` 提到 `1<<29`，否则
  `402,688×384` 在 `bert6` 加载时直接 panic。

#### 11.11.2 L12：R12 的 STS 增益没有转成检索

| 配置 | 分布 | 字节 | STS P | STS S | MRR@10 | R@10 | R@100 | nDCG@10 | nDCG@100 |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| L12 纯 f32 | — | 84,934,656 | 0.837595 | 0.830894 | 0.553757 | 0.733167 | 0.896667 | 0.589919 | 0.626971 |
| L12 纯 int3 | 0/72/0 | 10,782,720 | 0.818440 | 0.813363 | 0.496705 | 0.676278 | 0.842667 | 0.531968 | 0.568245 |
| L12 纯 int4 | 0/0/72 | 13,436,928 | 0.837505 | 0.832047 | 0.529372 | 0.716389 | 0.893333 | 0.568436 | 0.608503 |
| **L12 R12** | **6/54/12** | **10,782,720** | **0.833671** | **0.835411** | **0.482403** | **0.683389** | **0.866000** | **0.525849** | **0.565486** |

`R12` 在 STS 上相对 int3 是 `+0.015231 / +0.022050`，Spearman 还比 int4 高
`+0.003364`；但检索 MRR `0.482` 低于 int3 `0.497`、更低于 int4 `0.529`，
nDCG@10 `0.526` 低于 int3 `0.532`、int4 `0.568`。Recall@100 `0.866` 比 int3
`0.843` 高 `0.023`（点估计），但仍低于 int4 `0.893`；说明相关文档在更深处能
被捞到一些，top-10 排序本身更差。

`R12` 对 int3 的 10k query bootstrap：

- MRR `−0.014474`，CI `[−0.035253, +0.005781]`，候选更好 `8.29%`；
- Recall@10 `+0.007016`，CI `[−0.016778, +0.030444]`，候选更好 `71.43%`；
- Recall@100 `+0.023097`，CI `[−0.001667, +0.048333]`，候选更好 `95.87%`；
- nDCG@10 `−0.006279`，CI `[−0.024015, +0.011341]`，候选更好 `24.40%`；
- nDCG@100 `−0.002941`，CI `[−0.018624, +0.012491]`，候选更好 `36.13%`。

五个 CI 都跨 0：**top-10 排序（MRR/nDCG@10）点估计为负，Recall@10/100 点估计为
正但都不显著；不能写“R12 的 STS 增益转到检索”。** 对 int4 更差：MRR `−0.047132`
`[−0.076319, −0.018868]`、Recall@100 `−0.027297`
`[−0.049000, −0.007333]`、nDCG@10 `−0.042747`
`[−0.068165, −0.017325]`、nDCG@100 `−0.043115`
`[−0.064823, −0.021745]`；Recall@10 `−0.033184`
`[−0.068556, +0.001111]` 是唯一 CI 跨 0 的。对 f32 五个指标 CI 全部为负：
MRR `−0.071590` `[−0.100799, −0.043157]`、Recall@10 `−0.049757`
`[−0.085000, −0.016994]`、Recall@100 `−0.030599`
`[−0.055000, −0.007992]`、nDCG@10 `−0.064242`
`[−0.090263, −0.039157]`、nDCG@100 `−0.061650`
`[−0.084376, −0.039637]`。

#### 11.11.3 L6：STS 最优 rescan-k2 在检索上显著更差

| 配置 | 分布 | 字节 | STS P | STS S | MRR@10 | R@10 | R@100 | nDCG@10 | nDCG@100 |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| L6 纯 f32 | — | 42,467,328 | 0.827406 | 0.820325 | 0.534049 | 0.708944 | 0.896000 | 0.571812 | 0.613864 |
| L6 纯 int3 | 0/36/0 | 5,391,360 | 0.814594 | 0.810425 | 0.510308 | 0.671278 | 0.873333 | 0.541137 | 0.584826 |
| L6 纯 int4 | 0/0/36 | 6,718,464 | 0.826061 | 0.819355 | 0.534526 | 0.719611 | 0.889333 | 0.573827 | 0.611194 |
| **L6 rescan-k2** | **6/24/6** | **5,391,360** | **0.831304** | **0.824457** | **0.474242** | **0.661611** | **0.891556** | **0.514939** | **0.564027** |

`rescan-k2` 在 STS 上相对 int3 是 `+0.016710 / +0.014032`，相对 int4 是
`+0.005243 / +0.005103`，确实是 L6 的 STS 最优点；但检索 MRR `0.474` 低于
int3 `0.510`、更低于 int4 `0.535`，nDCG@10 `0.515` 低于 int3 `0.541`、int4
`0.574`，Recall@10 `0.662` 也低于 int3 `0.671`、int4 `0.720`。唯一更高的是
Recall@100 `0.892`（高于 int3 `0.873`、int4 `0.889`），但 top-10 排序明显更差。

`rescan-k2` 对 int3 的 10k query bootstrap：

- MRR `−0.036272`，CI `[−0.065249, −0.007896]`，候选更好 `0.59%`；
- Recall@10 `−0.009852`，CI `[−0.042667, +0.022556]`，候选更好 `27.24%`；
- Recall@100 `+0.018233`，CI `[−0.004000, +0.041667]`，候选更好 `94.48%`；
- nDCG@10 `−0.026406`，CI `[−0.052461, −0.000934]`，候选更好 `2.10%`；
- nDCG@100 `−0.020964`，CI `[−0.044046, +0.001554]`，候选更好 `3.38%`。

**MRR 和 nDCG@10 的 CI 整段为负；Recall@10 中性；Recall@100 点估计正但 CI
跨 0。** 对 int4：MRR `−0.060348` `[−0.092095, −0.030075]`、Recall@10
`−0.058298` `[−0.094778, −0.023778]`、nDCG@10 `−0.059010`
`[−0.086957, −0.032024]`、nDCG@100 `−0.047225`
`[−0.072066, −0.023567]`；Recall@100 `+0.002203`
`[−0.021111, +0.026667]` 跨 0。对 f32：MRR `−0.059883`
`[−0.091419, −0.029664]`、Recall@10 `−0.047492`
`[−0.083889, −0.011442]`、nDCG@10 `−0.056978`
`[−0.085822, −0.029147]`、nDCG@100 `−0.049905`
`[−0.074571, −0.026053]`；Recall@100 `−0.004384`
`[−0.027778, +0.020000]` 跨 0。

#### 11.11.4 结论、机制解释与边界

- **STS 增益没有转到检索。** `R12` 的 STS 对 int3 双正、Spearman 对 int4 也正，
  L6 `rescan-k2` 的 STS 对 int3/int4 都正；但检索侧 `R12` 对 int3 的 MRR/
  nDCG 点估计为负、CI 跨 0，对 int4/f32 基本整段为负；`rescan-k2` 对 int3 的
  MRR、nDCG@10 CI 整段为负。**不能写“块级 STS 最优布局在检索上也更好”。**
- **`R12` 和 L6 `rescan-k2` 只能保留为 STS in-domain 候选，不能进检索任务头
  默认；运行时默认仍是 `I10` 外部保守、`K10K11`/`R12` STS in-domain。**
  任务头在这里起的是“否决门”作用，不是新的调参维度。
- **一个与读数一致、但本轮没有直接证明的解释**：STS 只看同一对句子的相对分数，
  块级重排可以在不改变全局近邻结构的情况下改善 pair ordering；检索排序直接依赖
  corpus 全体的绝对几何，top-10 邻居对局部扰动更敏感。本轮没有做几何诊断
  （邻域保持率、各向异性、距离秩相关），所以这只是机制假设，不是因果结论。
- **边界**：只做 SciFact 一个检索集、300 个 qrels query、seq 64 截断、mean
  pooling、两个 6/12 层 MiniLM；没有检索微调；bootstrap 是 query-level，不是
  corpus-level；分类头已在 §11.12–§11.21 完成。不能外推到所有检索/分类任务，
  也不能写“量化 embedding 普遍不适合检索”。本轮负向结论的具体范围是：“在这套 SciFact
  口径下，`R12` 和 L6 `rescan-k2` 的 STS 增益没有转成检索增益”。
- **下一步**：分类头路线已做到 §11.21（SST-2 内稳定、跨 CoLA/hate 不复现；
  M1.17–M1.21 给出伴随结构、非充分性、方向症状、`sep` 的 SST-2 L12
  伴随读数和低剂量早饱和），不要再调 `R12`。
  否则接受“BUDGET 只在 STS 相关性上有效”，把 `R12` 留在 STS in-domain，
  不进运行时默认。

#### 11.11.5 复算

```bash
# 1) SciFact 导出（L12/L6 各一次；低内存，不加载 transformer）
OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 python scripts/wsp_export_scifact.py \
  --dataset mteb/scifact --out scifact/data-l12 \
  --model all-MiniLM-L12-v2 --seq 64 --local-only
OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 python scripts/wsp_export_scifact.py \
  --dataset mteb/scifact --out scifact/data-l6 \
  --model all-MiniLM-L6-v2 --seq 64 --local-only

# 2) R12：6 张降 int2、12 张升 int4，字节严格等于纯 int3
python scripts/wsp_budget_variant.py \
  --base budget-l12-pure-int3.json \
  --up 1:k,10:v,8:o,6:o,11:v,9:o,7:o,0:o,5:k,4:v,6:q,0:v \
  --down 5:f,7:f,7:v,8:v,9:v,6:v \
  --require-exact --out scifact/r12/swap-r12.json

# 3) 编码：以 L12 int3 为例；纯 f32 由 f32_base 先落为 pure-f32.f32
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l12-pure-int3.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 \
TRIT_EMBED_DUMP=scifact/emb TRIT_EMBED_TAG=l12-int3 \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir scifact/data-l12 --activation-mode f32 \
  --out scifact/runs/l12-int3.json
# 纯 int4：TRIT_WEIGHT_BITS=4，不传 TRIT_BUDGET
# R12：TRIT_WEIGHT_BITS=3 TRIT_BUDGET=scifact/r12/swap-r12.json
# L6 rescan-k2：TRIT_LAYERS=6 TRIT_BUDGET=l6/l6-rescan-k2.json

# 4) 评测：读 WSPEMB1，10k query bootstrap
python scripts/wsp_retrieval_eval.py \
  --texts scifact/data-l12/texts.json --qrels scifact/data-l12/qrels.json \
  --run f32=scifact/emb/l12-pure-f32.f32 \
  --run int3=scifact/emb/l12-int3.f32 \
  --run int4=scifact/emb/l12-int4.f32 \
  --run R12=scifact/emb/l12-r12.f32 \
  --baseline f32 --baseline int3 --baseline int4 --bootstrap 10000 \
  --out scifact/eval-l12-all.json
```

每次运行都会先落 `pure-f32.f32`；跑下一档前先复制为 `l12-pure-f32.f32` /
`l6-pure-f32.f32`，避免被覆盖。表见
`data/wsp-retrieval-scifact-l12.tsv`（4 行）、
`data/wsp-retrieval-scifact-l6.tsv`（4 行）、
`data/wsp-retrieval-scifact-bootstrap.tsv`（30 行）。

## 11.12 分类任务头（M1.12，SST-2 / 冻结编码器 + 线性 probe）

> **M1.26 修正**：本节至 §11.25 的 SST-2 L12 `f32` 列实际是 ternary
> （`TRIT_WEIGHT_BITS=2`），不是 f32；相关 `f32` 对比与几何结论改读
> §11.26。CoLA/hate 的 L12 `f32` 是真 f32，结论不变。
> **M1.28 追加**：本节 L6 的 `f32` 列实际也是 ternary，不是 f32；L6 真
> f32→int4 为 `−0.004964`（CI `[−0.013729,+0.003844]`），旧
> `int4−f32≈+0.0060` 是 ternary→4-bit 恢复。L6 修正表见 §11.28。
> **M1.29 追加**：本节 L12 的 `int3`/`R12` 和 L6 的 `int3`/`rescan-k2`
> 也都在真 f32 上重算，全部低于真 f32；L12 `int3`/`R12` 的 CI 整段为负，
> L6 的三个量化档 CI 都跨 0。旧表里除 int4 外的量化“增益”同样是
> ternary 基线造成的；全臂表见 §11.29。

### 11.12.1 口径

- 数据集 `SetFit/sst2`：6,920 train / 1,821 test，标签 0/1；test 留出不参与
  C 选择。没有额外 validation split，test 就是最终评测集。
- 文本 seq 64、mask mean pooling + L2 normalize，与检索头用同一份 `WSPEMB1`
  句向量口径；编码器全程冻结，只训练一个 logistic 线性 probe。
- L12 四档：`f32 / int3 / int4 / R12`；L6 四档：`f32 / int3 / int4 /
  rescan-k2`。R12 仍是 `6/54/12`、`10,782,720 B`；rescan-k2 仍是 `6/24/6`、
  `5,391,360 B`。
- 主口径的 `C` 由 train 内 5 折 `StratifiedKFold` 在
  `{0.01, 0.1, 1, 10}` 上选出，再用全 train 重训、test 只评一次；不做
  encoder 微调，不拿 test 选 `C`。另跑固定 `C=1`、`C=10` 和逐维标准化的
  敏感性，见 §11.12.4。
- 10k test-level paired bootstrap；同一重采样索引同时用于两档，报告
  accuracy 差值、95% CI 和 `better_fraction`。
- 8,741 条文本的 `input.f32` 与 `reference.f32` 各 859 MB，一次性读入会超过
  本机进程 commit 预算；编码按 1,500 条分块，拼回 `WSPEMB1` 后再评测。

### 11.12.2 主结果（CV 选 C）

L12：

| 配置 | accuracy | macro-F1 | AUC | selected C | train-CV acc | f32-probe transfer acc |
|---|---:|---:|---:|---:|---:|---:|
| f32 | 0.800110 | 0.800078 | 0.878938 | 10 | 0.774711 | 0.800110 |
| int3 | 0.823723 | 0.823662 | 0.905766 | 1 | 0.802457 | 0.753981 |
| int4 | 0.840198 | 0.840113 | 0.918861 | 10 | 0.828035 | 0.771005 |
| R12 | 0.814937 | 0.814848 | 0.901555 | 1 | 0.799855 | 0.738605 |

L6：

| 配置 | accuracy | macro-F1 | AUC | selected C | train-CV acc | f32-probe transfer acc |
|---|---:|---:|---:|---:|---:|---:|
| f32 | 0.798462 | 0.798427 | 0.882219 | 10 | 0.777168 | 0.798462 |
| int3 | 0.809445 | 0.809324 | 0.894031 | 10 | 0.797110 | 0.707304 |
| int4 | 0.804503 | 0.804439 | 0.893376 | 1 | 0.802312 | 0.747392 |
| rescan-k2 | 0.802306 | 0.802186 | 0.888280 | 1 | 0.792341 | 0.745744 |

“f32-probe transfer” 是只用该模型 f32 档 train 训练 probe，再把它原样投到
同模型其他档 test 的 accuracy；它不是重新训练后的读数。

### 11.12.3 bootstrap：关键比较

L12，probe 重新训练：

- `R12 − int3 = −0.008842`，95% CI `[−0.021417, +0.003844]`，
  better `0.9094`（这里 better 指 int3 更好）；R12 点估计更低，CI 跨 0。
- `R12 − int4 = −0.025402`，CI `[−0.041735, −0.009336]`；整段为负。
- `R12 − f32 = +0.014632`，CI `[−0.003295, +0.032949]`；R12 点估计更高，
  但不显著。
- `int4 − int3 = +0.016560`，CI `[+0.001098, +0.032400]`；int4 显著更高。
- `f32 − int3 = −0.023474`，CI `[−0.041735, −0.005491]`；
  `f32 − int4 = −0.040034`，CI `[−0.058759, −0.020868]`。即在这个
  线性 probe 口径下，L12 的 int3/int4 都显著高于 f32。

L6，probe 重新训练：

- `rescan-k2 − int3 = −0.007209`，CI `[−0.022515, +0.007688]`；CI 跨 0。
- `rescan-k2 − int4 = −0.002220`，CI `[−0.018122, +0.013180]`；基本打平。
- `rescan-k2 − f32 = +0.003619`，CI `[−0.013180, +0.020868]`；不显著。
- `int3 − int4 = +0.004989`，CI `[−0.009885, +0.020319]`；不显著。

f32 probe 直接迁移（不重新训练）：

- L12：int3 `−0.046248` CI `[−0.068094, −0.024163]`；int4 `−0.029229`
  CI `[−0.050522, −0.008237]`；R12 `−0.061604` CI
  `[−0.083471, −0.039539]`。三档都显著低于 f32 自评。
- L6：int3 `−0.091471` CI `[−0.114223, −0.068644]`；int4 `−0.051274`
  CI `[−0.073586, −0.029654]`；rescan-k2 `−0.052979` CI
  `[−0.073037, −0.032949]`。同样三档都显著低于 f32 自评。

这说明量化后的表示确实变了：f32 probe 的决策边界不能直接迁到量化档；
但重新训练 probe 后，L12 的 int3/int4 又能超过 f32。两种读数回答的是不同问题，
不能混用。

### 11.12.4 C 与标准化敏感性

固定 `C=1` 时，量化档的优势会被放大：L12 f32 只有 `0.777046`，int4 为
`0.842394`；但 f32 在 `C=10` 升到 `0.800110`，int4 为 `0.840198`。L6 在
`C=10` 时 f32 `0.798462`、int3 `0.809445`，差距缩到 1.1 个百分点；
rescan-k2 `0.805601`，与 f32 只差 0.7 个百分点。逐维标准化、固定 `C=1`
时 L12 f32 `0.789676`、int4 `0.841296`，int4 仍高；L6 f32 `0.800110`，
int3/int4/rescan 都在 `0.802–0.807`，基本打平。

因此主结论用 CV 选 C 的读数；固定 `C=1` 只能说明“在这个正则强度下量化档
更可分”，不能当无条件结论。完整网格见
`data/wsp-classify-sst2-sensitivity.tsv`。

### 11.12.5 结论与边界

- **STS 的块级增益没有转成分类增益。** L12 `R12` 对 int3 点估计低
  `−0.008842`、CI 跨 0，对 int4 整段为负；L6 `rescan-k2` 对 int3/int4
  的 CI 都跨 0，点估计也不高于 int3。`R12`/`rescan-k2` 仍然只能留在
  STS in-domain，不能进分类任务头默认。
- **一个额外的正向观察，但不属于 BUDGET 的 STS 增益**：L12 的均匀 int4
  在线性 probe 上比 f32 高 `+0.040088`（0.840198−0.800110；10k bootstrap
  均值 `+0.040034`，CI 整段为正），int3 也高 `+0.023613`（bootstrap 均值
  `+0.023474`）。这更像量化把异常维度/尺度压平后改善了线性可分性，而不是
  `R12` 的块级重排有效；L6 上这个优势缩到不显著。没有做几何诊断，不能把它
  写成“量化普遍提升分类”。
- **f32 probe 不能直接迁移**到量化档：L12/L6 三档的 transfer accuracy 都
  显著低于 f32 自评。任务头比较必须重新训练 probe；把“f32 probe 投到量化”
  当成量化掉点会高估损失，把“各档重训 probe”当成纯表示差异又会忽略正则化。
- 边界：单数据集 SST-2（SetFit 6,920/1,821）、单任务二分类、主 probe 家族
  （logistic regression）、无 encoder 微调、seq 64、两个 MiniLM；没有第二
  分类集、没有 MLP、没有句子级去重。不能外推到所有分类任务，也不能从 SST-2
  反推生成任务。第二种线性头（LinearSVC）见 §11.13。
- 第二种线性头（LinearSVC）的复验见 §11.13：L12 `int4 > f32` 仍成立，
  `R12` 仍不高于 int3；L6 仍无显著差异。

### 11.12.6 复算

```bash
# 1) 导出 SST-2（直连 JSONL，避免 HF datasets 的 X-Repo-Commit 头问题）
python scripts/wsp_export_sst2.py \
  --train sst2/raw/train.jsonl --test sst2/raw/test.jsonl \
  --out sst2/data-l12 --model all-MiniLM-L12-v2 --seq 64
python scripts/wsp_export_sst2.py \
  --train sst2/raw/train.jsonl --test sst2/raw/test.jsonl \
  --out sst2/data-l6 --model all-MiniLM-L6-v2 --seq 64

# 2) 8,741 条一次性读入会超 commit 预算，按 1,500 条分块
python scripts/wsp_chunk_embed_input.py \
  --src sst2/data-l12 --dst sst2/chunks-l12-1500 --chunk-size 1500
python scripts/wsp_chunk_embed_input.py \
  --src sst2/data-l6 --dst sst2/chunks-l6-1500 --chunk-size 1500

# 3) 编码：以 L12 int3 的 chunk-000 为例；其余档按 §11.11.5 的 budget 规则
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l12-pure-int3.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 \
TRIT_EMBED_DUMP=sst2/emb12 TRIT_EMBED_TAG=l12-int3-c000 \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir sst2/chunks-l12-1500/chunk-000 --activation-mode f32 \
  --out sst2/runs12/l12-int3-c000.json

# 4) 评测：CV 选 C + 10k test bootstrap
python scripts/wsp_classify_eval.py \
  --texts sst2/data-l12/texts.json \
  --run l12-f32='sst2/emb12/l12-pure-f32-c*.f32' \
  --run l12-int3='sst2/emb12/l12-int3-c*.f32' \
  --run l12-int4='sst2/emb12/l12-int4-c*.f32' \
  --run l12-r12='sst2/emb12/l12-r12-c*.f32' \
  --group 'l12=l12-f32|l12-int3,l12-int4,l12-r12' \
  --cv-C --bootstrap 10000 --out sst2/eval-l12-cv.json
```

表见 `data/wsp-classify-sst2-l12.tsv`（4 行）、
`data/wsp-classify-sst2-l6.tsv`（4 行）、
`data/wsp-classify-sst2-bootstrap.tsv`（24 行）、
`data/wsp-classify-sst2-sensitivity.tsv`（16 行）。

## 11.13 第二种线性 probe 复验（M1.13，SST-2 / LinearSVC）

### 11.13.1 口径

M1.12 的 L12 “均匀 int4 高于 f32”是用 logistic regression 得到的，可能只是
logistic 损失/正则化的产物。M1.13 不改 embedding、不改 split，只把 probe 换成
平方 hinge 的 `LinearSVC`（仍是线性头；按用户口径不做 MLP、不做 encoder 微调）。
`C` 仍由 train 内 5 折 `StratifiedKFold` 在 `{0.01,0.1,1,10}` 中选，test 只评
一次；10k test-level paired bootstrap，除 `--probe linear_svc` 外其余口径与
§11.12 相同。

### 11.13.2 主结果

L12：

| 配置 | accuracy | macro-F1 | AUC | selected C | train-CV acc | f32-probe transfer acc |
|---|---:|---:|---:|---:|---:|---:|
| f32 | 0.797913 | 0.797896 | 0.878679 | 1 | 0.774133 | 0.797913 |
| int3 | 0.818781 | 0.818680 | 0.906843 | 10 | 0.803324 | 0.751785 |
| int4 | 0.841845 | 0.841799 | 0.920562 | 0.1 | 0.826734 | 0.766063 |
| R12 | 0.814388 | 0.814327 | 0.901651 | 0.1 | 0.799566 | 0.735310 |

L6：

| 配置 | accuracy | macro-F1 | AUC | selected C | train-CV acc | f32-probe transfer acc |
|---|---:|---:|---:|---:|---:|---:|
| f32 | 0.802306 | 0.802285 | 0.884427 | 10 | 0.780636 | 0.802306 |
| int3 | 0.809445 | 0.809344 | 0.893821 | 1 | 0.797110 | 0.650192 |
| int4 | 0.805052 | 0.805006 | 0.893225 | 0.1 | 0.801301 | 0.683690 |
| rescan-k2 | 0.802856 | 0.802760 | 0.887944 | 0.1 | 0.792486 | 0.690280 |

与 §11.12 的 logistic probe 对比：L12 int4 `0.841845` vs `0.840198`
（`+0.001647`），f32 `0.797913` vs `0.800110`（`−0.002197`），所以
`int4 − f32` 从 `+0.040088` 扩大到 `+0.043932`；R12 `0.814388` vs
`0.814937`（`−0.000549`），int3 `0.818781` vs `0.823723`（`−0.004942`）。
L6 两套 probe 除 f32 高 `+0.003844` 外，其余档差 `0.0000–0.0006`，基本同读数。
完整对照见 `data/wsp-classify-sst2-probe-compare.tsv`。

### 11.13.3 bootstrap：SVM probe

L12：

- `int4 − f32 = +0.043783`，95% CI `[+0.025261,+0.062603]`；
  即 int4 高于 f32 的结论在 SVM 下仍然成立，且区间比 logistic 更靠上。
- `int3 − f32 = +0.020769`，CI `[+0.001098,+0.040088]`；显著为正。
- `R12 − int3 = −0.004502`，CI `[−0.019220,+0.010434]`；不显著。
- `R12 − int4 = −0.027516`，CI `[−0.042834,−0.012630]`；整段为负。
- `R12 − f32 = +0.016267`，CI `[−0.001647,+0.034596]`；不显著。

L6：

- `int3 − f32 = +0.006939`，CI `[−0.012095,+0.026359]`；
  `int4 − f32 = +0.002492`，CI `[−0.017573,+0.023064]`；
  `rescan-k2 − f32 = +0.000303`，CI `[−0.017573,+0.018122]`。
  三个 CI 都跨 0，L6 没有 L12 那种量化高于 f32 的现象。
- int3/int4/rescan-k2 之间的两两 CI 也全部跨 0。

SVM 的 f32 共享 probe 迁移同样掉点，而且比 logistic 更明显：L12 投到
int3/int4/R12 为 `0.751785 / 0.766063 / 0.735310`（f32 自评 `0.797913`）；
L6 为 `0.650192 / 0.683690 / 0.690280`（f32 自评 `0.802306`）。这说明
量化改变的不只是边际尺度，线性决策边界的方向也变了；该结论仍只是观测，
没有做几何诊断。

### 11.13.4 结论与边界

- **第二种线性头复现了 M1.12 的排序**：L12 `int4 > int3 > f32`，`R12`
  不高于 int3 且低于 int4；L6 四档基本打平，量化相对 f32 不显著。
- 因此“L12 均匀 int4 对线性 probe 的正向”不是 logistic 损失特有的；它
  可能是量化对异常维度/尺度的压缩使线性边界更稳。但它仍然**不是** `R12`
  的块级重排增益，不能写成“BUDGET 布局提升分类”。
- 边界：同一批 SST-2 embedding、同一 split、只换线性损失；`LinearSVC` 的
  `C` 网格在 L12 int3 和 L6 f32 上取到边界 10，但 L12 的核心比较
  （int4 选 0.1、f32 选 1）不在边界，L6 的结论是“无显著优势”，扩网格也
  不会把 L6 的布局候选救成正向。仍没有第二分类集、没有 MLP（用户口径明确
  排除）、没有几何诊断；不能外推到所有分类任务。

表见 `data/wsp-classify-sst2-svc.tsv`（8 行）、
`data/wsp-classify-sst2-svc-bootstrap.tsv`（24 行）、
`data/wsp-classify-sst2-probe-compare.tsv`（8 行）。复算时把 §11.12.6 的
`wsp_classify_eval.py` 命令加上 `--probe linear_svc` 即可。

## 11.14 第二个分类数据集（M1.14，GLUE CoLA / 线性 probe）

### 11.14.1 口径

- 数据集 `GLUE/CoLA`，官方 zip
  `https://dl.fbaipublicfiles.com/glue/data/CoLA.zip`：8,551 train / 1,043 dev；
  `test.tsv` 只有 `index/sentence`、没有标签，因此用 `dev.tsv` 作 held-out
  test，下文所有 “test” 都指 dev。标签 `0 = unacceptable`、`1 = acceptable`；
  train 为 6,023/2,528，dev 为 721/322。
- 9,594 条文本（train+dev），seq 64、mask mean pooling + L2 normalize，编码器
  冻结；L12/L6 各四档，与 §11.12/§11.13 的量化预算相同（L12 `f32/int3/int4/R12`，
  L6 `f32/int3/int4/rescan-k2`）。
- CoLA dev 不是平衡集：多数类（acceptable）基线 accuracy `0.691275`，全判
  acceptable 的 macro-F1 `0.408730`。按 SST-2 的“accuracy 选 C + 无类别权重”
  口径，四个配置全部塌到多数类（§11.14.4），因此**主口径改为
  `class_weight="balanced"` + train 内 5 折按 macro-F1 选 C**；仍只训练线性头，
  不做 MLP、不做 encoder 微调。
- 两种线性头都跑：logistic regression 与平方 hinge `LinearSVC`；`C` 网格
  `{0.01,0.1,1,10}`，test 只评一次。10k test-level paired bootstrap 同时对
  accuracy 与 macro-F1 报差值、95% CI 和 `better_fraction`。另做
  `{0.01,...,100}` 扩网格与无类别权重敏感性（§11.14.4）。
- 编码同样按 1,500 条分块（9,594 → 7 块，前 6 块 1,500、尾块 594）；L12/L6
  每个档位拼回 `WSPEMB1` 后均为 9,594 行。

### 11.14.2 主结果（balanced + macro-F1 选 C）

L12：

| probe | 配置 | accuracy | macro-F1 | AUC | selected C | CV macro-F1 |
|---|---|---:|---:|---:|---:|---:|
| logreg | f32 | 0.574305 | 0.549444 | 0.602362 | 10 | 0.569175 |
| logreg | int3 | 0.558006 | 0.532857 | 0.595448 | 10 | 0.558885 |
| logreg | int4 | 0.587728 | 0.561518 | 0.603783 | 10 | 0.570000 |
| logreg | R12 | 0.566635 | 0.543040 | 0.585619 | 10 | 0.563494 |
| linear_svc | f32 | 0.577181 | 0.550525 | 0.606660 | 10 | 0.572624 |
| linear_svc | int3 | 0.566635 | 0.539541 | 0.595181 | 10 | 0.559755 |
| linear_svc | int4 | 0.585810 | 0.559916 | 0.606268 | 10 | 0.569985 |
| linear_svc | R12 | 0.563758 | 0.538937 | 0.590344 | 10 | 0.563314 |

L6：

| probe | 配置 | accuracy | macro-F1 | AUC | selected C | CV macro-F1 |
|---|---|---:|---:|---:|---:|---:|
| logreg | f32 | 0.582934 | 0.558366 | 0.603137 | 1 | 0.548995 |
| logreg | int3 | 0.574305 | 0.551128 | 0.584844 | 10 | 0.548159 |
| logreg | int4 | 0.573346 | 0.547339 | 0.595659 | 10 | 0.547077 |
| logreg | rescan-k2 | 0.570470 | 0.544950 | 0.583474 | 1 | 0.535794 |
| linear_svc | f32 | 0.573346 | 0.549910 | 0.601565 | 0.1 | 0.550060 |
| linear_svc | int3 | 0.571429 | 0.548302 | 0.584525 | 1 | 0.549473 |
| linear_svc | int4 | 0.573346 | 0.548644 | 0.595474 | 1 | 0.546789 |
| linear_svc | rescan-k2 | 0.574305 | 0.549872 | 0.592735 | 10 | 0.540597 |

CoLA 上线性 probe 本身很弱：balanced accuracy 只有 `0.57–0.59`（随机为 0.50），
AUC 约 `0.60`。因此这组读数检验的是“在弱线性可分的第二分类集上，int4 的
SST-2 正向是否还在”，不是“CoLA 状态-of-the-art”。

### 11.14.3 bootstrap：int4 的正向没有复现

L12，probe 重新训练、balanced logistic：

- `int4 − f32 = +0.013341`，95% CI `[−0.006711,+0.033557]`，
  better `0.9077`；点估计为正，但 CI 跨 0。
- `int3 − f32 = −0.016517`，CI `[−0.046021,+0.013423]`；点估计转负。
- `R12 − f32 = −0.007833`，CI `[−0.036433,+0.022052]`；没有分类增益。
- `int4 − int3 = +0.029858`，accuracy CI `[0.000000,+0.059444]`，
  better `0.9789`；int4 对 int3 只在 accuracy 上擦边，macro-F1 的
  `int4 − int3 = +0.028788`，CI `[−0.000054,+0.058031]`，下界擦边为负。

L12，balanced `LinearSVC`：

- `int4 − f32 = +0.008467`，CI `[−0.011505,+0.028763]`，better `0.8091`。
- `int3 − f32 = −0.010809`，CI `[−0.040268,+0.018217]`。
- `R12 − f32 = −0.013690`，CI `[−0.042186,+0.015340]`。
- `int4 − int3 = +0.019276`，CI `[−0.010547,+0.048897]`，不显著。

macro-F1 的读数和 accuracy 同向：L12 logreg `int4 − f32 = +0.012021`
（CI `[−0.007247,+0.031408]`），SVC `+0.009261`（CI `[−0.009135,+0.027831]`），
两个 CI 都跨 0。

L6：

- logreg：`int4 − f32 = −0.009679`（CI `[−0.031640,+0.012464]`），
  `rescan-k2 − f32 = −0.012451`（CI `[−0.040268,+0.016299]`）；量化档点估计
  反而略低。
- SVC：`int4 − f32 = −0.000126`（CI `[−0.022052,+0.022052]`），
  `rescan-k2 − f32 = −0.000950`（CI `[−0.028763,+0.029722]`）；四档基本打平。

**关键判据**：SST-2 上 L12 `int4−f32` 的点估计是 logistic `+0.040088`、
SVC `+0.043783`；CoLA 的 95% CI 上界分别只有 `+0.033557` / `+0.028763`。
也就是说，CoLA 这组数据在 95% 置信水平下已经把“SST-2 同量级的 +0.040~+0.044
增益”排除在区间外。它可以容许一个小的、约 1 个百分点的正向，但不能支持
“int4 的正向跨数据集稳定存在”。

### 11.14.4 不平衡与 C 边界敏感性

无类别权重的 logistic（accuracy 选 C）在 CoLA 上塌到多数类：

| 配置 | accuracy | macro-F1 | AUC | selected C |
|---|---:|---:|---:|---:|
| f32 | 0.687440 | 0.462187 | 0.604240 | 1 |
| int3 | 0.689358 | 0.443825 | 0.592998 | 1 |
| int4 | 0.686481 | 0.457052 | 0.603292 | 1 |
| R12 | 0.691275 | 0.408730 | 0.550015 | 0.01 |

`R12` 这一行正好等于“全判 acceptable”的基线（accuracy `0.691275`、
macro-F1 `0.408730`），说明无权重 accuracy 口径在 69/31 的 dev 上不可用；
它不是量化档之间的真实差异。在这个口径下 int4 也没有高于 f32
（`0.686481 < 0.687440`）。改成 balanced + macro-F1 选 C 后才有
§11.14.2 的读数。

C 网格也有边界效应：主口径四个 L12 logreg 配置都选到上界 `C=10`。把网格
扩到 `100` 后，f32/int4 都选 `100`，`int4−f32` 从 `+0.013423` 缩到
`+0.003836`（0.580058−0.576222）；int3 从 `0.558006` 升到 `0.571429`。
SVC 的 f32/int4 仍选 `C=10`，差值维持 `+0.008629`；R12 在扩网格后略降。
因此主表用 `0.01–10` 的协议读数，但 int4 的点估计优势并不稳：换一格 C 就
从 1.3 个百分点缩到 0.4 个百分点。

### 11.14.5 结论与边界

- **SST-2 的 L12 int4 正向没有跨数据集泛化。** 在 CoLA 上，logreg 点估计
  `+0.013341`、SVC `+0.008467`，两个 95% CI 都跨 0；L6 的 int4 与 f32
  打平或略低。把 SST-2 的 `+0.040/+0.044` 当成“量化普遍提升线性分类”的
  证据是不成立的。
- **`R12`/`rescan-k2` 在第二个分类集上仍没有任务头增益。** L12 `R12` 对 f32
  为 `−0.007833`（logreg）/ `−0.013690`（SVC），L6 `rescan-k2` 对 f32 为
  `−0.012451` / `−0.000950`，全部不显著；STS 的块级重排仍只能留在
  STS in-domain。
- **CoLA 的 probe 太弱，不能把“不显著”写成“已证不存在”。** balanced
  accuracy 只有 0.57–0.59、AUC 0.60，1,043 条 test 对 1–2 个百分点的效应
  没有足够功效；它能排除 SST-2 同量级（约 4 个百分点）的效应，但不能排除
  一个更小的、数据集/正则化特定的正向。要判“完全没有”需要更强 probe；
  第三数据集已在 §11.15 完成，但仍只能排除同量级效应，不能把“不显著”
  写成“已证不存在”。
- 边界：第二个数据集只有一个（GLUE CoLA）、dev 作 test、单 split、单 seq 64、
  两个 MiniLM、只训练线性头、无 MLP、无 encoder 微调；CoLA 的标签分布
  69/31，主口径必须带类别权重。不能外推到平衡/多分类/NLI/生成任务。
- 结论只改“泛化”判断，不改 SST-2 的原始读数：SST-2 上 int4 对 f32 的正向
  仍是该数据集/该 probe 家族的观察，但不再是跨数据集的普遍结论。

### 11.14.6 复算

```bash
# 1) 下载并解压 GLUE CoLA
curl -L -o cola-raw/CoLA.zip https://dl.fbaipublicfiles.com/glue/data/CoLA.zip
unzip -q cola-raw/CoLA.zip -d cola-raw

# 2) 导出：train.tsv -> train，dev.tsv -> test（test.tsv 无标签）
python scripts/wsp_export_sst2.py \
  --train cola-raw/CoLA/train.tsv --test cola-raw/CoLA/dev.tsv \
  --format cola --out cola/data-l12 --model all-MiniLM-L12-v2 --seq 64
python scripts/wsp_export_sst2.py \
  --train cola-raw/CoLA/train.tsv --test cola-raw/CoLA/dev.tsv \
  --format cola --out cola/data-l6 --model all-MiniLM-L6-v2 --seq 64

# 3) 1,500 条分块；编码前先把全量 texts.json 另存为 texts-l12.json/texts-l6.json
python scripts/wsp_chunk_embed_input.py \
  --src cola/data-l12 --dst cola/chunks-l12-1500 --chunk-size 1500
python scripts/wsp_chunk_embed_input.py \
  --src cola/data-l6 --dst cola/chunks-l6-1500 --chunk-size 1500

# 4) 编码：以 L12 int3 chunk-000 为例；f32 由每次运行的 pure-f32.f32 另存
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l12-pure-int3.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 \
TRIT_EMBED_DUMP=cola/emb12 TRIT_EMBED_TAG=l12-int3-000 \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir cola/chunks-l12-1500/chunk-000 --activation-mode f32 \
  --out cola/runs12/l12-int3-000.json

# 5) 主评测：balanced + macro-F1 选 C + accuracy/macro-F1 双 bootstrap
python scripts/wsp_classify_eval.py \
  --texts cola/texts-l12.json \
  --run l12-f32='cola/emb12/l12-pure-f32-*.f32' \
  --run l12-int3='cola/emb12/l12-int3-*.f32' \
  --run l12-int4='cola/emb12/l12-int4-*.f32' \
  --run l12-r12='cola/emb12/l12-r12-*.f32' \
  --group 'l12=l12-f32|l12-int3,l12-int4,l12-r12' \
  --cv-C --cv-metric macro_f1 --class-weight balanced \
  --bootstrap 10000 --bootstrap-metric both \
  --probe logreg --out cola/eval-l12-balanced-cv.json
```

L6 把前缀换成 `l6`、budget 换成 `budget-l6-pure-int3.json`/`l6-rescan-k2.json`，
`--probe` 换成 `linear_svc` 即得另一套。表见
`data/wsp-classify-cola-l12.tsv`（8 行）、`data/wsp-classify-cola-l6.tsv`
（8 行）、`data/wsp-classify-cola-l12-bootstrap.tsv`（48 行）、
`data/wsp-classify-cola-l6-bootstrap.tsv`（48 行）、
`data/wsp-classify-cola-l12-probe-compare.tsv`（4 行）、
`data/wsp-classify-cola-l6-probe-compare.tsv`（4 行）、
`data/wsp-classify-cola-l12-unweighted.tsv`（4 行）、
`data/wsp-classify-cola-l12-cgrid.tsv`（16 行）。

## 11.15 第三个分类数据集（M1.15，TweetEval/hate / 线性 probe）

### 11.15.1 口径

- 数据集 `tweet_eval/hate`（HuggingFace `tweet_eval` 的 `hate` 配置），原始
  parquet：
  `https://huggingface.co/datasets/tweet_eval/resolve/main/hate/train-00000-of-00001.parquet`
  / `.../test-00000-of-00001.parquet`。单句二分类，标签 `0=non-hate`、`1=hate`。
- 原始 8,993 train / 2,970 test；清洗规则是去掉空文本、按 stripped text 去重、
  丢弃同文本标签冲突、移除 test 中与 train 精确重叠的文本。train 去掉 7 条空文本
  与 1 条标签冲突，test 去掉 232 条 train 重叠及 test 内重复后，得到
  **8,985 train / 2,734 test**；标签为 train 5,204/3,781、test 1,619/1,115。
  清洗统计见 `prepare-summary.json`，清洗后的 JSONL 是 `raw/train.jsonl` /
  `raw/test.jsonl`。
- test 多数类（全判 non-hate）基线：accuracy `0.592173`、macro-F1 `0.371927`。
- seq 64，mean pooling + L2 normalize，编码器冻结；L12/L6 各四档，与
  §11.12–§11.14 的量化预算相同（L12 `f32/int3/int4/R12`，L6
  `f32/int3/int4/rescan-k2`）。用关闭截断的 tokenizer 统计：train 4.5% 超过
  64 token（均值 31.8、p95 63），test 12.8% 超过 64 token（均值 36.6、p95 77、
  max 141）。
- 评测口径分两套，均只训练线性头，不做 MLP、不做 encoder 微调：
  1. **无权重口径**，与 SST-2 主口径一致：`class_weight="none"`、train 内
     5 折按 accuracy 选 C，主指标 accuracy；用于直接回答“SST-2 的 int4 正向
     是否在第三数据集复现”。
  2. **balanced 口径**，用于类别不均衡的稳健性：`class_weight="balanced"`、
     train 内 5 折按 macro-F1 选 C，同时报 accuracy 与 macro-F1。
- 两种线性头都跑：logistic regression 与平方 hinge `LinearSVC`；`C` 网格
  `{0.01,0.1,1,10}`，test 只评一次。10k test-level paired bootstrap 报差值、
  95% CI 与 `better_fraction`；另做 L12 balanced logreg 的 fixed `C=1`
  敏感性，消除“不同档位选到不同 C”的 confound。
- 编码按 1,500 条分块（11,719 → 8 块，前 7 块 1,500、尾块 1,219）；
  L12/L6 每个档位拼回 `WSPEMB1` 后均为 11,719 行。

### 11.15.2 L12 无权重口径（与 SST-2 同口径）

| probe | 配置 | accuracy | macro-F1 | AUC | selected C | CV accuracy |
|---|---|---:|---:|---:|---:|---:|
| logreg | f32 | 0.576445 | 0.567921 | 0.686456 | 1 | 0.771842 |
| logreg | int3 | 0.560717 | 0.549944 | 0.698220 | 1 | 0.777629 |
| logreg | int4 | 0.572787 | 0.564640 | 0.687451 | 1 | 0.769839 |
| logreg | R12 | 0.562546 | 0.551344 | 0.700598 | 1 | 0.779410 |
| linear_svc | f32 | 0.571324 | 0.562138 | 0.688636 | 0.1 | 0.772510 |
| linear_svc | int3 | 0.553767 | 0.541568 | 0.698113 | 0.1 | 0.777518 |
| linear_svc | int4 | 0.566569 | 0.557233 | 0.691178 | 1 | 0.770840 |
| linear_svc | R12 | 0.557791 | 0.545421 | 0.703384 | 0.1 | 0.778408 |

L12，probe 重新训练、10k paired bootstrap（下列均为“候选−f32”）：

- logreg `int4−f32 = −0.003679`，95% CI `[−0.012802,+0.005486]`，
  better `0.2304`；点估计略负，CI 跨 0。
- logreg `int3−f32 = −0.015737`，CI `[−0.028164,−0.003292]`；
  `R12−f32 = −0.013923`，CI `[−0.026701,−0.001463]`；两者都显著为负。
- SVC `int4−f32 = −0.004826`，CI `[−0.015362,+0.005852]`，better `0.1908`；
  `int3−f32 = −0.017598`（CI `[−0.029993,−0.004755]`），
  `R12−f32 = −0.013585`（CI `[−0.025969,−0.001454]`）。

AUC 在四档之间只有 `0.686–0.703` 的小幅差异，且没有做 AUC 的区间估计；
它不能把 int4 的 accuracy 负号翻成正号。无权重口径下的直接结论是：
**int4 没有复现 SST-2 的正向，点估计反而略低；`int3`/`R12` 明显低于 f32。**

### 11.15.3 L12 balanced 口径（macro-F1 选 C）

| probe | 配置 | accuracy | macro-F1 | AUC | selected C | CV macro-F1 |
|---|---|---:|---:|---:|---:|---:|
| logreg | f32 | 0.547549 | 0.530500 | 0.689610 | 10 | 0.765937 |
| logreg | int3 | 0.529627 | 0.505323 | 0.695445 | 10 | 0.768920 |
| logreg | int4 | 0.534748 | 0.513450 | 0.689071 | 1 | 0.765414 |
| logreg | R12 | 0.532187 | 0.508100 | 0.702542 | 1 | 0.774654 |
| linear_svc | f32 | 0.545721 | 0.528534 | 0.694737 | 1 | 0.766030 |
| linear_svc | int3 | 0.526335 | 0.500743 | 0.702604 | 0.1 | 0.768441 |
| linear_svc | int4 | 0.532553 | 0.510524 | 0.693180 | 0.1 | 0.764588 |
| linear_svc | R12 | 0.532187 | 0.508267 | 0.708809 | 0.1 | 0.775566 |

balanced 口径下，int4 对 f32 的点估计与 CI 全部转负：

- logreg：accuracy `int4−f32 = −0.012694`，CI `[−0.022677,−0.002917]`；
  macro-F1 `−0.016905`，CI `[−0.028280,−0.005419]`。
- SVC：accuracy `−0.013028`，CI `[−0.022312,−0.003292]`；
  macro-F1 `−0.017835`，CI `[−0.028703,−0.006747]`。
- `int3−f32` 与 `R12−f32` 也全部为负：logreg accuracy `−0.017922` /
  `−0.015281`，SVC `−0.019338` / `−0.013453`；对应的 macro-F1 也整段为负。

**选 C 不匹配的对照**：balanced logreg 的 f32/int3 选到上界 `C=10`，
int4/R12 选 `C=1`。为排除这个 confound，另跑 fixed `C=1`
（`eval-l12-balanced-c1.json`）：f32 `0.544623 / 0.525344`，int4
`0.534748 / 0.513450`；`int4−f32` accuracy `−0.009860`
（CI `[−0.017922,−0.002195]`），macro-F1 `−0.011855`
（CI `[−0.021066,−0.002938]`）。**负号不是选 C 造成的**；固定 C 后仍是约
−1 个百分点的显著负向。`int3−f32`（`−0.015424`）与 `R12−f32`
（`−0.012446`）在 fixed C=1 下也仍为负。

### 11.15.4 L6 balanced 口径

| probe | 配置 | accuracy | macro-F1 | AUC | selected C | CV macro-F1 |
|---|---|---:|---:|---:|---:|---:|
| logreg | f32 | 0.516459 | 0.490066 | 0.674528 | 1 | 0.761785 |
| logreg | int3 | 0.512070 | 0.483218 | 0.693341 | 1 | 0.759251 |
| logreg | int4 | 0.509876 | 0.482207 | 0.680805 | 1 | 0.757567 |
| logreg | rescan-k2 | 0.515362 | 0.486043 | 0.709857 | 1 | 0.767368 |
| linear_svc | f32 | 0.513168 | 0.485776 | 0.677936 | 0.1 | 0.760818 |
| linear_svc | int3 | 0.506218 | 0.476248 | 0.685253 | 1 | 0.759570 |
| linear_svc | int4 | 0.504023 | 0.476024 | 0.671679 | 1 | 0.757271 |
| linear_svc | rescan-k2 | 0.514265 | 0.484592 | 0.710314 | 0.1 | 0.768537 |

L6 的两个 probe 方向不一致，没有一致优势：

- logreg `int4−f32 = +0.006576`，CI `[−0.001463,+0.014265]`；macro-F1
  `+0.007854`，CI `[−0.001562,+0.017248]`；点估计为正但 CI 跨 0。
- SVC `int4−f32 = −0.009173`，CI `[−0.018288,−0.000366]`；macro-F1
  `−0.009786`，CI `[−0.020570,+0.000887]`；accuracy 的负向擦边显著。
- `rescan-k2−f32` 在 logreg/SVC 都约为 0（`+0.001127` / `+0.001069`），
  CI 全跨 0；L6 的块级重扫同样没有分类增益。

### 11.15.5 跨数据集对照与结论

L12 `int4−f32` 的跨数据集汇总（候选−基线；SST-2 无权重、CoLA/hate balanced
为各自主口径，hate 另列无权重同口径）：

| 数据集 / 口径 | test 条数 | logreg | 95% CI | SVC | 95% CI |
|---|---:|---:|---|---:|---|
| SST-2，无权重 | 1,821 | +0.040034 | `[+0.020868,+0.058759]` | +0.043783 | `[+0.025261,+0.062603]` |
| CoLA，balanced | 1,043 | +0.013341 | `[−0.006711,+0.033557]` | +0.008467 | `[−0.011505,+0.028763]` |
| Hate，无权重 | 2,734 | −0.003679 | `[−0.012802,+0.005486]` | −0.004826 | `[−0.015362,+0.005852]` |
| Hate，balanced | 2,734 | −0.012694 | `[−0.022677,−0.002917]` | −0.013028 | `[−0.022312,−0.003292]` |

- **SST-2 的 int4 正向没有跨任务/跨数据集复现。** 第三个数据集上，无权重口径
  点估计略负且 CI 跨 0；balanced 口径（以及 fixed C=1）整段为负。hate 的
  test 有 2,734 条，无权重口径 CI 上界只有 `+0.005486`，比 CoLA 的
  `+0.033557` 更严格地把 SST-2 的 `+0.040/+0.044` 排除在外。
- **结论强度**：三个数据集（SST-2 电影评论、CoLA 语法判断、TweetEval/hate
  社媒仇恨言论）覆盖不同任务/领域，SST-2 的正向只在 SST-2 出现；因此它应被
  写成数据集/任务/probe 家族特定的观察，而不是“int4 普遍提升线性分类”。
- **`R12`/`rescan-k2` 仍无任务头增益。** L12 `R12` 在这个数据集上不仅低于
  int4，也显著低于 f32；L6 `rescan-k2` 与 f32 打平。STS 上的块级重排收益
  没有转成分类收益的结论继续成立。
- **probe 很弱，不能把“没有正向”写成“已证不存在任何正向”。** hate 的
  raw accuracy 只有 `0.51–0.58`，低于多数类 `0.592173`；macro-F1 `0.48–0.57`
  高于全判 non-hate 的 `0.371927`；AUC 约 `0.67–0.71`。这说明线性头能分开
  两类但阈值/间隔很差，绝对 accuracy 不能当任务 SOTA，只能做同一 test 上的
  配对比较。2,734 条 test 足以排除 +4 个百分点量级的效应，不能排除更小的、
  数据集/正则化特定的正向。
- 边界：单 split、单 seq 64（test 12.8% 被截断）、两个 MiniLM、只训练线性头、
  无 MLP、无 encoder 微调；hate 标签本身噪声/主观性高；清洗后的 test 移除了
  232 条与 train 重叠的文本，不能直接等同官方榜单读数；结论不能外推到平衡/
  多分类/NLI/生成任务。

> **M1.38 追加（口径与跨域）**：本节主表是 per-arm C；fixed C=1 只是敏感性，
> 不能与 M1.31 的 per-arm CV 表或 M1.35 的 matched-C 表直接拼。Hate 的 test
> 还是跨域读数（域分类器 AUC `0.921`、长度单特征 `0.561`；20pp 里约
> `6–9pp` 是目标域阈值漂移，校准后仍差 `12–13pp`），所以本节数字只能当
> “历史 per-arm C + 默认 0.5 阈值 + 跨域 test”的参考，不能当 in-domain
> 量化损失上界。M1.38 的 target_prior 全臂复验见 §11.38。

### 11.15.6 复算

```bash
# 1) 下载原始 parquet
curl -L -o hate-train.parquet \
  https://huggingface.co/datasets/tweet_eval/resolve/main/hate/train-00000-of-00001.parquet
curl -L -o hate-test.parquet \
  https://huggingface.co/datasets/tweet_eval/resolve/main/hate/test-00000-of-00001.parquet

# 2) 清洗并写 JSONL：去空文本、按 stripped text 去重、丢弃标签冲突、
#    移除 test 中与 train 重叠的文本；同时输出 prepare-summary.json
python - <<'PY'
import json
from pathlib import Path
import pandas as pd

out = Path("raw"); out.mkdir(parents=True, exist_ok=True)
train = pd.read_parquet("hate-train.parquet")[["text", "label"]]
test = pd.read_parquet("hate-test.parquet")[["text", "label"]]
for frame in (train, test):
    frame["text"] = frame["text"].fillna("").astype(str).str.strip()
train = train[train["text"] != ""]
test = test[test["text"] != ""]

conflict = train.groupby("text")["label"].nunique()
conflict = set(conflict[conflict > 1].index)
train = train[~train["text"].isin(conflict)].drop_duplicates("text")
test = test.drop_duplicates("text")
test = test[~test["text"].isin(set(train["text"]))]

for name, frame in (("train", train), ("test", test)):
    with (out / f"{name}.jsonl").open("w", encoding="utf-8") as handle:
        for row in frame.itertuples(index=False):
            handle.write(json.dumps({"text": row.text, "label": int(row.label)},
                                    ensure_ascii=False) + "\n")
summary = {
    "dataset": "tweet_eval/hate",
    "label_names": {"0": "non-hate", "1": "hate"},
    "raw": {"train": 8993, "test": 2970},
    "drop_label_conflicts": {"train": len(conflict), "test": 0},
    "drop_test_train_overlap": 232,
    "clean": {"train": len(train), "test": len(test)},
    "labels": {
        name: {str(k): int(v) for k, v in frame["label"].value_counts().items()}
        for name, frame in (("train", train), ("test", test))
    },
    "note": "empty text dropped; exact stripped-text duplicates deduped; "
            "conflicting duplicate labels dropped; test texts present in train removed",
}
(out / "prepare-summary.json").write_text(
    json.dumps(summary, indent=2, ensure_ascii=False), encoding="utf-8")
PY

# 3) 导出：manifest 的 dataset/labels 由 --dataset-name/--label-names 覆盖
python scripts/wsp_export_sst2.py \
  --format jsonl --train raw/train.jsonl --test raw/test.jsonl \
  --dataset-name 'TweetEval/hate (dedup/leak-filtered)' \
  --label-names non-hate,hate \
  --out data-l12 --model all-MiniLM-L12-v2 --seq 64
python scripts/wsp_export_sst2.py \
  --format jsonl --train raw/train.jsonl --test raw/test.jsonl \
  --dataset-name 'TweetEval/hate (dedup/leak-filtered)' \
  --label-names non-hate,hate \
  --out data-l6 --model all-MiniLM-L6-v2 --seq 64

# 4) 1,500 条分块；编码前先把全量 texts.json 另存为 texts-l12.json/texts-l6.json
python scripts/wsp_chunk_embed_input.py \
  --src data-l12 --dst chunks-l12-1500 --chunk-size 1500
python scripts/wsp_chunk_embed_input.py \
  --src data-l6 --dst chunks-l6-1500 --chunk-size 1500

# 5) 编码：以 L12 int3 chunk-000 为例；f32 由每次运行的 pure-f32.f32 另存
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l12-pure-int3.json \
TRIT_THREADS=16 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 \
TRIT_EMBED_DUMP=emb12 TRIT_EMBED_TAG=l12-int3-000 \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir chunks-l12-1500/chunk-000 --activation-mode f32 \
  --out runs12/l12-int3-000.json

# 6) 无权重主口径（与 SST-2 一致）
python scripts/wsp_classify_eval.py \
  --texts texts-l12.json \
  --run l12-f32='emb12/l12-pure-f32-*.f32' \
  --run l12-int3='emb12/l12-int3-*.f32' \
  --run l12-int4='emb12/l12-int4-*.f32' \
  --run l12-r12='emb12/l12-r12-*.f32' \
  --group 'l12=l12-f32|l12-int3,l12-int4,l12-r12' \
  --cv-C --bootstrap 10000 --probe logreg --out eval-l12-cv.json

# 7) balanced 口径（macro-F1 选 C，accuracy/macro-F1 双 bootstrap）
python scripts/wsp_classify_eval.py \
  --texts texts-l12.json \
  --run l12-f32='emb12/l12-pure-f32-*.f32' \
  --run l12-int3='emb12/l12-int3-*.f32' \
  --run l12-int4='emb12/l12-int4-*.f32' \
  --run l12-r12='emb12/l12-r12-*.f32' \
  --group 'l12=l12-f32|l12-int3,l12-int4,l12-r12' \
  --cv-C --cv-metric macro_f1 --class-weight balanced \
  --bootstrap 10000 --bootstrap-metric both \
  --probe logreg --out eval-l12-balanced-cv.json
```

L6 把前缀换成 `l6`、budget 换成 `budget-l6-pure-int3.json`/`l6-rescan-k2.json`，
`--probe` 换成 `linear_svc` 即得另一套。表见
`data/wsp-classify-hate-l12.tsv`（8 行）、
`data/wsp-classify-hate-l12-bootstrap.tsv`（24 行）、
`data/wsp-classify-hate-l12-balanced.tsv`（8 行）、
`data/wsp-classify-hate-l12-balanced-bootstrap.tsv`（48 行）、
`data/wsp-classify-hate-l12-probe-compare.tsv`（4 行）、
`data/wsp-classify-hate-l12-c1.tsv`（4 行）、
`data/wsp-classify-hate-l6-balanced.tsv`（8 行）、
`data/wsp-classify-hate-l6-balanced-bootstrap.tsv`（48 行）、
`data/wsp-classify-hate-l6-probe-compare.tsv`（4 行）。

## 11.16 SST-2 内部子样本稳定性（M1.16）

### 11.16.1 口径

- 固定 SST-2 官方 test（1,821 条），只在 train（6,920 条）上重复做
  **分层 5 折 × 5 个种子**（seed `7–11`），共 **25 次重训**。每次用 4/5
  的 train（5,536 条）训练线性 probe，在完整 test 上评一次。
- 为隔离“probe 训练样本变化”，**C 固定为全量 train 选出的值**，不在每个
  子样本上重新选 C。L12 logreg：`f32=10 / int3=1 / int4=10 / R12=1`；
  L12 SVC：`1 / 10 / 0.1 / 0.1`；L6 logreg：`10 / 10 / 1 / 1`；
  L6 SVC：`10 / 1 / 0.1 / 0.1`。
- 每个 replicate 记录 4 个配置在完整 test 上的 accuracy/macro-F1，并计算
  `candidate − f32`；汇总 25 个差值的均值、标准差、最小/最大值和正负计数。
  这里问的是“换一批训练样本，SST-2 上的排序是否还在”，不是新的独立
  test 估计，也不是对 test 的重采样。
- 实现：`scripts/wsp_classify_subsample.py`；原始结果
  `subsample-l12-logreg.json`、`subsample-l12-svc.json`、
  `subsample-l6-logreg.json`、`subsample-l6-svc.json`。

### 11.16.2 L12：int4 正向在 25/25 次重训中保持

| probe | 候选 | 全量 test Δ | 子样本均值 | sd | min | max | 正/25 |
|---|---|---:|---:|---:|---:|---:|---:|
| logreg | int3 | +0.023613 | +0.024997 | 0.005001 | +0.013729 | +0.036244 | 25 |
| logreg | int4 | +0.040088 | +0.042987 | 0.005372 | +0.028556 | +0.050522 | 25 |
| logreg | R12 | +0.014827 | +0.019001 | 0.004605 | +0.009336 | +0.030203 | 25 |
| linear_svc | int3 | +0.020868 | +0.020297 | 0.003529 | +0.013729 | +0.028007 | 25 |
| linear_svc | int4 | +0.043932 | +0.044437 | 0.004632 | +0.034596 | +0.052718 | 25 |
| linear_svc | R12 | +0.016474 | +0.020319 | 0.004668 | +0.010983 | +0.034596 | 25 |

- L12 logreg 的 `int4−f32`：全量 test `+0.040088`，25 次子样本均值
  `+0.042987`，**25/25 为正**，最差一次仍有 `+0.028556`。
- L12 SVC 的 `int4−f32`：全量 `+0.043932`，子样本均值 `+0.044437`，
  **25/25 为正**，最差 `+0.034596`。
- 两种头的子样本均值与全量 test 只差约 `0.003`，方向完全一致；`int3`/`R12`
  也都在 25/25 次中为正，但幅度小于 int4。**SST-2 的 int4 正向不是某一次
  train 划分的偶然。**

### 11.16.3 L6：有小幅正向，但不稳定

| probe | 候选 | 全量 test Δ | 子样本均值 | sd | min | max | 正/负/零 |
|---|---|---:|---:|---:|---:|---:|---:|
| logreg | int3 | +0.010983 | +0.009665 | 0.004182 | −0.000549 | +0.018671 | 24/1/0 |
| logreg | int4 | +0.006041 | +0.006304 | 0.004479 | −0.002746 | +0.014827 | 22/2/1 |
| logreg | rescan-k2 | +0.003844 | +0.002724 | 0.004577 | −0.005491 | +0.011532 | 16/9/0 |
| linear_svc | int3 | +0.007139 | +0.007249 | 0.004367 | −0.002197 | +0.014827 | 23/2/0 |
| linear_svc | int4 | +0.002746 | +0.003580 | 0.005268 | −0.010983 | +0.012630 | 19/6/0 |
| linear_svc | rescan-k2 | +0.000549 | −0.000615 | 0.003865 | −0.008237 | +0.008786 | 10/14/1 |

L6 的 int4 均值只有 `+0.0036–0.0063`，且分别有 2/25、6/25 次为负；
`rescan-k2` 的 logreg/SVC 均值接近 0，SVC 甚至有 14/25 为负。**L6 的量化
正向即使存在，也远小于 L12，且不满足“每次重训同号”。**

### 11.16.4 与跨数据集结论的关系

- 这条实验把 SST-2 的观察从“单 split 的点估计”升级为“**训练子样本稳定的
  L12 表示效应**”：在 25 次不同的 80% train 子样本上，L12 int4 对 f32 的
  accuracy 差值全部为正，且均值与官方 test 的 `+0.040/+0.044` 一致。
- 但它仍**不是量化普遍提升线性分类**：同样的 int4 在 CoLA 上只有
  `+0.013/+0.008`（CI 跨 0），在 TweetEval/hate 上是 `−0.0037/−0.0048`
  （无权重）或 `−0.0127/−0.0130`（balanced，CI 全负）；L6 上也只有
  `+0.0036–0.0063` 且不稳定。**“数据集内稳定”与“跨数据集泛化”是两件事，
  不能用一个替代另一个。**
- 也可以反过来读：SST-2 的正向既然在训练子样本上稳定，就不能用“训练噪声”
  解释掉；更合理的描述是 L12 的 int4 表示在 SST-2 这个任务/分布上有一个
  稳定的线性头优势，但这个优势没有随任务/数据集迁移。
- 边界：C 固定自全量 train，只测训练样本变化，不测 C 选择或 test 重采样；
  5 折的训练集重叠，25 个差值不独立，正负计数只作描述，不做二项检验；
  只做 SST-2、两个 MiniLM、两个线性头，无 MLP、无 encoder 微调。这个实验
  不能证明机制（例如量化正则化、边界间隔、特征各向异性），只证明稳定性。

### 11.16.5 复算

```bash
# L12 logreg：固定 test，5 折 × 5 seed = 25 次重训
python scripts/wsp_classify_subsample.py \
  --texts sst2/data-l12/texts.json \
  --run l12-f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run l12-int3='sst2/emb12/l12-int3-*.f32' \
  --run l12-int4='sst2/emb12/l12-int4-*.f32' \
  --run l12-r12='sst2/emb12/l12-r12-*.f32' \
  --baseline l12-f32 \
  --C l12-f32=10 --C l12-int3=1 --C l12-int4=10 --C l12-r12=1 \
  --probe logreg --folds 5 --repeats 5 --seed 7 \
  --out sst2/subsample-l12-logreg.json

# L12 SVC 把 --probe 换成 linear_svc，C 换成 1/10/0.1/0.1
# L6 把前缀换成 l6、budget 换成 rescan-k2，C 换成上表所列
```

表见 `data/wsp-classify-sst2-subsample.tsv`（24 行，accuracy/macro-F1 ×
4 模型-probe × 3 候选）与
`data/wsp-classify-sst2-subsample-runs.tsv`（16 行，4 配置的均值/极值）。

## 11.17 几何诊断与 PCA 干预（M1.17）

### 11.17.1 问题与口径

M1.16 把 SST-2 的 L12 int4 正向钉成“训练子样本稳定”，但没有回答为什么
只在 SST-2 发生、CoLA/hate 不复现。这里检查三个候选解释：

1. 样本范数：量化改变了向量的长度，线性头只是被尺度带着走；
2. 协方差谱/各向异性：量化压平了少数主方向，线性头因此更好切；
3. 类别方向集中度：量化把类均值差方向挤进少数主成分，任务信号变成低秩。

口径：

- 数据与 §11.12–§11.16 同一批 `WSPEMB1`，seq 64、mask mean pooling + L2
  normalize；所有 run 的逐行 `mean_norm` 都是 `1.000000`，**样本范数这条
  在数据层面已经不成立**。
- 几何量：`sep`（类间距离/类内散度）、`fisher`、`centroid_cos`（两类质心
  余弦，越低越分开）、`centroid_norm`（质心范数）、`eff_rank`（协方差
  有效秩）、`top1_share`，以及 `signal_topK`（类均值差方向落在前 K 个
  主成分的平方能量占比）。train/test 各自算散度和有效秩；test 的
  `signal_topK` 复用 train 拟合的主成分，不用 test 自己拟合 PCA。
- PCA 干预：在 train 上拟合 PCA，test 只做 transform；`keepN` 把第 N 个
  之后的主成分置零，`whiten` 除以 `sqrt(eigenvalue)`；probe 仍是 logistic
  线性头，train 内 5 折选 C，test 只评一次。SST-2/hate 用无权重 accuracy，
  CoLA 用 balanced + macro-F1 选 C（无权重会塌到 0.691 的多数类）。
- 10k test-level paired bootstrap；PCA 是线性变换，叠加线性 probe 仍是线性
  头，没有 MLP、没有 encoder 微调。

### 11.17.2 几何读数：只有 SST-2 发生类别方向低秩化

L12、train split 的关键读数（完整 48 行见
`data/wsp-classify-geometry.tsv`）：

| 数据集 | run | sep | centroid_cos | eff_rank | top1_share | signal_top4 |
|---|---|---:|---:|---:|---:|---:|
| SST-2 | f32 | 0.154708 | 0.994163 | 77.636 | 0.118231 | 0.404870 |
| SST-2 | int4 | 0.199277 | 0.942967 | 200.093 | 0.044142 | 0.855378 |
| CoLA | f32 | 0.046371 | 0.993718 | 187.068 | 0.040238 | 0.166043 |
| CoLA | int4 | 0.043392 | 0.996032 | 183.190 | 0.042468 | 0.161427 |
| Hate | f32 | 0.168854 | 0.940824 | 164.700 | 0.093571 | 0.475858 |
| Hate | int4 | 0.166660 | 0.955907 | 158.151 | 0.097598 | 0.480608 |

- **SST-2**：int4 把 `signal_top4` 从 `0.404870` 提到 `0.855378`，把
  `centroid_cos` 从 `0.994163` 压到 `0.942967`，`eff_rank` 从 `77.636`
  提到 `200.093`，`top1_share` 从 `0.118231` 降到 `0.044142`。test 上
  同向：`signal_top4 0.421652 → 0.820898`，`sep 0.169130 → 0.209133`。
- **CoLA**：int4 的 `signal_top4 0.166043 → 0.161427`（略降），
  `eff_rank 187.068 → 183.190`，`sep 0.046371 → 0.043392`。test 上
  `signal_top4 0.213239 → 0.163653`，也没有集中。
- **Hate**：int4 的 `signal_top4 0.475858 → 0.480608`（基本不动），
  `eff_rank 164.700 → 158.151`，`sep 0.168854 → 0.166660`。test 上
  `signal_top4 0.215673 → 0.217194`。
- L6 有同向但更小的集中：SST-2 `signal_top4 0.430587 → 0.604095`、
  `eff_rank 89.097 → 189.811`；CoLA `0.166869 → 0.177379`、`eff_rank`
  略降；Hate `0.457703 → 0.457597`、`eff_rank` 从 `160.295` 降到
  `152.871`。L6 的 SST-2 集中度只有 L12 的一半左右，和 M1.16 里 L6
  正向更弱一致。

一句话：SST-2 的 f32 本身有一个很强的共同方向（`eff_rank` 只有 77.6，
两类质心几乎同向 `0.994`），int4 把这个共同方向压平，同时把类别方向
挤进前几个主成分；CoLA/hate 的 int4 没有做这件事，反而略微更各向异性。

### 11.17.3 PCA 干预：SST-2 的增益跟着低秩类别方向走

下表是 `int4−f32` 的 accuracy 差值（L12/L6，完整 60 行含 CI 见
`data/wsp-classify-pca.tsv`）：

| 数据集 | 模型 | raw | keep4 | keep16 | keep64 | whiten |
|---|---|---:|---:|---:|---:|---:|
| SST-2 | L12 | +0.040088 | +0.166392 | +0.125755 | +0.081274 | +0.049423 |
| SST-2 | L6 | +0.006590 | +0.089511 | +0.070840 | +0.042284 | +0.005491 |
| CoLA | L12 | +0.011505 | −0.004794 | −0.008629 | +0.010547 | −0.001918 |
| CoLA | L6 | −0.008629 | +0.006711 | +0.000959 | +0.001918 | −0.005753 |
| Hate | L12 | −0.003658 | −0.008778 | +0.005852 | −0.009510 | −0.002195 |
| Hate | L6 | −0.014631 | +0.004389 | +0.009144 | +0.003658 | +0.002926 |

- **SST-2 L12 的 keep4 差值是 `+0.166392`，CI `[+0.138935,+0.194399]`**：
  f32 只留 4 维时掉到 `0.602965`，int4 还有 `0.769357`；raw 全维只差
  `+0.040088`。也就是说，int4 的优势在“只给 4 个主成分”时反而放大，
  因为它的类别信号本来就集中在这几个方向；f32 的类别信号散在更多维，
  砍到 4 维就崩。
- `keep16`/`keep64` 同向，差值 `+0.125755` / `+0.081274`，CI 都全正；
  维数越放开，差值越回到 raw 的 `+0.040`。
- **白化不消掉差值**：`whiten` 后 f32 `0.789127`、int4 `0.838550`，
  差值 `+0.049423`，CI `[+0.028556,+0.070291]`。如果只是“协方差谱更平”，
  白化应该把两档拉平；实际没有，说明二阶谱不是唯一原因，类别方向
  的集中度才是跟着 probe 增益走的量。
- **SST-2 L6**：`keep4` 差值 `+0.089511`（CI 全正），但 raw 只有
  `+0.006590`、CI `[-0.013193,+0.026359]`；`whiten` 也只有 `+0.005491`、
  CI 跨 0。L6 的类别方向有一些集中，但不足以在全维产生稳定优势。
- **CoLA**：balanced 口径 raw `+0.011505`、CI 跨 0；`keep4`/`keep16`
  转负，`keep64` `+0.010547`、CI 仍跨 0。CoLA 的句法标签本来就不在
  MiniLM 的前几个主成分里，砍维只会掉信息。
- **Hate**：raw L12 `-0.003658`、L6 `-0.014631`（后者 CI 全负）；
  `keep4`/`keep16`/`keep64` 的 L12 差值都在 0 附近且 CI 跨 0，L6 的
  `keep16 +0.009144` 也只是 CI 跨 0 的小点估计。hate 的词汇/主题信号
  同样不在前几个主成分里。

### 11.17.4 结论与边界

- **SST-2 的 L12 int4 正向可以描述成“数据集特异的类别方向低秩集中”**：
  int4 把类均值差方向挤进前 4 个主成分（train `0.405 → 0.855`，
  test `0.422 → 0.821`），同时压平 f32 的共同方向（`eff_rank 77.6 → 200.1`）。
  这解释了为什么只有 SST-2 的线性 probe 明显受益：它的任务信号在量化后
  被挤进少数主方差方向（f32 只有 `0.405`，int4 到 `0.855`），而 CoLA/hate
  的同一比例不随量化改变；这是与读数一致的描述，不是已证的因果链。
- **这不是样本范数**：所有 WSPEMB1 行都是单位范数；也**不只是协方差谱**：
  白化后 SST-2 的差值仍在（`+0.049`，CI 全正）。几何诊断把“尺度/各向异性”
  这两个最容易想到的解释排除掉，留下“类别方向集中度”这条。
- **但这不是因果证明**。PCA keep/whiten 是对固定 embedding 的线性干预，
  不是对量化过程本身做消融；没有证明 int4 的哪一步（absmean、scale、
  per-group）造成了集中，也没有做 MLP 或 encoder 微调（当前口径排除）。
  结论只能写到“与读数一致的机制描述”，不能写“量化通过压平共同方向
  普遍提升分类”。
- 边界：三个单句二分类集、两个 MiniLM、seq 64、一个 logreg、一个 seed、
  train 内固定 C 网格；CoLA 用 balanced + macro-F1（无权重会塌到多数类），
  SST-2/hate 用无权重；没有 NLI/MNLI、没有第四数据集、没有 test 重采样。
  `signal_topK` 用 train 主成分投 test，仍是描述性读数，不是 test 上的
  模型选择。

### 11.17.5 复算

```bash
# 几何读数：每个数据集/深度跑一次
python scripts/wsp_classify_geometry.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int3='sst2/emb12/l12-int3-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --run alt='sst2/emb12/l12-r12-*.f32' \
  --out sst2/geometry-l12.json

# PCA 干预：SST-2 无权重；CoLA 换成 balanced + macro-F1
python scripts/wsp_classify_pca.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --baseline f32 --modes raw,keep4,keep16,keep64,whiten \
  --bootstrap 1 --bootstrap-iters 10000 --out sst2/pca-l12.json

# CoLA：追加 --class-weight balanced --cv-metric macro_f1
```

表见 `data/wsp-classify-geometry.tsv`（48 行：6 个数据集-深度 × 4 档 ×
2 个 split）与 `data/wsp-classify-pca.tsv`（60 行：6 个数据集-深度 ×
5 个 mode × 2 档）。

## 11.18 跨档 PCA 子空间迁移（M1.18）

### 11.18.1 问题与口径

M1.17 的读数是“SST-2 的 int4 把类别方向挤进前几个主成分”。但这只是相关：
如果正向来自一个更好的低秩子空间，那么把 int4 的主成分基搬到 f32 上，应该
能复现一部分；如果搬不过去，说明优势在子空间内的坐标/尺度，而不只是基。

`wsp_classify_pca.py` 新增 `--pca-fit NAME`：PCA 在指定 run 的 train split
上拟合，所有 run 的 train/test 都投影到同一组主成分；probe 仍只在各自
train 上重训。两个方向：

- `fit=int4`：f32 投到 int4 基，int4 是自投影；
- `fit=f32`：int4 投到 f32 基，f32 是自投影。

模式 `raw / keep4 / keep8 / keep16 / keep64`；10k test-level paired
bootstrap 只算 accuracy，macro-F1 只报点估计；CoLA 用 balanced +
macro-F1 选 C，SST-2/hate 用无权重；仍是冻结编码器、单一 logistic 头、
seq 64、一个 seed。

### 11.18.2 SST-2 L12：基搬过去了，增益没有搬过去

| fit | mode | f32 acc | f32−raw | int4 acc | int4−f32 | 95% CI |
|---|---:|---:|---:|---:|---:|---|
| int4 | raw | 0.800110 | 0 | 0.840198 | +0.040088 | `[+0.020868,+0.058759]` |
| int4 | keep4 | 0.694124 | −0.106016 | 0.769357 | +0.075233 | `[+0.051620,+0.098298]` |
| int4 | keep8 | 0.709500 | −0.090650 | 0.775398 | +0.055898 | `[+0.042284,+0.088962]` |
| int4 | keep16 | 0.722131 | −0.078136 | 0.805601 | +0.083470 | `[+0.062054,+0.104887]` |
| int4 | keep64 | 0.753981 | −0.046242 | 0.826469 | +0.072488 | `[+0.052169,+0.093355]` |
| f32 | keep4 | 0.602965 | −0.197481 | 0.636463 | +0.033498 | `[+0.011532,+0.055464]` |
| f32 | keep8 | 0.628775 | −0.171545 | 0.672707 | +0.043932 | `[+0.021966,+0.066447]` |
| f32 | keep16 | 0.679846 | −0.120380 | 0.693575 | +0.013729 | `[−0.008237,+0.035146]` |
| f32 | keep64 | 0.745195 | −0.055016 | 0.792971 | +0.047776 | `[+0.026908,+0.068644]` |

- **int4 的基确实含类别方向**：f32 投到 int4 基后，`keep4` 从 `0.603`
  回到 `0.694`，`keep16` 从 `0.680` 回到 `0.722`；比自己拟合的 f32 基
  （`0.603` / `0.680`）好一截。
- **但 int4 自己没有跟着降**：同一个 `keep4` 下 int4 还有 `0.769`，
  `int4−f32` 是 `+0.075233`，比 raw 的 `+0.040088` 更大；`keep16`
  更是 `+0.083470`。也就是说，把两者压到同一个低秩基之后，差距没有被
  抹平，反而放大了。**低秩基本身不是增益的充分条件。**
- **反方向也不成立**：把 int4 投到 f32 的基上，`keep4` 仍高 `+0.033498`、
  `keep8` `+0.043932`、`keep64` `+0.047776`；只有 `keep16` 的
  `+0.013729` CI 跨 0。int4 的优势也不依赖“自己那套基”。
- 因此更准确的描述是：**优势跟着 int4 自己的坐标/尺度走**，而不是
  跟着某个可搬运的主成分基走。M1.17 的低秩集中是伴随读数，不是充分原因；
  换基只能解释 f32 掉点的一部分，不能解释 int4 为什么更高。M1.19 进一步
  把这条读数定位到类均值方向本身旋转（§11.19），不是同一方向在不同基里的
  坐标变化。

### 11.18.3 L6 与 CoLA/hate

- SST-2 L6 raw `int4−f32 = +0.006590`、CI 跨 0；`fit=int4` 下
  `keep4 +0.028583`、`keep16 +0.038890`、`keep64 +0.042180`，CI 都过 0，
  但这是“两者都被砍维”后的条件差值；L6 raw 本身仍不是稳定正向，不能
  把 `keepN` 的条件差值写成 L6 的增益。
- CoLA balanced：两个 fit 方向、所有 mode 的 `int4−f32` CI 都跨 0；
  `keep4` 甚至转负（fit int4 `−0.008652`、fit f32 `−0.000947`）。
- Hate：raw L12 `−0.003679`、L6 `−0.014631`（后者 CI 全负）；两个 fit
  方向下绝大多数 mode CI 跨 0。`fit=f32` 的 hate L12 `keep8` 出现
  `+0.009829`、CI `[+0.001463,+0.018288]`，但同一数据 `fit=int4` 的
  `keep8` 只有 `+0.003939`、CI 跨 0；这是多格子里的单点，不作为稳定正向。

### 11.18.4 结论与边界

- **SST-2 的 L12 int4 正向不能由“更好的低秩子空间”单独解释。** 把 int4
  的主成分基搬到 f32 能救回 f32 的一部分掉点，但 int4 在同维数下仍明显
  更高，差距从 raw `+0.040` 扩大到 `+0.075/+0.084`；反向把 int4 投到
  f32 基也没有消掉优势。低秩集中是描述性伴随现象，真正跟着 probe 增益
  走的是 int4 自己的坐标/尺度，而不是基的编号；M1.19 进一步表明这个
  差异主要是类均值方向本身旋转，而不是同一方向在另一组基里的系数。
- 这仍不是量化过程的因果消融：PCA 是固定 embedding 的线性变换，两个
  run 的基互相搬运不改变量化算子；没有拆 absmean/scale/per-group 哪一步
  造成坐标差异，也没有测 SVC、MLP 或 encoder 微调（当前口径排除）。
- 边界：三个单句二分类集、两个 MiniLM、seq 64、一个 logreg、一个 seed；
  bootstrap 只算 accuracy（10k），macro-F1 是点估计；CoLA balanced +
  macro-F1 选 C，SST-2/hate 无权重。不能外推成“换基普遍提升分类”。

### 11.18.5 复算

```bash
# int4 基投 f32；反方向把 --pca-fit 换成 f32
python scripts/wsp_classify_pca.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --baseline f32 --pca-fit int4 \
  --modes raw,keep4,keep8,keep16,keep64 \
  --bootstrap 1 --bootstrap-iters 10000 --bootstrap-metrics accuracy \
  --bootstrap-raw --out sst2/pca-transfer-l12-fitint4.json
```

完整 120 行在 `data/wsp-classify-pca-transfer.tsv`（6 个数据集-深度 ×
2 个 fit 方向 × 5 个 mode × 2 个 run）。

## 11.19 跨档类方向旋转（M1.19）

### 11.19.1 问题与口径

M1.18 的负迁移说明“低秩基”不是可搬运机制，但还没有回答：是基选错了，
还是两档的类别方向根本不同？M1.17 的 `signal_topK` 是各档在**自己**的
PCA 基里算的，两个百分比不能直接跨档比较；需要把一档的方向投到另一档的
基里看。

M1.19 直接比较两档的类均值差方向：

```text
d = mean(embedding | y=1) - mean(embedding | y=0)
```

`wsp_classify_direction.py` 在每档的 train split 上拟合 PCA 基（test 只
投影，不重拟合），再算：

- `cos` / `angle`：两档 `d` 的余弦与夹角；
- `norm_ratio`：`||d_int4|| / ||d_f32||`；
- `own_topK`：`d` 落在**自己**前 K 个主成分的能量占比；
- `cross_topK`：`d` 落在**对方**前 K 个主成分的能量占比。

这是纯诊断，不训练 probe、不做 bootstrap、不加数据集；train 和 test
分开报，test 方向也只用于诊断，不参与任何选参。

### 11.19.2 SST-2 的方向转了，CoLA/hate 没转

| 数据集 | 深度 | cos train/test | 夹角 train/test | `‖d_int4‖/‖d_f32‖` | f32 top4 自基 | f32 top4 在 int4 基 | int4 top4 自基 | int4 top4 在 f32 基 |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| SST-2 | L12 | 0.5715 / 0.5891 | 55.14° / 53.90° | 1.938 | 0.4049 | 0.2781 | 0.8554 | 0.0716 |
| SST-2 | L6 | 0.6201 / 0.6278 | 51.67° / 51.11° | 1.535 | 0.4306 | 0.2148 | 0.6041 | 0.0837 |
| CoLA | L12 | 0.9701 / 0.9731 | 14.04° / 13.33° | 0.909 | 0.1660 | 0.1493 | 0.1614 | 0.1718 |
| CoLA | L6 | 0.9730 / 0.9732 | 13.34° / 13.29° | 0.948 | 0.1669 | 0.1611 | 0.1774 | 0.1705 |
| TweetEval-hate | L12 | 0.9757 / 0.9785 | 12.65° / 11.89° | 0.957 | 0.4759 | 0.4494 | 0.4806 | 0.4591 |
| TweetEval-hate | L6 | 0.9698 / 0.9747 | 14.12° / 12.92° | 0.982 | 0.4577 | 0.4221 | 0.4576 | 0.4456 |

- **SST-2 的两档类方向明显分离**：L12 夹角 `55.1°`（test
  `53.9°`），L6 `51.7°`（test `51.1°`）；int4 的方向还更长，
  `||d_int4||/||d_f32|| = 1.94`（L6 `1.54`）。
- **CoLA/hate 的两档类方向几乎同向**：cos 都在 `0.970–0.976`，夹角
  `12.6–14.1°`，范数比 `0.91–0.98`。量化没有把这两个数据集的类方向
  转走，也没有把它放大。
- 方向夹角的 train/test 读数很接近，说明这不是某一次划分的偶然；但这是
  点估计，没有给方向夹角做 bootstrap CI。

### 11.19.3 为什么 M1.18 的基搬不过去

- 在 SST-2 L12，f32 的类方向只有 `27.8%` 落在 int4 的前 4 个主成分里
  （top16 也只有 `41.1%`），而 int4 自己的方向有 `85.5%` 落在同一组前 4
  个主成分里（top16 `95.6%`）。反过来，int4 的方向只有 `7.2%` 落在 f32
  的前 4 个主成分里（top16 `17.7%`），f32 自己的方向是 `40.5%`。
- 因此 M1.18 的 `fit=int4 keep4` 实际上是把 f32 投到一个**不包含 f32
  类方向**的低秩基：f32 丢掉了大部分类别信号（`0.603`），int4 却保留了
  自己方向的大部分（`0.769`）。`fit=f32` 也一样：int4 的方向大部分不在
  f32 前 4 维里，两个 run 都被砍掉信号，搬基不能把优势“复制”过去。
- M1.17 的 `signal_top4 0.855` 是 int4 在**自己旋转后的方向**上的集中度，
  不是“存在一个两档共享的 4 维类别子空间”。低秩集中是 int4 方向的属性，
  不是可搬运的公共结构；M1.18 的“坐标/尺度”读数应更准确地写成
  **类方向本身旋转**。
- CoLA/hate 的 `cross_top4` 与 `own_top4` 很接近（例如 hate L12：
  f32 `0.449` vs 自基 `0.476`，int4 `0.459` vs 自基 `0.481`），说明两档
  方向基本共享；这也和它们没有 int4 正向一致。三数据集对照把“方向旋转”
  与“SST-2 特异正向”对上了，但仍是观测性关联，不是量化算子的因果证明。

### 11.19.4 结论与边界

- **SST-2 的 L12 int4 正向对应的是“类方向旋转 + 方向变长”，不是
  “更好的共享低秩基”。** f32 的类方向在 int4 基的前 4 维里只剩 `27.8%`，
  int4 的方向在 f32 基的前 4 维里只剩 `7.2%`；两个基各自低秩，但低秩的
  不是同一个方向。CoLA/hate 的方向几乎不转（cos `0.97+`），也就没有
  同量级的 int4 正向。
- 这把 M1.17/M1.18 的解释收紧了：`signal_top4` 的集中度是**每档自基**
  的读数；跨档可搬运性要先看方向是否对齐。M1.18 的负迁移因此不能只归因于
  “基不够好”：基对齐了，方向没对齐。
- **边界**：只测 f32 vs 均匀 int4、三个单句二分类集、两个 MiniLM、
  seq 64；没有测 int3/R12、SVC、MLP 或 encoder 微调；方向是类均值差，
  没有做逐样本方向或置信区间；test 方向只用于诊断，不参与选参。不能
  外推成“量化都会旋转类方向”，也不能把这套读数写成量化过程的因果消融。
  下一步若要检验因果，需要做量化算子级的干预，当前没有做。
- **方向角本身不是充分机制**：正交旋转是可逆线性变换，线性 probe 可以
  把它吸收。M1.20 的旋转对照把 f32 的类方向转到 int4 后，f32 精度仍是
  `0.800110`、`int4−f32` 仍是 `+0.040088`；真正随 int4 变化的是
  `sep/Fisher` 类可分性（§11.20）。

### 11.19.5 复算

```bash
python scripts/wsp_classify_direction.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --out sst2/direction-l12.json
```

完整 6 行在 `data/wsp-classify-direction.tsv`（6 个数据集-深度 ×
f32/int4 一对方向）；每行的 top1/2/4/8/16/32/64/128/256 在对应 JSON 的
`pairs[0].train/test.{left,right}_{own,in_other}` 里。

## 11.20 旋转对照与类可分性（M1.20）

### 11.20.1 问题与口径

M1.19 看到 SST-2 两档类方向夹角约 `55°`。但方向角本身不能解释线性 probe
的精度：正交旋转是可逆线性变换，`w' = R^{-T}w` 可以把旋转后的线性边界
原样映射回去；如果只旋转类方向、不改类内结构，线性 probe 的假设空间没变，
精度不应改变。需要把“方向角”和“类可分性”分开：

- **旋转对照**：`wsp_classify_rotate.py` 用 train 上的类均值差方向估计一个
  Householder 反射，把 f32 的类方向转到 int4 的方向；train/test 都乘同一个
  正交矩阵，probe 在旋转后的 train 上重训，C 仍按原口径在 train 内选。
- **类可分性**：直接读 M1.17 的 `sep = sqrt(between/within)`（Fisher 比值
  的平方根）。它在正交旋转下不变，是“信号相对类内散度”的尺度无关读数。
  同时报 `fisher = sep²`，以及 `between_sq` / `within_sq` 各自的变化。

旋转对照仍只训练线性 probe，冻结编码器；没有 MLP、没有 encoder 微调，
没有 MNLI。10k test-level paired bootstrap 只用于 `rotated−raw` 的 accuracy。

### 11.20.2 把 f32 转到 int4 方向，精度不变

| 数据集 | 深度 | cos 前→后 | f32 raw | f32 rotated | rotated−raw [95% CI] | int4 | gap raw→after |
|---|---|---:|---:|---:|---|---:|---:|
| SST-2 | L12 | 0.5715→1.0000 | 0.800110 | 0.800110 | `+0.000000` `[0,+0]` | 0.840198 | `+0.040088→+0.040088` |
| SST-2 | L6 | 0.6201→1.0000 | 0.797913 | 0.797913 | `+0.000000` `[0,+0]` | 0.804503 | `+0.006590→+0.006590` |
| CoLA | L12 | 0.9701→1.0000 | 0.575264 | 0.574305 | `−0.000952` `[−0.002876,+0]` | 0.586769 | `+0.011505→+0.012464` |
| CoLA | L6 | 0.9730→1.0000 | 0.582934 | 0.583893 | `+0.000963` `[0,+0.002876]` | 0.574305 | `−0.008629→−0.009588` |
| TweetEval-hate | L12 | 0.9757→1.0000 | 0.576445 | 0.576445 | `+0.000000` `[0,+0]` | 0.572787 | `−0.003658→−0.003658` |
| TweetEval-hate | L6 | 0.9698→1.0000 | 0.546818 | 0.546818 | `+0.000000` `[0,+0]` | 0.532187 | `−0.014631→−0.014631` |

- **SST-2 L12/L6 的 f32 精度与 `int4−f32` 差值在旋转后逐位不变**；把
  int4 反向转到 f32 方向也一样（L12 int4 `0.840198→0.840198`，gap
  `−0.040088→−0.040088`）。方向角不是能改变线性 probe 精度的量。
- CoLA 的 `±0.000952` 是 1,043 条 test 上的 1 条样本翻转（L12 C=10、
  L6 C=1），不是方向效应；hate 也是逐位不变。旋转矩阵的正交误差最大
  `1.1e-15`，`cos_after = 1.000000`。
- 因此 M1.19 的“方向旋转”应写成**伴随读数/症状**：它说明两档的类方向
  不同，但不能单独解释 int4 的精度优势。

### 11.20.3 真正变化的是 sep/Fisher

| 数据集 | 深度 | sep 比 train/test | Fisher 比 train/test | between 比 train | within 比 train |
|---|---|---:|---:|---:|---:|
| SST-2 | L12 | 1.288 / 1.236 | 1.659 / 1.529 | 3.757 | 2.264 |
| SST-2 | L6 | 1.126 / 1.073 | 1.267 / 1.152 | 2.357 | 1.861 |
| CoLA | L12 | 0.936 / 0.982 | 0.876 / 0.964 | 0.826 | 0.943 |
| CoLA | L6 | 0.974 / 1.018 | 0.948 / 1.036 | 0.898 | 0.947 |
| TweetEval-hate | L12 | 0.987 / 0.998 | 0.974 / 0.996 | 0.916 | 0.940 |
| TweetEval-hate | L6 | 1.008 / 1.004 | 1.016 / 1.007 | 0.964 | 0.949 |

- **SST-2 L12 的 `sep` 在 train 上升 `28.8%`、test 上升 `23.7%`**；Fisher
  比值是 `1.659× / 1.529×`。L6 也有正向但更弱（`1.126× / 1.073×`，
  Fisher `1.267× / 1.152×`）。
- 分解看，SST-2 L12 的 `between_sq` 增加 `3.757×`，`within_sq` 只增加
  `2.264×`，所以“类间信号”跑得比“类内噪声”快，`sep` 上升。CoLA/hate 的
  `between` 和 `within` 基本同向下降或持平（比值都在 1 附近），`sep` 不升，
  也就没有 int4 正向。
- 这解释了旋转对照为什么不变：旋转保持 `between/within`，所以 `sep` 不变、
  线性 probe 精度不变；M1.18 搬 PCA 基也只改坐标，不改 `sep`。**M1.19 的
  方向角、M1.17 的低秩集中都是伴随读数；在 SST-2 L12 上，`sep` 和精度
  同向（但 M1.21 的剂量-反应显示它不是跨数据集的普遍中介量）。**
- 但旋转只改方向、保持范数；SST-2 的 `sep` 上升还包含方向变长
  （`between_sq ×3.757`）和类内散度只增 `×2.264`。旋转对照因此只排除
  “方向角”，不能排除“方向长度/类内结构”；`sep` 把这些合在一起。
- 精度差值与 `sep` 只是定性同向：SST-2 L12 `sep 1.288×` 对应 `+0.040`，
  L6 `1.126×` 对应 `+0.0066`；CoLA/hate 的 `sep≈1` 对应 0 或负值。不能把
  这 6 个点拟合成“sep 每升 1% 就升多少 accuracy”的定律：类别平衡、probe
  正则和 C 选择都不同。

### 11.20.4 结论与边界

- **SST-2 的 L12 int4 正向对应的是“类可分性上升”，不是“类方向旋转”。**
  把 f32 的方向转到 int4 后精度逐位不变，说明方向角可被线性 probe 吸收；
  `sep`/Fisher 才是随 int4 变化、且在旋转下不变的量。SST-2 L12 的
  `sep 1.288×`（test `1.236×`）、Fisher `1.659×`（test `1.529×`），
  CoLA/hate 都在 1 附近。
- 这把 M1.17–M1.21 的解释链条收口成：低秩集中是伴随结构（M1.17），
  基不可搬运（M1.18），方向旋转是症状（M1.19），**`sep` 在 SST-2 L12
  上和精度同向（M1.20），但不是跨数据集的普遍中介量：M1.21 的剂量-反应
  显示 L6 的精度在 1/8 剂量见顶、`sep` 到 1/2 才见顶；CoLA L12 的 `sep`
  降到 `0.936×`，精度却升 `+1.15pp`**。仍不是量化算子的因果拆解：没有拆
  absmean/scale/per-group 哪一步造成 `sep` 变化，也没有噪声/交叉迁移对照。
- **边界**：只测 f32 vs 均匀 int4、三个单句二分类集、两个 MiniLM、seq 64、
  一个 logreg、一个 seed；`sep` 是欧氏 `between/within` 的 Fisher-like
  比值，不是完整协方差 LDA；旋转对照的 CoLA 有 1 条 test 样本的数值翻转；
  没有 MLP、encoder 微调或 MNLI。不能外推成“量化普遍提升类可分性”。

### 11.20.5 复算

```bash
python scripts/wsp_classify_rotate.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --rotate f32 --to int4 \
  --bootstrap 1 --bootstrap-iters 10000 \
  --out sst2/rotate-l12.json
```

`sep/Fisher` 由 `scripts/wsp_classify_geometry.py` 的 `fisher` / `sep` 字段
给出；完整 6 行在 `data/wsp-classify-rotation.tsv`（f32→int4 旋转、
`f32_raw`/`f32_rotated`、`gap`、`sep_ratio`、`fisher_ratio`）。

## 11.21 量化误差剂量-反应（M1.21）

### 11.21.1 问题与口径

M1.20 只证明了“方向角可被线性 probe 吸收”，并把 SST-2 L12 的正向和
`sep` 上升对上；但那是两个端点之间的静态比较，不能说明 `sep` 是不是
随量化误差连续起作用的量。M1.21 直接在 f32 和 int4 之间插值：

```text
x(λ) = normalize(f32 + λ * (int4 - f32)),  λ ∈ [0, 1]
```

- 端点保持原始 embedding（λ=0 是 f32，λ=1 是 int4）；中间点按 WSPEMB1
  的 mean-pool + L2 口径重新归一化。
- λ 网格：`0, 1/16, 1/8, 1/4, 1/2, 3/4, 1`，共 7 档。
- 每个 λ 都重新在 train 内 5 折选 C、重训同一个 logistic 线性 probe，
  test 只评一次；10k test-level paired bootstrap 只算
  `accuracy(λ) - accuracy(0)`。
- 同一个 λ 上同步算 `sep`、`between_sq`、`within_sq`（train/test 各自算），
  以及类方向对 f32/int4 的 cos。
- 另对 SST-2 L12/L6、CoLA L12 跑 `LinearSVC` 关键剂量
  `0, 1/8, 1/4, 1/2, 1` 的复验。
- 仍只训练线性 probe、冻结编码器；没有 MLP、encoder 微调、MNLI 或新数据集。

### 11.21.2 剂量-反应：SST-2 L12 在低剂量就饱和

下表给 accuracy 和相对 λ=0 的差值；`sep 比` 是 λ=1 时 train/test 的
`sep(1)/sep(0)`。完整 42 行在 `data/wsp-classify-interpolate.tsv`。

| 数据集/深度 | λ=0 acc | 1/16 Δ | 1/8 Δ | 1/4 Δ | 1/2 Δ | 1 Δ | sep 比 train/test |
|---|---:|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | 0.800110 | +0.010434 | +0.025261 | +0.038990 | +0.041735 | +0.040088 | 1.288 / 1.237 |
| SST-2 L6 | 0.797913 | +0.010434 | +0.019769 | +0.018122 | +0.014827 | +0.006590 | 1.126 / 1.073 |
| CoLA L12 | 0.575264 | −0.000959 | −0.001918 | +0.000959 | +0.005753 | +0.011505 | 0.936 / 0.982 |
| CoLA L6 | 0.582934 | −0.001918 | −0.003835 | −0.006711 | −0.010547 | −0.008629 | 0.974 / 1.018 |
| Hate L12 | 0.576445 | −0.001829 | +0.002560 | −0.002195 | −0.002560 | −0.003658 | 0.987 / 0.998 |
| Hate L6 | 0.546818 | −0.000732 | −0.013168 | +0.001829 | −0.012070 | −0.014631 | 1.008 / 1.004 |

SST-2 L12 的 bootstrap（对 λ=0）：

- 1/16 `+0.010398`，CI `[+0.002197,+0.018671]`；
- 1/8 `+0.025254`，CI `[+0.013729,+0.036793]`；
- 1/4 `+0.038952`，CI `[+0.024712,+0.053817]`；
- 1/2 `+0.041662`，CI `[+0.024163,+0.059308]`；
- 1 `+0.040034`，CI `[+0.020868,+0.058759]`。

关键不是“λ 越大越好”，而是**增益在低剂量就接近饱和**：1/8 剂量拿到
最终增益的约 `63%`，1/4 拿到约 `97%`，1/2 已经略高于全剂量；而 `sep`
到 1/8 只完成总增幅的约 `24%`，到 1/2 才约 `85%`。精度先饱和、`sep`
后饱和，两者不是同一条曲线。

SST-2 L6 更直接地脱钩：1/8 的 `+0.019769`（CI `[+0.009336,+0.030203]`）
是峰值，1/4 `+0.018122`，1/2 `+0.014827`（CI 跨 0），1 只剩 `+0.006590`
（CI 跨 0）；而 `sep` 到 1/2 才见顶 `1.128×`，1 又回到 `1.126×`。
**L6 的精度峰值比 `sep` 峰值早，且全剂量反而更差。**

CoLA L12 是符号相反的例子：`sep` 从 `1.000×` 单调降到 `0.936×`（train）
/ `0.982×`（test），accuracy 却从 `0.575264` 升到 `0.586769`（`+0.011505`，
CI 跨 0）；SVC 也从 `0.577181` 升到 `0.585810`（`+0.008629`，CI 跨 0）。
CoLA 的正向本身不显著，但它的路径方向足以说明：`sep` 不是线性 probe
精度变化的普遍中介量。

CoLA L6 的 accuracy 随剂量下降（1/2 `−0.010547`，1 `−0.008629`），
train `sep` 也降到 `0.974×`，test `sep` 反升到 `1.018×`；hate 两档都没有
正增益，L6 全剂量 `−0.014631`、CI 整段为负。**三个数据集没有一个统一的
“sep 升→精度升”关系。**

### 11.21.3 这否证了什么

M1.20 的结论要降级：`sep` 是 SST-2 L12 上的伴随读数，不是跨数据集的
充分中介量。更准确的表述是：

- **SST-2 L12 的 int4 正向是“低剂量、早饱和”的表示效应**：少量量化误差
  已经带来大部分增益，继续加剂量主要改变 `sep`，不再提升 accuracy；
- **L6 的精度峰值出现在 1/8，早于 `sep` 峰值**；
- **CoLA L12 的 `sep` 下降、accuracy 上升**，符号相反；
- hate 没有正增益，L6 还有稳定负增益。

这仍然不是量化算子的因果拆解：插值路径是每个 λ 一个固定的非线性映射，
probe 在每个 λ 重训，C 也重选；它说明“沿这条路径精度怎么变”，不说明
absmean/scale/per-group 哪一步单独造成变化。一个可检验的假设是：低剂量
量化误差像噪声/正则化，先改善 SST-2 的泛化，继续加剂量才破坏信息；但
本轮没有做 train-only 噪声、test-only 噪声或 f32/int4 交叉迁移矩阵，不能
把“正则化”写成结论。

**边界**：三个单句二分类集、两个 MiniLM、seq 64、一个 seed；主表是
logreg，SVC 只复验三条关键路径；中间点重新归一化，端点保持原始；
`sep` 是欧氏 Fisher-like 比值，不是完整协方差 LDA；没有 MLP、encoder
微调、MNLI。不能外推成“量化误差普遍提升线性分类”。

### 11.21.4 复算

```bash
python scripts/wsp_classify_interpolate.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --bootstrap 1 --bootstrap-iters 10000 \
  --out sst2/interpolate-l12.json
```

CoLA 加 `--class-weight balanced --cv-metric macro_f1`；SVC 加
`--probe linear_svc --lams 0,0.125,0.25,0.5,1`。主表
`data/wsp-classify-interpolate.tsv`（42 行），SVC 复验表
`data/wsp-classify-interpolate-svc.tsv`（15 行）。

## 11.22 train×test 交叉迁移矩阵（M1.22）

### 11.22.1 问题与口径

M1.21 看到 SST-2 L12 的增益在低剂量就饱和，留下两个不能区分的解释：

- **测试时表示更好**：int4 的 test embedding 本身让分类更容易；
- **训练时正则化**：在量化表示上训练出的 probe 更稳，即使 test 仍是 f32。

M1.22 用 5×5 交叉矩阵把两者拆开：

```text
train on x(train_λ), test on x(test_λ)
λ_train, λ_test ∈ {0, 1/8, 1/4, 1/2, 1}
```

- 每个 train λ 只训一个 logistic 线性 probe，C 仍在该 train 表示内 5 折选；
  同一个 probe 在 5 个 test λ 上各评一次，共 25 个格子。
- `(0,0)` 是 f32→f32 基线；`(0,1)` 是 test-only 量化；`(1,0)` 是
  train-only 量化（正则化 probe）；`(1,1)` 是 in-domain int4。
- 每个格子对 `(0,0)` 做 10k test-level paired bootstrap；test `sep` 同步记录。
- 主表是 logreg；SST-2 L12 / CoLA L12 另跑 `LinearSVC` 的 4×4
  （λ=`0,1/8,1/2,1`）复验。
- 仍只训练线性 probe、冻结编码器；没有 MLP、encoder 微调、MNLI 或新数据集。

### 11.22.2 四角与低剂量：SST-2 只在匹配剂量成立

下表每格是 accuracy（括号内为相对 `(0,0)` 的 Δ）；完整 150 行在
`data/wsp-classify-cross.tsv`，SVC 32 行在 `data/wsp-classify-cross-svc.tsv`。

| 数据集/深度 | (0,0) f32→f32 | (0,1) f32→int4 | (1,0) int4→f32 | (1,1) int4→int4 | (1/8,1/8) | (1/8,0) |
|---|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | 0.800110 (+0) | 0.771554 (−0.028556) | 0.567820 (−0.232290) | 0.840198 (+0.040088) | 0.825371 (+0.025261) | 0.795167 (−0.004942) |
| SST-2 L6 | 0.797913 (+0) | 0.746293 (−0.051620) | 0.702361 (−0.095552) | 0.804503 (+0.006590) | 0.817683 (+0.019769) | 0.798462 (+0.000549) |
| CoLA L12 | 0.575264 (+0) | 0.522531 (−0.052733) | 0.604027 (+0.028763) | 0.586769 (+0.011505) | 0.573346 (−0.001918) | 0.582934 (+0.007670) |
| CoLA L6 | 0.582934 (+0) | 0.591563 (+0.008629) | 0.567593 (−0.015340) | 0.574305 (−0.008629) | 0.579099 (−0.003835) | 0.580058 (−0.002876) |
| Hate L12 | 0.576445 (+0) | 0.595099 (+0.018654) | 0.551207 (−0.025238) | 0.572787 (−0.003658) | 0.579005 (+0.002560) | 0.577176 (+0.000732) |
| Hate L6 | 0.546818 (+0) | 0.542429 (−0.004389) | 0.531090 (−0.015728) | 0.532187 (−0.014631) | 0.533650 (−0.013168) | 0.535113 (−0.011704) |

SST-2 L12 的三个关键格子的 10k bootstrap：

- `(0,1)` `−0.028681`，CI `[−0.049973,−0.007688]`；
- `(1,0)` `−0.232229`，CI `[−0.260297,−0.203734]`；
- `(1,1)` `+0.040034`，CI `[+0.020868,+0.058759]`；
- `(1/8,1/8)` `+0.025254`，CI `[+0.013729,+0.036793]`；
- `(1/8,0)` `−0.004985`，CI `[−0.015925,+0.006041]`。

**SST-2 L12 的增益只在 train/test 剂量匹配时成立。** 单独把 test 换成
int4，f32 probe 掉 `−2.86pp`；单独把 train 换成 int4，probe 在 f32 test
上掉 `−23.23pp`，比 f32 基线低一大截；两者同时换，才有 `+4.00pp`。
`(1/8,0)` 是 M1.21 里拿到约 63% 最终增益的低剂量，但它对 f32 test 只有
`−0.50pp`（CI 跨 0）。**这直接排除“int4 训练把 probe 正则化”作为 SST-2
L12 的解释：低剂量 train-only 没有把 f32 test 拉起来，全剂量 train-only
反而灾难性掉点。** int4-trained probe 不是更稳的 f32 probe，而是已经适配
int4 坐标的 probe。

SST-2 L6 同向但幅度小：`(0,1) −5.16pp`（CI `[−0.074135,−0.030203]`）、
`(1,0) −9.56pp`（CI `[−0.117518,−0.073586]`）、`(1,1) +0.66pp`（CI 跨 0）；
`(1/8,0)` 只有 `+0.05pp`、CI 跨 0。**L6 也没有 train-only 正则化；它的
正向同样只在匹配剂量出现，只是对角本身不显著。**

SVC 复验（4×4，λ=`0,1/8,1/2,1`）给出同一结构：

| 数据集 | (0,0) | (0,1) | (1,0) | (1,1) | (1/8,1/8) | (1/8,0) |
|---|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | 0.797913 | 0.766063 (−0.031851) | 0.687534 (−0.110379) | 0.841845 (+0.043783) | 0.823174 (+0.025261) | 0.795167 (−0.002746) |
| CoLA L12 | 0.577181 | 0.511026 (−0.066155) | 0.605944 (+0.028763) | 0.585810 (+0.008629) | 0.577181 (+0.000000) | 0.586769 (+0.009588) |

SST-2 L12 的 `(1,0)` SVC 是 `−0.110454`、CI `[−0.133992,−0.086766]`，
`(1,1)` 是 `+0.043783`、CI `[+0.025261,+0.062603]`；
`(1/8,0)` 是 `−0.002820`、CI 跨 0。**换 probe 不改变“匹配才成立”的结论。**

### 11.22.3 三个数据集把“正向”落在不同格子

交叉矩阵最重要的读数是：三个数据集的“int4 正向”不在同一个格子上。

- **SST-2 L12：对角型。** 两个单边都是负的，只有 `(1,1)` 是 `+4.00pp`；
  `(0,1) −2.86pp`、`(1,0) −23.23pp`。这是**匹配表示效应**：quantized
  test 只对 quantized probe 有利，f32 probe 在 int4 test 上反而更差；
  int4 probe 回到 f32 test 上更差。增益不在“更好的 test 几何”里，也不在
  “更稳的 probe”里，而在两者的耦合里。
- **CoLA L12：train-only 型。** `(1,0) +2.88pp`，CI `[+0.008629,+0.049856]`，
  显著为正；`(0,1) −5.27pp`，CI `[−0.076702,−0.029722]`；对角 `(1,1)`
  只有 `+1.15pp`、CI 跨 0。SVC 同样：`(1,0) +2.87pp`、CI
  `[+0.007670,+0.049856]`，`(0,1) −6.62pp`、CI
  `[−0.090125,−0.043145]`，对角 `+0.86pp`、CI 跨 0。**CoLA 的正向是
  probe 正则化，不是测试表示；test-only 量化还会伤害它。** 两个单边在
  in-domain 对角上互相抵消，所以 M1.21 只看到 `+1.15pp`、CI 跨 0。
  `(1/8,0)` 也已经 `+0.77pp`（logreg CI `[0,+0.015340]`；SVC `+0.96pp`、
  CI `[+0.000959,+0.018217]`），说明低剂量 train-only 就足以启动这层正则化。
- **Hate L12：test-only 型。** `(0,1) +1.87pp`，CI
  `[+0.008413,+0.028895]`；`(1,0) −2.53pp`，CI
  `[−0.034382,−0.016459]`；对角 `(1,1) −0.37pp`、CI 跨 0。**clean f32
  probe 在 int4 test 上略好，但在 int4 上训练会掉；对角净效果仍是负的。**
  这不是 SST-2 式的匹配增益，也不能算“hate 复现了 int4 正向”。

CoLA L6 和 hate L6 没有正格子：CoLA L6 的 `(1,0) −1.53pp`、`(1,1) −0.86pp`，
CI 都跨 0；hate L6 的 `(1,0) −1.58pp`、`(1,1) −1.47pp`，CI 都为负
（`[−0.025604,−0.006218]` / `[−0.024150,−0.005121]`）。

**`sep` 不能解释这三种模式。** CoLA 的 `+2.88pp` 在 `(1,0)` 上，test
特征完全是 f32（test λ=0），test `sep` 没有变化；hate 的 `(0,1)` 正向前
test `sep` 也基本不动。`sep` 只在 SST-2 L12 的对角上和精度同向，仍只是
伴随读数；交叉矩阵把“对角差值”拆成 train-side 和 test-side 后，`sep`
不再是任何一格的解释变量。

### 11.22.4 这改变了什么

M1.21 的未决问题现在有答案：

- **SST-2 L12 的低剂量早饱和不是“更好的 test 表示”，也不是“正则化
  probe”，而是匹配表示**：train-only 1/8 对 f32 test 只有 `−0.50pp`，
  test-only 全剂量是 `−2.86pp`；只有两者同时到 1/8 才 `+2.53pp`。
  增益存在于 int4 坐标里的 probe 与 int4 坐标里的 test 的耦合，不在任一
  单边。
- **CoLA L12 的小正向确实是正则化**，但只在 train 侧；部署的均匀 int4
  是 train/test 同时量化，test-only 的 `−5.27pp` 会把它抵消到 `+1.15pp`
  （CI 跨 0）。所以不能把 CoLA 的 `(1,0)` 当作“int4 提升 CoLA”。
- **hate L12 的 `(0,1)` 是 test-only 的局部正向**，train 侧为负；对角仍
  是 `−0.37pp`。它不构成第三个复现。

因此主线问题的答案要再收紧一层：**SST-2 L12 的正向是数据集/深度特异的
匹配表示效应；几何结构（`sep` 上升、方向低秩集中）描述的是 int4 这一侧
的表示，但单独把 f32 投到 int4 基、单独把 f32 probe 放到 int4 test、或
单独在 int4 上训练 probe，都不能复现它。** 只有 encoder 输出和 probe
同时处于 int4 几何时才成立。这不是“量化普遍提升线性分类”，也不是
“int4 训练正则化”能概括的跨数据集机制。

**边界**：3 个数据集 × 2 个深度，主表 logreg，SVC 只复验 SST-2 L12 /
CoLA L12；一个 seed；C 在每个 train λ 内重选；bootstrap 只重采样 test，
不覆盖 train 侧方差（train 侧稳定性见 M1.16）。交叉矩阵的 25 个格子共享
同一个 train fit，格子之间不是独立实验；它适合定位 train/test 哪一侧
起作用，不适合做多重比较的显著性筛选。仍没有 MLP、encoder 微调、MNLI；
没有做 per-group/absmean 等量化算子内部的因果拆解。

### 11.22.5 复算

```bash
python scripts/wsp_classify_cross.py \
  --texts sst2/data-l12/texts.json \
  --run f32='sst2/emb12/l12-pure-f32-*.f32' \
  --run int4='sst2/emb12/l12-int4-*.f32' \
  --bootstrap 1 --bootstrap-iters 10000 \
  --out sst2/cross-l12.json
```

CoLA 加 `--class-weight balanced --cv-metric macro_f1`；SVC 加
`--probe linear_svc --lams 0,0.125,0.5,1`。主表
`data/wsp-classify-cross.tsv`（150 行：6 个数据集-深度 × 25 格），SVC
复验表 `data/wsp-classify-cross-svc.tsv`（32 行：2 个数据集 × 16 格）。

## 11.23 量化残差探针与类别条件偏置（M1.23）

> **M1.26 修正**：本节 SST-2 L12 的 `f32` 实为 ternary，`1.594× / +0.0676`
> 是 ternary→4-bit；真 f32→4-bit 是 `0.191× / −0.353`，见 §11.26。
> CoLA/hate 臂的 `f32` 是真 f32，数值保留。
>
> **M1.27 复核**：本节 SST-2 L12 的残差矩阵已用真 f32/int4 重跑；真 f32 下
> 没有合法残差臂超过 f32（§11.27）。`hybrid_rest_noclassmean` 与
> `hybrid_rest_perm_class` 在 test 侧使用 `y_test`，是泄漏/标签依赖对照，
> 不能再当控制臂引用。

### 11.23.1 问题与口径

M1.22 把 SST-2 L12 的正向定位成“train/test 匹配表示效应”：单边量化都掉点，
只有 train 和 test 同时在 int4 几何里才有 `+4.00pp`。但这仍只回答了“哪一侧
需要匹配”，没有回答“int4 表示里到底多了什么”。M1.23 直接看量化残差

```text
r = x_int4 − x_f32
```

把它当成一个特征来探：

- **残差单独**：在 `r` 上训练同一个线性 probe，看它自己是否带类别信息；
- **子空间**：PCA 只在 f32 train 上拟合，把 `r` 拆成前 k 个主成分
  `r_top_k` 和其余 `r_rest_k`；分别构造 `x_f32 + r_rest_k`（保留 f32 高方差
  部分、换入 int4 低方差残差）和 `x_f32 + r_top_k`（反方向）；
- **方向/幅度控制**：`r` 行归一化、`sign(r)`、逐行 `||r||`；
- **分布控制**：整体行置换、按类别内行置换、按残差逐维标准差匹配的高斯噪声、
  残差缩放、残差取负；
- **几何量**：类均值差 `|Δμ|`、残差类均值与 f32 类均值/夹角、残差类均值在
  f32 probe 权重上的投影（margin）。

主表 logreg；SST-2/CoLA/hate × L12/L6 共 6 组。CoLA 仍用 balanced +
train 内 macro-F1 选 C，其余用无权重 + accuracy 选 C；C 均在每个 arm 的
train 内 5 折重选。bootstrap 只重采样 test，10k 次，固定各 arm 的 probe。
仍只训练线性 probe、冻结编码器；没有 MLP、encoder 微调、MNLI。

### 11.23.2 残差单独就是一个分类特征，但只有 SST-2 L12 强

| 数据集/深度 | f32 | int4 | int4−f32 | 残差单独 | 残差−f32 | 残差 bootstrap 95% CI |
|---|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | 0.800110 | 0.840198 | +0.040088 | **0.832510** | **+0.032304** | [+0.010983,+0.052718] |
| SST-2 L6 | 0.797913 | 0.804503 | +0.006590 | 0.796266 | −0.001781 | [−0.022515,+0.018685] |
| CoLA L12 | 0.574305 | 0.586769 | +0.012354 | 0.595398 | +0.020704 | [−0.015340,+0.057526] |
| CoLA L6 | 0.583893 | 0.574305 | −0.009588 | 0.549377 | −0.034678 | [−0.069032,−0.001918] |
| Hate L12 | 0.576445 | 0.572787 | −0.003679 | 0.557059 | −0.019289 | [−0.035113,−0.002926] |
| Hate L6 | 0.546818 | 0.532553 | −0.014265 | 0.536576 | −0.010347 | [−0.026335,+0.005486] |

SST-2 L12 是唯一一个“只看残差”就明显高于 f32 的组：`0.832510`，比 f32
高 `+3.23pp`，已经接近 int4 的 `0.840198`。SST-2 L6 的残差单独只有
`0.796266`，和 f32 打平；CoLA L12 点估计高 `+2.07pp`，但 CI 跨 0；CoLA L6、
hate L12/L6 都低于 f32，其中 CoLA L6 与 hate L12 的 CI 整段为负。**残差不是
普适的“额外信号”；它在 SST-2 L12 上才明显带类别信息。**

SVC 在 SST-2 L12 上做了全 32 arm 复验
（`data/wsp-classify-residual-svc.tsv`）：f32 `0.797913`、int4 `0.841845`，
`int4−f32 +0.043932`（bootstrap `+0.043783`、CI `[+0.025261,+0.062603]`）；
残差单独 `0.829764`，对 f32 `+0.031730`、CI `[+0.010434,+0.052718]`；
`hybrid_rest_k16 0.841296`，对 f32 `+0.043328`、
CI `[+0.024163,+0.062603]`，对 int4 `−0.000455`、CI 跨 0。残差方向、
低方差 rest 和 probe margin 比值都与 logreg 同向，**不是 logistic 特有**。

残差的信息在**方向**而不是幅度。SST-2 L12：

- `r` 原始：`0.832510`；
- `r / ||r||`（逐行方向）：`0.833608`；
- `sign(r)`：`0.788578`；
- 逐行 `||r||`（单变量）：`0.550247`；
- f32 逐行 `||x||`：`0.499176`。

行归一化后不降、取符号后仍高、只看范数接近 chance，说明**残差的逐维方向
模式**带类别信息；范数本身不是分类器。`sign` 弱一些，但仍有 `+78.9%`。

### 11.23.3 增益在低方差残差：保留 f32 前 16 维，换入 int4 其余维度

SST-2 L12 的子空间 hybrid（PCA fit 在 f32 train；`k=0` 等价 int4，
`k=384` 等价 f32）：

| k | f32 + int4 残差 rest | f32 + int4 残差 top | 残差 rest 单独 | 残差 top 单独 |
|---:|---:|---:|---:|---:|
| 0 | 0.840198（=int4） | 0.800110（=f32） | 0.832510 | 0.499176 |
| 4 | **0.841296** | 0.802856 | 0.830313 | 0.528830 |
| 16 | **0.839649** | 0.800110 | 0.829764 | 0.580999 |
| 64 | 0.835255 | 0.816584 | 0.830313 | 0.715541 |
| 384 | 0.800110（=f32） | 0.840198（=int4） | 0.499176 | 0.832510 |

`k=4/16` 的 `f32 + int4 rest` 与完整 int4 统计打平：`k=16` 是 `0.839649`，
对 int4 的 10k bootstrap 是 `−0.000585`、CI `[−0.009336,+0.008237]`；
`k=4` 点估计还高 `+0.001098`。反过来，`int4 top16 + f32 rest` 只有
`0.800110`，正好退回 f32。也就是说：**SST-2 L12 的 int4 增益主要不在
前 16 个主成分里，而在前 16 维之外的低方差残差里。** f32 的高方差子空间
保留下来不会掉点，int4 的低方差残差才是新增量。

SST-2 L6 同方向但更弱：`k=16` hybrid `0.810544`，对 f32 `+1.25pp`、
CI `[−0.006590,+0.031301]` 跨 0；对 int4 `+0.61pp`、CI 跨 0。CoLA/hate
的 hybrid 都没有超过 f32（CoLA L12 `0.584851`、CoLA L6 `0.567593`、
Hate L12 `0.569495`、Hate L6 `0.538771`），且多数 CI 跨 0。

### 11.23.4 几何解释：SST-2 L12 的残差是“类别条件偏置”

只看 `sep` 仍不够；M1.23 把残差拆成类均值部分和类内部分。train 上的
类均值统计：

| 数据集/深度 | \|Δμ_f32\| | \|Δμ_resid\| | \|Δμ_int4\| | 残差/f32 | cos(f32,resid) | cos(f32,int4) |
|---|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | 0.088596 | **0.141233** | 0.171722 | **1.594×** | +0.067629 | +0.571547 |
| SST-2 L6 | 0.098561 | 0.118811 | 0.151329 | 1.205× | −0.039708 | +0.620130 |
| CoLA L12 | 0.043927 | 0.010994 | 0.039922 | 0.250× | −0.472800 | +0.970100 |
| CoLA L6 | 0.039152 | 0.009090 | 0.037097 | 0.232× | −0.336200 | +0.973000 |
| Hate L12 | 0.154370 | 0.033915 | 0.147760 | 0.220× | −0.300600 | +0.975700 |
| Hate L6 | 0.150261 | 0.036690 | 0.147539 | 0.244× | −0.195600 | +0.969800 |

再看 f32 probe 权重 `w` 上的 margin：`w·Δμ_f32`、`w·Δμ_resid`、`w·Δμ_int4`：

| 数据集/深度 | f32 margin | 残差 margin | int4 margin | 残差/f32 margin | cos(w,resid) |
|---|---:|---:|---:|---:|---:|
| SST-2 L12 | 2.206456 | **+2.664641** | 4.871101 | **+1.208×** | +0.344432 |
| SST-2 L6 | 2.362288 | +0.776867 | 3.139153 | +0.329× | +0.127333 |
| CoLA L12 | 0.558906 | −0.052901 | 0.506005 | −0.095× | −0.167726 |
| CoLA L6 | 0.325334 | −0.032060 | 0.293274 | −0.099× | −0.282616 |
| Hate L12 | 2.144552 | −0.141629 | 2.002923 | −0.066× | −0.206234 |
| Hate L6 | 1.963371 | −0.130023 | 1.833350 | −0.066× | −0.184483 |

注：probe margin 两列由 **f32 train probe** 与 **train 类均值**计算；
test 行只重复样本数/类均值几何，不重复 train margin。几何表里 test 行的
probe 列留空。

这三张表给出一个比 `sep` 更具体的答案：

- **SST-2 L12** 的残差类均值差是 f32 类均值差的 `1.59×`，但两者几乎正交
  （cos `+0.068`）；它不是把原方向简单拉长，而是加了一个新的类别条件
  位移。这个位移在 f32 probe 上的投影是 `+2.665`，**比 f32 自身的 margin
  `+2.206` 还大 1.21×**，于是 int4 的 probe margin 变成 `4.871`，accuracy
  跟着到 `0.840198`。
- **SST-2 L6** 的残差类均值也不小（`1.21×`），但它在 f32 probe 上的投影
  只有 `+0.777`，是 f32 margin 的 `0.33×`；所以只贡献 `+0.66pp`，对角
  CI 还跨 0。**残差位移大不等于对当前 probe 有用，关键是它是否落在 probe
  的判别方向上。**
- **CoLA/hate** 的残差类均值只有 f32 的 `0.22–0.25×`，且与 f32 类均值
  反向（cos `−0.20` 到 `−0.47`）；投影到 f32 probe 上全是负的
  （`−0.032` 到 `−0.142`），int4 margin 反而比 f32 小。**这解释了为什么
  CoLA/hate 的 `cos(f32,int4)` 高达 `0.97`：残差太小、方向又相反，几乎
  不改变类几何；它也解释了为什么 int4 在这些数据集上没有正向。**
- M1.19 看到的 SST-2 L12 `cos(f32,int4)=0.5715`、夹角 `55°`，正是这个
  近正交类均值位移造成的；M1.20 的旋转对照说明“方向角本身”可被线性 probe
  吸收，但 M1.23 说明**位移的幅度和它在 probe 上的投影**不是方向角能吸收的。

因此，主线问题现在可以写成：**SST-2 L12 的 int4 正向不是“量化噪声普遍
正则化”，而是一个类别条件的量化偏置：int4 的残差类均值把两个类中心推开，
且这个位移在 f32 probe 的判别方向上有足够大的正投影；CoLA/hate 的同类位移
小且反向，SST-2 L6 虽同向但投影只有 L12 的约四分之一。**

### 11.23.5 控制：不是幅度、不是各向同性噪声、不是样本级残差

SST-2 L12 的 10k bootstrap（相对 `hybrid_rest_k16`，accuracy）：

| 对照 | accuracy | Δ vs hybrid | 95% CI |
|---|---:|---:|---:|
| `hybrid_rest_k16` | 0.839649 | +0 | — |
| `r` 逐行归一化 | 0.833608 | −0.006041 | — |
| `sign(r)` | 0.788578 | −0.051071 | — |
| 逐行 `||r||` | 0.550247 | −0.289402 | — |
| 整体行置换 | 0.718836 | **−0.120773** | [−0.142779,−0.098298] |
| 类别内行置换 | 0.873696 | **+0.034108** | [+0.012630,+0.055464] |
| 去掉类均值 | 0.807798 | **−0.031924** | [−0.054915,−0.009336] |
| 高斯噪声（逐维 std 匹配） | 0.697968 | **−0.141705** | [−0.165294,−0.118616] |
| 残差 ×0.5 | 0.840747 | +0.001098 | [−0.0082,+0.0104] |
| 残差 ×2 | 0.836903 | −0.002746 | [−0.0121,+0.0066] |
| 残差取负 | 0.824822 | −0.014827 | [−0.0291,−0.0005] |

读法：

- **整体行置换**把残差和样本的对应关系打散，掉 `−12.08pp`；残差不是
  只靠边缘分布，样本级对应关系重要。
- **高斯噪声**按残差逐维标准差匹配，仍掉 `−14.17pp`；**不是各向同性
  噪声正则化**。M1.21 的“量化像加噪”直觉在这里被否掉。
- **残差 ×0.5 / ×2** 基本不掉点，说明幅度不是关键；方向和类别结构才是。
- **残差取负**掉 `−1.48pp`，CI 刚好不跨 0；符号/方向有作用，但不是
  简单“越大越好”。
- **类别内行置换**（保留每个类的残差分布、打散样本对应）反而 `+3.41pp`
  到 `0.873696`；**去掉类均值**则掉 `−3.19pp` 到 `0.807798`。这说明
  残差里有一块**类别条件均值**是增益来源，而样本级残差更像噪声；把样本级
  残差随机化、只保留类别条件分布，反而让 probe 更好用。

注意：类别内置换和“去掉类均值”都在 test 上使用了真实标签来构造残差，
它们是**机制诊断**，不是可部署方法；不能把 `0.873696` 当成一个新的分类器
精度。它们的作用是证明“类别条件结构”而不是“样本级残差”在起作用。

SST-2 L6 给出一个反向的对照：`hybrid_rest_k16 0.810544`，去掉类均值反而
`0.861065`（`+5.05pp`，CI `[+0.029105,+0.071938]`）。所以“类均值一定有用”
不能写成跨深度规律；L12 的类均值是增益，L6 的类均值更像是要被去掉的
偏移。这个深度差异目前只作为观察记录，不升级成机制结论。

### 11.23.6 结论与边界

M1.23 把 M1.22 的“匹配表示效应”再拆开一层：

- **SST-2 L12 的 int4 残差单独就是一个线性可分的类别特征**：`0.832510`，
  比 f32 高 `+3.23pp`；行归一化不降、范数接近 chance，说明是方向信息。
- **增益在低方差残差**：保留 f32 前 4/16 个主成分、换入 int4 其余维度，
  与完整 int4 打平（`k=16` 对 int4 `−0.000585`、CI 跨 0）；反过来保留
  int4 top16、换入 f32 rest 就退回 f32。M1.17 的 `signal_top4`、`eff_rank`
  是这一结构的伴随读数，但 M1.23 给出了更直接的子空间定位。
- **几何上是类别条件量化偏置**：SST-2 L12 的残差类均值差是 f32 的 `1.59×`，
  与 f32 类均值近正交，但在 f32 probe 上的投影是 f32 margin 的 `1.21×`；
  CoLA/hate 的残差类均值只有 `0.22–0.25×` 且投影为负；SST-2 L6 同向但
  投影只有 `0.33×`。**这是“为什么只有 SST-2 L12”目前最具体的几何答案。**
- **不是各向同性噪声，也不是 probe 正则化**：高斯噪声掉 `−14.17pp`，
  整体置换掉 `−12.08pp`，残差缩放基本不掉；M1.22 已排除 train-only
  probe 正则化。SST-2 L12 的正向是“int4 坐标里的类别条件位移 + 与之匹配的
  probe”，不是“量化普遍加噪”。

**边界**：6 个数据集-深度、主表 logreg；SST-2 L12 额外用 SVC 全 32 arm
复验、结论同向，其余五组没有 SVC。一个 seed；C 在每个 arm 内 5 折重选；
bootstrap 只重采样 test，不覆盖 train 侧方差。残差是量化算子的确定性函数，
本文只做观测性拆解，没有做 per-group/absmean、scale 或舍入模式的因果消融；
`resid_full` 需要同时拿到 f32 和 int4 两套 embedding，是诊断特征，不是
部署方案。类别内置换/去类均值使用 test 标签，是 oracle 对照，不能写成
可部署增益；SVC 的 `noclassmean` 方向仍为负但 CI 跨 0（logreg 显著为负），
这一项只能当两套 probe 的共同方向、不能当强显著结论。仍没有 MLP、
encoder 微调、MNLI。

### 11.23.7 复算

```bash
python scripts/wsp_classify_residual.py \
  --texts sst2/data-l12/texts.json \
  --f32 'sst2/emb12/l12-pure-f32-*.f32' \
  --int4 'sst2/emb12/l12-int4-*.f32' \
  --ks 4,16,64,384 --primary-k 16 \
  --bootstrap 1 --bootstrap-iters 10000 \
  --out sst2/residual-l12.json
```

CoLA 加 `--class-weight balanced --cv-metric macro_f1`；L6 把 texts/emb
换成 `data-l6` / `emb6` 和 `l6-pure-f32` / `l6-int4`。主表
`data/wsp-classify-residual.tsv`（192 行：6 个数据集-深度 × 32 arm），
SST-2 L12 的 SVC 复验 `data/wsp-classify-residual-svc.tsv`（32 行），
几何表 `data/wsp-classify-residual-geometry.tsv`（12 行：6 组 × train/test）。

## 11.24 直接量化代理：负对照（M1.24）

> **M1.26 修正**：本节 SST-2 L12 的输入 `f32` 是 ternary，不是真 f32；
> SST-2 行不能用来否定“真 f32→int4”的偏置。CoLA/hate 的 `f32` 是真
> f32，行保留；见 §11.26。

### 11.24.1 问题与口径

M1.23 的残差 `r = x_int4 − x_f32` 有一个容易过度解释的读法：既然残差带
类别条件偏置，是不是 **任何** 均匀 int4 舍入都会产生这种偏置？M1.24 做一个
更窄的负对照：

```text
x_f32  →  直接对 x 做 per-group 对称 int4 RTN  →  q
r_direct = q − x
```

`q` 和 `r_direct` 都只由冻结的 f32 embedding 决定，不经过编码器权重。
量化器与 `linear/uniform.rs` 一致：每组 `scale = max|x| / 7`，
`q = round(x/scale)` 截到 `±7`，反量化回 f32。组大小取 `1 / 32 / 384`：
`1` 是逐元素（退化为精确，残差只有浮点误差），`32` 是当前权重默认，
`384` 是逐行。

6 个数据集-深度、logreg、同样 train 内 5 折选 C；CoLA 仍用 balanced +
macro-F1 选 C。bootstrap 10k 只重采样 test，固定各 arm 的 probe。

### 11.24.2 直接量化不产生类条件偏置

主表 `data/wsp-classify-direct-quant.tsv`（18 行：6 组 × 3 个 group）。
摘要（accuracy；SST-2/CoLA 的 chance 约 0.50，hate 的多数类基线是 0.592）：

| 数据集/深度 | group | f32 | q | 直接残差 | 残差 macro-F1 | 残差/f32 类均值 | 残差 margin/f32 |
|---|---:|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | 1 | 0.800110 | 0.800110 | 0.499176 | 0.332967 | 0.000 | +0.0000 |
| SST-2 L12 | 32 | 0.800110 | 0.787479 | 0.499176 | 0.332967 | 0.0331 | +0.0002 |
| SST-2 L12 | 384 | 0.800110 | 0.768259 | 0.499176 | 0.332967 | 0.0557 | +0.0017 |
| SST-2 L6 | 32 | 0.797913 | 0.785832 | 0.499725 | 0.335200 | 0.0289 | +0.0003 |
| CoLA L12 | 32 | 0.574305 | 0.571429 | 0.542665 | 0.520100 | 0.0502 | +0.0025 |
| CoLA L6 | 32 | 0.583893 | 0.577181 | 0.488974 | 0.462900 | 0.0600 | −0.0032 |
| Hate L12 | 32 | 0.576445 | 0.574250 | 0.592173 | 0.371900 | 0.0131 | +0.0013 |
| Hate L6 | 32 | 0.546818 | 0.530358 | 0.592173 | 0.371900 | 0.0138 | +0.0005 |

读法：

- **q 本身没有增益**：SST-2 L12 直接量化后 `0.800110 → 0.787479`（g32）、
  `→ 0.768259`（g384）；SST-2 L6、CoLA、hate 也大多持平或略降。直接对
  最终 embedding 做 int4 不会复制 M1.23 的 `+4pp`。
- **直接残差单独不可分**：SST-2/CoLA 的残差 probe 在 `0.499–0.543`，
  基本是 chance；hate 的 `0.592` 正好是多数类基线（test 1619/1115），
  macro-F1 只有 `0.372`，不是类别信号。group=1 的残差几乎是零（浮点
  误差），它的 `0.499` 也是 chance。
- **残差类均值有一点几何量，但不投到 probe 上**：`resid/f32` 从 g1 的
  `0` 升到 g384 的 `0.056–0.081`，可是在 f32 probe 上的 margin 比只有
  `−0.008` 到 `+0.005`。这和 M1.23 的 SST-2 L12 `1.208×` 差了两个数量级。
- **bootstrap 方向也不支持正增益（§11.24）**：SST-2 L12 q−f32 在 g32 是
  `−0.012631 [−0.024163,−0.001098]`、g384 是
  `−0.031968 [−0.046142,−0.017573]`；残差−f32 全为负或跨 0。

### 11.24.3 结论与边界

**M1.23 的类别条件偏置不是“对最终 embedding 做 int4 舍入”就能复现的。**
直接量化的残差既没有足够大的类均值，也没有落在 f32 probe 方向上的正投影；
它更像高频舍入残差，单独 probe 接近 chance。因此 M1.23 的偏置依赖
**权重量化误差在编码器里的传播路径**，不是输出端量化的普适性质。

边界：这是 representation-level surrogate，不是权重 per-group/absmean 的
因果消融；`q` 的量化器与 `linear/uniform.rs` 同公式，但作用对象是 embedding，
不是权重。只跑 logreg；一个 seed；bootstrap 只重采样 test；group=1 是退化
端点（残差只有浮点误差），只作为 sanity check。weight per-group sweep 在
本机 CPU 上单档 20+ 分钟仍未完成，已停，不写成结论；absmean 也未测。

复算：

```bash
python scripts/wsp_classify_direct_quant.py \
  --texts sst2/data-l12/texts.json \
  --f32 'sst2/emb12/l12-pure-f32-*.f32' \
  --groups 1,32,384 --bootstrap 1 --bootstrap-iters 10000 \
  --out sst2/direct-l12.json
```

## 11.25 残差线性算子：可预测性与数据集特异几何（M1.25）

> **M1.26 修正**：本节 SST-2 L12 的 `f32` 是 ternary；`1.52× / 86°` 的
> 放大是 ternary→4-bit，真 f32→4-bit 是 `0.191× / −0.353`。CoLA/hate
> 的 `f32` 是真 f32，压缩结论保留；见 §11.26。
> **M1.28 追加**：本节 SST-2 L6 的 `f32` 也是 ternary；L6 的 `1.15× /
> 93.7° / 0.33×` 等读数同属 ternary→4-bit，不能当 f32→int4。CoLA/hate
> L6 在 M1.32 已全量确认是真 f32 且 bits4 全量对齐；见 §11.32。

### 11.25.1 问题与口径

§11.23 说明 `r = x_int4 − x_f32` 带类别条件偏置；§11.24 说明这个偏置不是
输出端舍入的普适性质。M1.25 问一个中间问题：**残差本身有多少是 f32
embedding 的线性响应？这个响应是跨数据集共同的，还是数据集特异的？**

只在 train split 上拟合 ridge 算子

```
r ≈ (x_f32 − μ) W + r_μ
```

`W` 是 384×384，`α=0.1` 固定（没有做 CV 选 α）。在 test split 上报
`R²`、逐样本余弦；再用 test 类均值算：

```
d_f = μ_f32(class 1) − μ_f32(class 0)
d_r = μ_r(class 1) − μ_r(class 0)
d_pred = Wᵀ d_f
```

`class_mean_gain = |d_r|/|d_f|` 是观测到的类均值放大率，
`operator_gain = |d_pred|/|d_f|` 是线性算子预测的放大率，
`class_mean_angle` 是 `d_f` 与 `d_r` 的夹角。另做两个控制：train 残差行
随机置换后重拟合（`shuffled_r2`），以及把三个数据集拼起来拟合一个共同
算子（`pooled`）。所有输入都是已冻结的 f32/int4 embedding，没有跑编码器
前向。

### 11.25.2 残差的大部分方差是线性可预测的

表 `data/wsp-classify-residual-linear.tsv`（24 行）。六组的 test `R²`：

| 组 | test R² | test cos | shuffled R² |
|---|---:|---:|---:|
| SST-2 L12 | 0.422404 | 0.843775 | −0.041220 |
| CoLA L12 | 0.501086 | 0.861727 | −0.048234 |
| Hate L12 | 0.446278 | 0.838915 | −0.081401 |
| SST-2 L6 | 0.451406 | 0.844327 | −0.043078 |
| CoLA L6 | 0.523482 | 0.860053 | −0.044116 |
| Hate L6 | 0.493052 | 0.840902 | −0.090525 |

也就是说，**42–52% 的 test 残差方差可以由 f32 embedding 的一个线性
函数预测**，逐样本余弦 0.84–0.86；把 train 残差打乱后 `R²` 全部为负
（−0.04 到 −0.09）。残差不是各向同性高频噪声：它有一个很大的、与输入
线性相关的成分。

### 11.25.3 类均值响应是数据集特异的

同一张表的类均值列：

| 组 | class_mean_gain | class_mean_angle | operator_gain | cos(d_r,d_pred) | cos(d_f,d_r) |
|---|---:|---:|---:|---:|---:|
| SST-2 L12 | 1.518319 | 86.06° | 1.101457 | 0.952390 | +0.068736 |
| CoLA L12 | 0.231379 | 108.10° | 0.174456 | 0.742082 | −0.310759 |
| Hate L12 | 0.206074 | 103.03° | 0.181739 | 0.922804 | −0.225386 |
| SST-2 L6 | 1.149203 | 93.75° | 0.967777 | 0.961088 | −0.065353 |
| CoLA L6 | 0.230621 | 98.84° | 0.173615 | 0.692430 | −0.153683 |
| Hate L6 | 0.223660 | 103.17° | 0.191941 | 0.933491 | −0.227843 |

SST-2 的类均值被残差**放大** 1.52×（L12）/1.15×（L6），并旋转约 86°–94°；
CoLA/hate 的类均值被**压缩**到 0.21–0.23×，旋转 99°–108°。线性算子能抓
住方向（`cos(d_r,d_pred)` 0.69–0.96），但系统性低估幅度：SST-2 L12 观测
1.52×、算子只有 1.10×；CoLA/hate 观测约 0.21–0.23×、算子约 0.17–0.19×。
所以“类均值方向”是线性可描述的，但“放大倍数”不是单靠这个线性拟合能
复现的。

### 11.25.4 没有一个跨数据集共同算子

把 SST-2 的算子直接用到 CoLA/hate 的 test 残差，`R²` 是 −43.41/−36.54
（L12）、−33.41/−27.34（L6），逐样本余弦只有 0.08–0.12；反过来把
CoLA/hate 的算子用到 SST-2，`R²` 只有 0.016–0.031（L12）/0.030–0.031
（L6）。三个数据集拼起来拟合的 pooled 算子能把 test `R²` 拉到
0.18–0.31、余弦 0.76–0.80，但类均值 `operator_gain` 仍是 SST-2 0.62、
CoLA/hate 0.19–0.22。**残差算子不是量化器的一个全局属性，它随数据集
的激活几何变化；SST-2 的类均值方向恰好是唯一被放大而不是压缩的方向。**

### 11.25.5 SST-2 残差的线性/非线性分解

对 SST-2 再把残差拆成 `r_hat = Wᵀ(x−μ)+r_μ` 和 `r_perp = r − r_hat`，
用同一套 logreg + 5 折 C 选择分别 probe（表
`data/wsp-classify-residual-linear-probe.tsv`，6 行）：

| 深度 | 分量 | accuracy | macro-F1 | 选中 C |
|---|---|---:|---:|---:|
| L12 | `r` | 0.832510 | 0.832470 | 10 |
| L12 | `r_hat` | 0.793520 | 0.793506 | 10 |
| L12 | `r_perp` | 0.630972 | 0.629645 | 1 |
| L6 | `r` | 0.796266 | 0.796218 | 1 |
| L6 | `r_hat` | 0.803954 | 0.803942 | 10 |
| L6 | `r_perp` | 0.583745 | 0.581775 | 1 |

L12 的线性分量单独就有 0.7935（完整残差 0.8325），剩下非线性分量
0.6310；L6 的线性分量 0.8040 甚至略高于完整残差 0.7963，非线性分量
0.5837。**SST-2 残差里的类别信号大部分（L12）或全部（L6）已经能被
f32 embedding 的线性函数承载；非线性余项仍高于 chance，但不是主载体。**
这是分解，不是因果消融：`r_hat` 是 train 上拟合的线性投影，probe 在
test 上评估；它说明“线性部分足够”，不说明“权重量化只做了线性变换”。

### 11.25.6 结论与边界

**SST-2 L12 的正向可以由几何结构描述，但不能由一个全局线性算子解释。**
权重量化留下的残差有一个大的线性可预测成分；这个成分的类均值响应是
数据集特异的：SST-2 被放大 1.5×并旋转近 90°，CoLA/hate 被压缩到 0.2×
并旋转超过 100°。SST-2 残差的类别信号也主要落在这个线性分量里。因此
“为什么只有 SST-2”至少有一部分答案是：**SST-2 的类均值方向落在量化
残差算子会放大的方向上；CoLA/hate 的类均值方向落在会被压缩的方向上。**
这不是权重误差逐层来源的证明，只是把 §11.23 的类别条件偏置进一步分解
成一个可复算的线性几何。

边界：ridge `α=0.1` 固定，没有 CV 选 α；只读最终 384 维 embedding，没有
逐层 hidden state；cross-transfer 用目标数据集自己的截距，衡量的是算子
方向而不是绝对尺度；`R²` 是 test 点估计，没有 bootstrap CI；一个 seed；
只跑 logreg。`class_mean_gain` 是 test 类均值比，受 test 类别比例影响；
`cos(d_f,d_r)` 与 §11.23 的 `cos(f32,resid)=+0.0676` 同一口径。不能把
“线性可预测”写成“量化是线性变换”，也不能把 pooled 算子当成部署结论。

复算：

```bash
python scripts/wsp_classify_residual_linear.py \
  --spec 'sst2-l12=sst2/data-l12/texts.json|sst2/emb12/l12-pure-f32-*.f32|sst2/emb12/l12-int4-*.f32' \
  --spec 'cola-l12=cola/texts-l12.json|cola/emb12/l12-pure-f32-*.f32|cola/emb12/l12-int4-*.f32' \
  --spec 'hate-l12=tweet-hate/texts-l12.json|tweet-hate/emb12/l12-pure-f32-*.f32|tweet-hate/emb12/l12-int4-*.f32' \
  --pooled --out sst2/residual-linear-l12.json

python scripts/wsp_classify_residual_linear.py \
  --spec 'sst2-l12=sst2/data-l12/texts.json|sst2/emb12/l12-pure-f32-*.f32|sst2/emb12/l12-int4-*.f32' \
  --probe --out sst2/residual-linear-probe-sst2-l12.json
```

## 11.26 基线标签审计：SST-2 的“f32”其实是 ternary（M1.26）

### 11.26.1 问题与结论

M1.12–M1.25 的 SST-2 结论都建立在 `sst2/emb12/l12-pure-f32-*.f32`
上。M1.26 逐位核对后发现，**这个文件不是 f32**，而是 `bert6` 在
`TRIT_WEIGHT_BITS=2` 下写的 ternary/absmean 权重输出。SST-2 L12 的
`+0.042207` 不是“int4 高于 f32”，而是“4-bit 相对 2-bit ternary 的
恢复”。真实 f32→int4 是 `−0.001503`，10k CI 跨 0。

同一审计还给出两条边界：

- CoLA/hate 的 `l12-pure-f32` 与 PyTorch 复刻的真 f32 只差 `2.1e-7`
  （f32 舍入误差量级），所以这两条数据集的旧 f32 基线是有效的；
- SST-2 L6 的 `l6-pure-f32` 来自另一个模型 `all-MiniLM-L6-v2`；M1.26
  当时只对照首 5 条、得到与真 f32 余弦 `0.97–0.99`，因此写了“没有 ternary
  迹象”。M1.28 用全量 8,741 条复核推翻了这个判断：官方 `l6-pure-f32` 与
  L6 真 ternary 余弦 `0.99999976`，与 L6 真 f32 只有 `0.373686`。L6 的
  `l6-pure-f32` 同样是 ternary，详见 §11.28。

因此修正范围不是“所有数据集”，而是 **SST-2 的 f32 基线**（M1.28 后确认
包括 L12 与 L6）。这也解释了为什么“正向只在 SST-2 出现”：SST-2 的 f32
标签被换成了更差的 ternary，CoLA/hate 的 L12 没有；CoLA/hate L6 的
`l6-pure-f32` 与 `l6-int4` 余弦约 `0.95`，倾向真 f32；M1.30/M1.32 已
补齐全量 f32/bits4 逐位验证（§11.30/§11.32）。

### 11.26.2 溯源方法

`bert6` 的 `f32_base` 只写 `pure-f32.f32`；量化模式按 `TRIT_EMBED_TAG`
写文件。官方 `l12-pure-f32-c*.f32` 是 **mode F32 + `TRIT_WEIGHT_BITS=2`**
的输出，不是 `f32_base` 的纯 f32。`--activation-mode f32` 只控制激活，
不关闭 `BertConfig.quantize` 的权重三值化。§11.11 记录每次运行都会先落
`pure-f32.f32`，跑下一档前再复制成 `l12-pure-f32`；SST-2 的复制点落在
一次 ternary 运行之后，CoLA/hate 落在真 f32 之后。两种情况共用同一个
文件名，所以从 tag 上看不出来。`TRIT_WEIGHT_BITS=2` 不走
`uniform::dequantize`，而走 `QuantLinear::from` 的 absmean 三值化
（每行一个 scale）；用对称 2-bit RTN 复刻会得到完全不同的 embedding
（SST-2 全量 acc `0.660626`），不能替代。

为避免依赖 Rust 前向的慢路径，M1.26 用 PyTorch 独立复刻 `bert6`：
12 层、GELU erf 近似、LayerNorm/线性累加用 f64、权重按
`TRIT_WEIGHT_BITS=4` 做 per-group 对称 RTN（group=32、`max|x|/7`、
半远离零、clamp ±7）。5 条 tiny 上，复刻 f32 与 `bert6` `pure-f32`
余弦 `1.0`，复刻 bits4 与 `bert6` `test.f32` 余弦 `1.0`；全量官方
`l12-int4` 与复刻 bits4 的 max abs diff `1.83e-7`。复刻不是逐位相同
（f32/f64 累加差异在 `2e-7` 量级），但足以判定标签与重算差值。

判定证据：

| 对照 | 结果 |
|---|---:|
| SST-2 `l12-pure-f32` vs 真 f32，首 5 条余弦 | `0.2790–0.3823` |
| SST-2 `l12-pure-f32` vs `TRIT_WEIGHT_BITS=2` tiny，首 5 条余弦 | `1.0`（逐条） |
| SST-2 `l12-int4` vs 真 bits4，全量 max abs diff | `1.83e-7` |
| CoLA `l12-pure-f32` vs 真 f32，全量 max abs diff | `2.09e-7` |
| Hate `l12-pure-f32` vs 真 f32，全量 max abs diff | `2.10e-7` |

CoLA/hate 的 `l12-int4` 与真 bits4 也只差 `1.45e-7`。所以 4-bit
生产路径没有错，错的是 SST-2 的 f32 文件名。
完整审计表见 `data/wsp-classify-baseline-audit.tsv`（M1.28 后 56 行）。

### 11.26.3 SST-2 L12 修正读数

冻结编码器，只训练 logreg；train 内 5 折选 C，test 只评一次；
bootstrap 10k 重采样 test。SST-2 无类别权重，指标 accuracy。

| arm | C | accuracy |
|---|---:|---:|
| 真 f32 | 1 | `0.842943` |
| 真 bits4 | 10 | `0.841296` |
| 官方 `l12-pure-f32`（ternary） | 10 | `0.797913` |
| 官方 `l12-int4` | 10 | `0.840198` |

| 比较 | Δ | 95% CI |
|---|---:|---:|
| 真 bits4 − 真 f32 | `−0.001503` | `[−0.012081, +0.008786]` |
| 官方 int4 − 官方 ternary | `+0.042207` | `[+0.023064, +0.061505]` |
| 官方 ternary − 真 f32 | `−0.044833` | `[−0.064250, −0.025261]` |
| 真 bits4 − 官方 int4 | `+0.001123` | `[−0.001647, +0.004393]` |

最后一行的 CI 跨 0，说明真 bits4 与官方 int4 只差 probe 噪声，4-bit
embedding 等价。`LinearSVC` 复验同向：真 bits4 − 真 f32
`−0.001040`（CI `[−0.010434, +0.008237]`），官方 int4 − 官方 ternary
`+0.043783`（CI `[+0.025261, +0.062603]`）。两种线性头都复现
“原正向是位宽恢复”，不是 int4 超过 f32。

### 11.26.4 几何修正：放大变成压缩

同一口径看 train 类均值（`d_f` 为 f32 类均值差，`d_q` 为 bits4 类均值差，
`d_r=d_q−d_f`）：

| 数据集 | norm(d_f) | norm(d_r) | norm(d_r)/norm(d_f) | cos(d_f,d_r) | norm(d_q)/norm(d_f) | cos(d_f,d_q) |
|---|---:|---:|---:|---:|---:|---:|
| SST-2 L12 | `0.180823` | `0.034505` | `0.190820` | `−0.352546` | `0.949667` | `0.982163` |
| CoLA L12 | `0.043927` | `0.010994` | `0.250283` | `−0.472799` | `0.908831` | `0.970110` |
| Hate L12 | `0.154370` | `0.033915` | `0.219699` | `−0.300562` | `0.957184` | `0.975744` |

三个数据集全部是**残差类均值压缩、与 f32 类均值负相关**，没有
SST-2 特异的放大。SST-2 的残差单独 probe 仍有 `0.804503`，只说明
残差里还留有可解码的标签信息，不说明它沿 f32 类均值方向增强了
margin。M1.23 的 `1.594× / +0.0676 / margin 1.21×` 对应的是
ternary→4-bit，不是 f32→4-bit；M1.25 的 `1.52× / 86°` 同样。

### 11.26.5 跨数据集修正

CoLA（balanced + macro-F1 选 C）与 Hate（无类别权重 + accuracy）用
同一复刻流程重算：

| 数据集 | 真 f32 | 真 bits4 | Δ | 95% CI |
|---|---:|---:|---:|---:|
| CoLA L12，macro-F1 | `0.548644` | `0.560717` | `+0.012073` | `[−0.007332, +0.030755]` |
| Hate L12，accuracy | `0.574250` | `0.571690` | `−0.002560` | `[−0.011704, +0.006584]` |

CoLA 的点估计为正但 CI 跨 0；Hate 点估计为负、CI 跨 0；SST-2 点估计
为负、CI 跨 0。三个数据集的真实 f32→int4 都没有统计显著的正向。
CoLA 的 accuracy 差是 `+0.013423`、macro-F1 差是 `+0.012073`，旧表
accuracy 是 `+0.013341`，同量级；旧表本身用的就是真 f32，所以 CoLA
的旧结论不需要推翻：它一直是“点估计小正、CI 跨 0”。Hate 的旧 f32
也是真 f32，旧负向结论不变；本文重算与旧表在 CV 折/选 C 上有
`0.2–0.7pp` 的 probe 噪声。

### 11.26.6 结论与边界

**SST-2 L12 的“均匀 int4 高于 f32”不存在。** 真实差值是
`−0.0015`（logreg）/`−0.0010`（SVC），CI 都跨 0；“只在 SST-2 出现”
是因为 SST-2 的 f32 基线被 ternary 替换，而 CoLA/hate 的 f32 没有。
因此这个问题不需要几何解释：现象本身是标签错误。M1.17–M1.22 的
SST-2 类均值方向/PCA/旋转/剂量读数、M1.23–M1.25 的 SST-2 残差几何
都应改读成“ternary→4-bit 的恢复”，不能再写成“f32→int4 的类别条件
偏置”。CoLA/hate 的 L12 臂仍是真 f32→4-bit，相关结论保留；SST-2 L6
已在 §11.28 确认为 ternary，L6 的旧 `int4−f32` 也要按 ternary→4-bit
读；CoLA/hate L6 在 M1.30/M1.32 已补齐全量，结论见 §11.30/§11.32。

边界：PyTorch 复刻是独立实现，不是 Rust 逐位复刻；f32/f64 累加差异在
`2e-7` 量级，足以判定标签和 `0.001` 级差值，但不能当位级等价。
M1.28 已重算 SST-2 L6；M1.30/M1.32 已补 CoLA/hate L6 全量；只跑 logreg +
LinearSVC，没有 MLP、没有 encoder 微调、没有 MNLI。历史表里 SST-2 L12/L6
的 `f32` 列一律按 ternary 读；需要真 f32 时用 §11.26/§11.28 的读数，
SST-2 全臂见 §11.29。



## 11.27 残差矩阵严格复核：真 f32 下没有残差子空间增益（M1.27）

### 11.27.1 问题与结论

M1.26 已证明 M1.23 的 SST-2 L12 `f32` 是 ternary。M1.27 用真 f32 /
真 bits4 重跑 `scripts/wsp_classify_residual.py` 的同一 32 臂矩阵，并逐条
审计对照构造。结论是：**真 f32 下没有任何合法的残差臂超过 f32**；
M1.23 的“残差子空间增益”同时来自 ternary 基线和两个使用 test 真标签的
对照，不是 f32→int4 的机制。

旧表与新表的直接对照：

| arm | M1.23 旧表（ternary 基线） | M1.27 真 f32 基线 | 旧 Δ vs f32 | 新 Δ vs f32 |
|---|---:|---:|---:|---:|
| `f32` | `0.800110` | `0.844591` | 0 | 0 |
| `int4` | `0.840198` | `0.840198` | `+0.040088` | `−0.004393` |
| `hybrid_rest_k4` | `0.841296` | `0.842943` | `+0.041186` | `−0.001647` |
| `hybrid_rest_k16` | `0.839649` | `0.838001` | `+0.039539` | `−0.006590` |
| `hybrid_rest_k64` | `0.835255` | `0.838550` | `+0.035145` | `−0.006041` |
| `hybrid_rest_k384` | `0.800110` | `0.844591` | `0` | `0` |

`hybrid_rest_k384` 在真基线下逐位回到 f32，`hybrid_top_k384` 逐位回到
int4（`0.840198`），说明端点构造正确。`hybrid_rest_k4` 只差 `−0.001647`
（约 3 个 test 样本），`hybrid_rest_k16` 差 `−0.006590`（约 12 个）。
没有残差臂在真 f32 上产生可复现的正向。

### 11.27.2 对照审计：`noclassmean` 和 `perm_class` 用了 test 真标签

M1.23 的 `hybrid_rest_noclassmean` 在 test 侧先按 `y_test` 求类均值，再从
test 残差里减去；`hybrid_rest_perm_class` 也按 `y_test` 做组内置换。两者
都在 test 特征构造里使用了真标签，是 oracle 诊断，不是合法对照。

M1.27 给脚本加严格模式：类均值只在 train 上用真标签估计；test 侧用
f32 base probe 的预测标签选择类均值，组内置换也按预测标签分组，不使用
任何 `y_test`。严格模式现在是默认，`--legacy-controls` 只用于复现 M1.23
旧表。

下表是**同一真 f32/真 bits4 输入**下 legacy 与 strict 的对照重跑，不是
M1.23 的 ternary 旧表；旧表的 noclassmean/perm_class 分别是 `+0.77pp` /
`+7.36pp`，同样来自 test 真标签构造。

| arm | legacy accuracy | strict accuracy | legacy Δ vs f32 | strict Δ vs f32 |
|---|---:|---:|---:|---:|
| `hybrid_rest_noclassmean` | `0.867655` | `0.837452` | `+0.023064` | `−0.007139` |
| `hybrid_rest_perm_class` | `0.852279` | `0.825920` | `+0.007688` | `−0.018671` |

`noclassmean` 的 `+2.31pp` 在严格模式下变成 `−0.71pp`，说明它原来的
“增益”完全来自 test 标签泄漏；`perm_class` 的 `+0.77pp` 也变成
`−1.87pp`，说明按真标签分组保留下来的类均值信号不能由预测标签复现。
M1.23 不能用这两行支持“去掉类均值后残差增益消失/保留”的任何结论。

### 11.27.3 严格矩阵读数

冻结编码器，只训练 logreg；train 内 5 折选 C，test 只评一次；无
bootstrap。f32 锚点为 `0.844591`，比 §11.26 的 `0.842943` 高
`0.001648`，原因是本脚本的 `_select_c` 用 `max_iter=2000`，§11.26 的
复算用 `5000`；这是 probe 求解器噪声，不是 embedding 差异。以下差值
都相对本矩阵内部的 f32 锚点。

| arm | C | accuracy | Δ vs f32 | Δ vs int4 |
|---|---:|---:|---:|---:|
| `f32` | 1 | `0.844591` | 0 | `+0.004393` |
| `int4` | 10 | `0.840198` | `−0.004393` | 0 |
| `hybrid_rest_k4` | 1 | `0.842943` | `−0.001647` | `+0.002746` |
| `hybrid_rest_k16` | 10 | `0.838001` | `−0.006590` | `−0.002197` |
| `hybrid_rest_k64` | 10 | `0.838550` | `−0.006041` | `−0.001647` |
| `hybrid_rest_k384` | 1 | `0.844591` | 0 | `+0.004393` |
| `hybrid_top_k16` | 1 | `0.839099` | `−0.005491` | `−0.001098` |
| `resid_full` | 10 | `0.803405` | `−0.041186` | `−0.036793` |
| `resid_rest_k16` | 10 | `0.796815` | `−0.047776` | `−0.043383` |
| `hybrid_scale_0.5` | 1 | `0.845140` | `+0.000549` | `+0.004942` |
| `hybrid_neg` | 1 | `0.845140` | `+0.000549` | `+0.004942` |

`hybrid_scale_0.5` / `hybrid_neg` 的 `+0.000549` 是 1 个 test 样本
（`0.000549 × 1,821 ≈ 1`），不是可解释的正向。`resid_full` 和
`resid_row_l2`（`0.807249`）仍说明残差单独可解码标签，但把它加回 f32
不改善分类；这与 §11.26 的“残差有信息、但不增强 margin”一致。

### 11.27.4 结论与边界

M1.23 的“保留 f32 前 16 维、换入 int4 其余维度”没有在真 f32 上复现。
`hybrid_rest_k16` 从旧表的 `+3.95pp` 变为 `−0.66pp`；所有合法臂都不超过
f32。旧表的正向应拆成三部分：`+4.00pp` 来自 ternary→4-bit 的基线恢复；
旧表的 `noclassmean +0.77pp` / `perm_class +7.36pp` 来自 test 真标签的
oracle 构造；在真 f32 上按同一 legacy 构造重跑，两个对照仍虚增
`+2.31pp` / `+0.77pp`，严格模式变成 `−0.71pp` / `−1.87pp`。三部分都
不是 f32→int4 的残差子空间机制。

边界：只复核 SST-2 L12 + logreg；没有 SVC、bootstrap、CoLA/hate 的
残差矩阵，也没有 MLP 或 encoder 微调。严格模式用 base probe 预测标签，
不是完全无标签的因果对照；它只保证不使用 `y_test`，不能保证预测标签
一定正确。完整 32 行见 `data/wsp-classify-residual-sst2-l12-strict.tsv`，
对照审计见 `data/wsp-classify-residual-control-audit.tsv`。

## 11.28 L6 基线审计：SST-2 的 `l6-pure-f32` 也是 ternary（M1.28）

### 11.28.1 问题与结论

M1.26 对 SST-2 L6 只做了首 5 条对照，写下“与 L6 真 f32 余弦
`0.97–0.99`、未发现 ternary 迹象”；这个判断在 M1.28 被全量复核推翻。
本轮用 `all-MiniLM-L6-v2` 全量生成真 f32、真 bits4、真 ternary 三份
`8741×384` embedding，并与官方 `l6-pure-f32-c000..c005.f32`、
`l6-int4-c000..c005.f32` 逐行比较：

| 对照 | 全量余弦均值 | 余弦范围 | max abs diff |
|---|---:|---:|---:|
| 官方 `l6-pure-f32` vs L6 真 ternary | `0.99999976` | `0.99999905–1.0000001` | `1.17e-3` |
| 官方 `l6-pure-f32` vs L6 真 f32 | `0.373686` | `0.084716–0.621769` | `0.397101` |
| 官方 `l6-pure-f32` vs L6 真 bits4 | `0.363878` | — | — |
| 官方 `l6-int4` vs L6 真 bits4 | `1.0` | — | `1.83e-7` |
| L6 真 f32 vs 真 bits4 | `0.948531` | `0.821476–0.972609` | `0.112170` |

首 5 条同样指向 ternary：官方 pure vs 真 ternary 逐条余弦
`0.9999997–0.9999999`，vs 真 f32 只有 `0.338123–0.459182`。所以
M1.26 的 `0.97–0.99` 不是“另一个 L6”造成的，而是当时的对照/参考配置
不成立；本轮不依赖首 5 条，判定用全量 8,741 条。

结论：**SST-2 的 `l6-pure-f32` 是 L6 ternary/absmean 输出，不是 f32；
官方 `l6-int4` 是真 bits4。** 这与 L12 的错误模式相同：f32 文件名落在
ternary 输出之后，4-bit 文件仍是正常 bits4。L6 的 `int4−f32` 正向因此
不是“int4 超过真 f32”，而是“4-bit 相对 2-bit ternary 的位宽恢复”。

### 11.28.2 修正后的 L6 分类读数

冻结编码器，只训练线性 probe；train 6,920 / test 1,821，train 内 5 折
选 C，test 只评一次，10k paired bootstrap。logreg 主表：

| arm | C | accuracy | macro-F1 |
|---|---:|---:|---:|
| 真 f32 | 1 | `0.811642` | `0.811576` |
| 真 bits4 | 1 | `0.806700` | `0.806620` |
| 官方 `l6-pure-f32`（ternary） | 10 | `0.795717` | `0.795678` |
| 官方 `l6-int4` | 1 | `0.806700` | `0.806620` |

| 比较 | logreg Δ | 95% CI | SVC Δ | 95% CI |
|---|---:|---:|---:|---:|
| 真 bits4 − 真 f32 | `−0.004964` | `[−0.013729, +0.003844]` | `−0.006079` | `[−0.014827, +0.002746]` |
| 官方 int4 − 官方 ternary | `+0.010811` | `[−0.009336, +0.030752]` | `+0.002492` | `[−0.017573, +0.023064]` |
| 官方 ternary − 真 f32 | `−0.015775` | `[−0.035695, +0.004393]` | `−0.008571` | `[−0.028556, +0.011532]` |
| 真 bits4 − 官方 int4 | `0` | `[0, 0]` | `0` | `[0, 0]` |

SVC 四档 accuracy 为 `0.811093 / 0.805052 / 0.802306 / 0.805052`，
selected C 为 `0.1 / 0.1 / 10 / 0.1`。最后一行是逐条预测完全相同，
不是“接近”：官方 `l6-int4` 与真 bits4 的 embedding max abs diff 只有
`1.83e-7`，两种 probe 都给出同一组 test 预测。

与旧表对照：M1.12 L6 `f32 0.798462`、`int4 0.804503`，旧
`int4−f32 = +0.006041`；M1.16 的 25 次子样本重训均值
`+0.0063`（logreg）/`+0.0036`（SVC）。这些正差值都建立在 ternary
`f32` 上，不能改写成“真 f32→int4 为正”。修正后的真 f32→int4 是
`−0.004964`（logreg）/`−0.006079`（SVC），两个 CI 都跨 0；官方
int4−官方 ternary 是 `+0.010811`（logreg）/`+0.002492`（SVC），CI 也都
跨 0。L6 的“正向”比 L12 更弱，但在修正后同样不成立。

### 11.28.3 跨数据集边界

用官方 `l6-pure-f32` 与 `l6-int4` 的余弦做快速诊断：

| 数据集 | L6 pure vs int4 余弦均值 | 范围 |
|---|---:|---:|
| SST-2 | `0.363878` | `0.100077–0.603399` |
| CoLA | `0.955514` | `0.839515–0.975569` |
| Hate | `0.953085` | `0.849936–0.973675` |

CoLA/hate 的 `0.95` 与 SST-2 真 f32 vs 真 bits4 的 `0.948531` 同量级，
倾向“真 f32 vs bits4”；SST-2 的 `0.36` 则是 ternary vs bits4。因此
SST-2 是**两个深度都出错**的数据集：L12 与 L6 的 `pure-f32` 都是
ternary；CoLA/hate L12 已确认是真 f32，L6 当时只有上述余弦诊断，
未做全量 PyTorch 逐位复刻，不能写成已确认。M1.30 已补齐全量复刻：
CoLA/hate 的 L6 `pure-f32` 都是真 f32（§11.30）。

### 11.28.4 结论与边界

- **M1.26 的 L6 边界作废**：SST-2 `l6-pure-f32` 不是 f32，是 ternary；
  `l6-int4` 是真 bits4。
- **L6 旧 `int4−f32` 正向作废**：真实 f32→int4 为
  `−0.004964`（logreg）/`−0.006079`（SVC），CI 都跨 0；旧
  `+0.0060` 是 ternary→4-bit 恢复。
- **SST-2 的“正向只在 SST-2”仍然成立，但范围扩大**：SST-2 的 L12 和 L6
  的 f32 标签都是 ternary；CoLA/hate L12 是真 f32，L6 在 M1.30 也已
  全量确认为真 f32。不能再把 SST-2 L6 的旧正向当反例。
- 边界：本节只做 SST-2 L6 的 f32/ternary/int4 标签判定；CoLA/hate L6
  的全量复刻在 M1.30 补齐（§11.30）；只跑 logreg + LinearSVC，没有 MLP、
  没有 encoder 微调、没有 MNLI。L6 的 `int3`/`rescan-k2` 已在 §11.29
  补跑，不能再用本节“未重算”的说法把它们留在旧锚点上。

复算材料：`data/wsp-classify-baseline-audit.tsv` 的 L6 行；全量
embedding 见实验目录 `l6-sst2-f32.npy` / `l6-sst2-bits4.npy` /
`l6-sst2-ternary.npy`，评测读数见 `l6-sst2-eval.json`。

## 11.29 SST-2 全臂矩阵：所有量化档都不超过真 f32（M1.29）

### 11.29.1 问题与结论

M1.26 修正了 L12 的 `f32` 标签，但只重算了 `int4`；M1.28 修正了 L6 的
`f32` 标签，也只重算了 `int3` 之外的 `int4/bits4`。M1.12 表里的
`int3`、`R12`、`rescan-k2` 仍然拿 ternary `f32` 当锚点，无法回答“这些
量化档相对真 f32 有没有增益”。M1.29 把两层的全臂矩阵一起重跑：

- L12：真 f32、真 bits4、官方 `l12-pure-f32`（ternary）、官方 `int3`、
  官方 `int4`、官方 `R12`；
- L6：真 f32、真 bits4、官方 `l6-pure-f32`（ternary）、官方 `int3`、
  官方 `int4`、官方 `rescan-k2`。

结论：**没有任何量化档超过真 f32。** L12 的 `int3` 和 `R12` 显著低于
真 f32（CI 整段为负）；`int4` 点估计略低但 CI 跨 0。L6 的
`int3/int4/rescan` 都略低于真 f32，CI 都跨 0。M1.12 的“量化档高于
f32”不只 int4，连 int3/R12/rescan 也是 ternary 基线的位宽恢复。

### 11.29.2 臂的口径与来源

- L12 的 `int3`/`R12` 是官方 embedding；没有为它们另做 PyTorch 逐位
  复刻。它们相对真 f32 的全量余弦为 `0.746296`（`int3`）和 `0.777828`
  （`R12`），明显是 3-bit/混合预算量级，不是 ternary；方向判定不依赖
  逐位复刻。
- L6 的 `int3` 用 `all-MiniLM-L6-v2` 独立生成真 bits3，官方
  `l6-int3` 与真 bits3 余弦 `1.0`、max abs diff `1.49e-7`；`l6-int4` 与
  真 bits4 的 max abs diff `1.83e-7`。因此 L6 的 int3/int4 标签有效。
- `R12` 与 `rescan-k2` 是混合预算臂（L12 `6/54/12`、L6 `6/24/6`），
  没有单一“真”可复刻；本轮直接对官方臂评分，相对真 f32 的余弦分别为
  `0.777828`（R12）和 `0.791706`（rescan-k2）。

### 11.29.3 L12 全臂读数

logreg，train 6,920 / test 1,821，5 折选 C，test 只评一次：

| arm | C | accuracy | macro-F1 |
|---|---:|---:|---:|
| 真 f32 | 1 | `0.842943` | `0.842904` |
| 真 bits4 | 10 | `0.841296` | `0.841212` |
| 官方 `l12-int3` | 1 | `0.823174` | `0.823109` |
| 官方 `l12-int4` | 10 | `0.840198` | `0.840113` |
| 官方 `l12-r12` | 10 | `0.819879` | `0.819788` |
| 官方 `l12-pure-f32`（ternary） | 10 | `0.797913` | `0.797875` |

| 比较 | logreg Δ | 95% CI | SVC Δ | 95% CI |
|---|---:|---:|---:|---:|
| 官方 int3 − 真 f32 | `−0.019732` | `[−0.035146, −0.004942]` | `−0.024053` | `[−0.040088, −0.008237]` |
| 官方 int4 − 真 f32 | `−0.002625` | `[−0.013180, +0.007688]` | `−0.001040` | `[−0.010434, +0.008237]` |
| 官方 R12 − 真 f32 | `−0.022967` | `[−0.038440, −0.007688]` | `−0.028556` | `[−0.043932, −0.013180]` |
| 真 bits4 − 真 f32 | `−0.001503` | `[−0.012081, +0.008786]` | `−0.001040` | `[−0.010434, +0.008237]` |
| 官方 int4 − 官方 int3 | `+0.017107` | `[+0.001647, +0.032400]` | `+0.023014` | `[+0.007688, +0.038440]` |
| 官方 R12 − 官方 int3 | `−0.003235` | `[−0.017024, +0.010434]` | `−0.004502` | `[−0.019220, +0.010434]` |
| 官方 R12 − 官方 int4 | `−0.020341` | `[−0.035695, −0.004942]` | `−0.027516` | `[−0.042834, −0.012630]` |
| 官方 int4 − 官方 ternary | `+0.042207` | `[+0.023064, +0.061505]` | `+0.043783` | `[+0.025261, +0.062603]` |
| 真 bits4 − 官方 int4 | `+0.001123` | `[−0.001647, +0.004393]` | `0` | `[0, 0]` |

L12 的 SVC 六档 accuracy 为 `0.842943 / 0.841845 / 0.818781 /
0.841845 / 0.814388 / 0.797913`（顺序同上表）。`int3`/`R12` 的 CI 整段
为负，说明它们在真 f32 下不是“略低”，而是明显更差；`int4` 只是点估计
略低、CI 跨 0。

### 11.29.4 L6 全臂读数

| arm | logreg accuracy | SVC accuracy |
|---|---:|---:|
| 真 f32 | `0.811642` | `0.811093` |
| 真 bits4 | `0.806700` | `0.805052` |
| 官方 `l6-int3` | `0.808347` | `0.809445` |
| 官方 `l6-int4` | `0.806700` | `0.805052` |
| 官方 `l6-rescan-k2` | `0.803405` | `0.802856` |
| 官方 `l6-pure-f32`（ternary） | `0.795717` | `0.802306` |

| 比较 | logreg Δ | 95% CI | SVC Δ | 95% CI |
|---|---:|---:|---:|---:|
| 官方 int3 − 真 f32 | `−0.003291` | `[−0.017573, +0.010983]` | `−0.001632` | `[−0.016474, +0.013180]` |
| 官方 int4 − 真 f32 | `−0.004964` | `[−0.013729, +0.003844]` | `−0.006079` | `[−0.014827, +0.002746]` |
| 官方 rescan − 真 f32 | `−0.008296` | `[−0.024163, +0.007139]` | `−0.008268` | `[−0.024163, +0.007139]` |
| 官方 int4 − 官方 int3 | `−0.001673` | `[−0.016474, +0.012630]` | `−0.004447` | `[−0.019769, +0.010983]` |
| 官方 rescan − 官方 int3 | `−0.005005` | `[−0.019769, +0.009336]` | `−0.006636` | `[−0.021980, +0.008237]` |
| 官方 rescan − 官方 int4 | `−0.003332` | `[−0.019220, +0.012081]` | `−0.002189` | `[−0.017573, +0.013180]` |
| 官方 int4 − 官方 ternary | `+0.010811` | `[−0.009336, +0.030752]` | `+0.002492` | `[−0.017573, +0.023064]` |
| 真 bits4 − 官方 int4 | `0` | `[0, 0]` | `0` | `[0, 0]` |

L6 的所有量化档都低于真 f32，CI 都跨 0；`int3` 点估计高于 `int4`，
`rescan` 在量化档中最低（ternary 仍是最低的总锚点）。这里没有“某个量化档
超过 f32”的证据，连点估计都没有。

### 11.29.5 与 M1.12 旧表对照

M1.12 的旧锚点是 ternary `f32`，所以旧表的“量化增益”需要整列改写：

| 臂 | 旧 f32 anchor | 旧 accuracy | 旧 Δ | 真 f32 | 新 Δ |
|---|---:|---:|---:|---:|---:|
| L12 int3 | `0.800110` | `0.823723` | `+0.023613` | `0.842943` | `−0.019732` |
| L12 int4 | `0.800110` | `0.840198` | `+0.040088` | `0.842943` | `−0.002625` |
| L12 R12 | `0.800110` | `0.814937` | `+0.014827` | `0.842943` | `−0.022967` |
| L6 int3 | `0.798462` | `0.809445` | `+0.010983` | `0.811642` | `−0.003291` |
| L6 int4 | `0.798462` | `0.804503` | `+0.006041` | `0.811642` | `−0.004964` |
| L6 rescan-k2 | `0.798462` | `0.802306` | `+0.003844` | `0.811642` | `−0.008296` |

旧表里的所有正差值都消失了；不是“量化让线性头更好”，而是“ternary
锚点让 f32 看起来更差”。L12 的 `int4−int3 = +1.71pp`（CI 全正）仍是
真实的档内排序，但它只在量化档内部成立，不能升格成“量化超过 f32”。

### 11.29.6 结论与边界

- **SST-2 的“量化提升线性分类”在 L12/L6 全臂上都不成立**：没有任何
  量化档超过真 f32；L12 `int3`/`R12` 还显著更差。
- **M1.12 的正向要整体改读**：`int3`、`int4`、`R12`、`rescan-k2` 的旧
  `Δ vs f32` 都是相对 ternary 的位宽恢复，不是相对真 f32 的增益。
- **可保留的档内结论**：L12 `int4` 优于 `int3`（`+1.71pp`，CI 全正）；
  L6 的 `int3/int4` 差异不显著。这个排序不能跨到“超过 f32”。
- 边界：只做 SST-2；L12 的 `int3`/`R12` 和 L6 的 `rescan-k2` 是官方臂，
  未做独立逐位复刻；L6 `int3`/`int4` 已用真 bits3/bits4 对齐。只跑
  logreg + LinearSVC，没有 MLP、没有 encoder 微调、没有 MNLI。
- 完整 64 行见 `data/wsp-classify-sst2-full-arms.tsv`；L12/L6 的原始
  评测 JSON 为 `sst2-l12-arms-eval.json` / `l6-sst2-arms-eval.json`。

## 11.30 CoLA/hate 的 L6 `l6-pure-f32` 全量确认真 f32（M1.30）

### 11.30.1 问题与结论

M1.28.3 只做了 `official pure vs official int4` 的余弦诊断（CoLA
`0.955514`、hate `0.953085`），把 CoLA/hate L6 的 `l6-pure-f32` 暂标为
“倾向真 f32、未全量确认”。M1.30 直接把这条边界做完：用
`all-MiniLM-L6-v2` 读取官方同一批 `chunks-l6-1500` 的 `input.f32` /
`mask.u8`，全量生成真 f32 与真 ternary，再和官方
`emb6/l6-pure-f32-*.f32` 逐行比较。

结论：**CoLA 与 hate 的 L6 `l6-pure-f32` 都是真 f32，不是 ternary。**
官方 `l6-int4` 与真 f32 的全量余弦分别只有 `0.955514` / `0.953085`，
是正常的 f32→bits4 距离；首 512 行与真 bits4 逐位一致，M1.32 已补成
全量（§11.32）。因此
M1.14/M1.15 的 L6 基线无需重算，SST-2 是唯一 L12 和 L6 的 `pure-f32`
都被错标成 ternary 的数据集。

### 11.30.2 口径与复算材料

- 模型：`all-MiniLM-L6-v2`，6 层 / 384 维 / 12 头，seq 64；forward
  与 `torch_l6_full.py` 相同（手写 LayerNorm/GELU、mean pooling、
  L2 normalize）。
- 输入：直接读官方 `chunks-l6-1500/chunk-*/input.f32` 与 `mask.u8`，
  不重新 tokenize，避免 tokenizer 差异污染标签判定。
- 全量规模：CoLA `9,594×384`，hate `11,719×384`；官方文件按 chunk
  顺序拼接后逐行比较。
- 真值文件：实验目录 `geometry/corrected/l6-cola-f32.npy` /
  `l6-cola-ternary.npy`、`l6-tweet-hate-f32.npy` /
  `l6-tweet-hate-ternary.npy`；脚本 `full_l6_other.py`（复用
  `probe_l6_labels.py` 的 forward）。
- 官方臂：`emb6/l6-pure-f32-000..006.f32`、`l6-int4-000..006.f32`；
  hate 为 `000..007`，格式 `WSPEMB1`。

### 11.30.3 全量逐行读数

| 数据集 | 对照 | 余弦均值 | 余弦范围 | max abs diff |
|---|---|---:|---:|---:|
| CoLA（9,594） | 官方 `l6-pure-f32` vs 真 f32 | `1.00000000` | `0.99999988–1.00000024` | `1.81e-7` |
| CoLA（9,594） | 官方 `l6-pure-f32` vs 真 ternary | `0.463824` | `0.194986–0.669567` | `0.325793` |
| CoLA（9,594） | 官方 `l6-int4` vs 真 f32 | `0.955514` | `0.839514–0.975569` | `0.112572` |
| CoLA（9,594） | 官方 `l6-int4` vs 真 ternary | `0.455618` | `0.171715–0.637002` | `0.332822` |
| Hate（11,719） | 官方 `l6-pure-f32` vs 真 f32 | `1.00000000` | `0.99999988–1.00000024` | `1.91e-7` |
| Hate（11,719） | 官方 `l6-pure-f32` vs 真 ternary | `0.378651` | `0.043444–0.619112` | `0.447816` |
| Hate（11,719） | 官方 `l6-int4` vs 真 f32 | `0.953085` | `0.849936–0.973675` | `0.106138` |
| Hate（11,719） | 官方 `l6-int4` vs 真 ternary | `0.369449` | `0.061683–0.623018` | `0.437681` |

`official pure vs true f32` 的 max abs diff 只有 `1.8–1.9e-7`，是纯
浮点舍入；对真 ternary 的余弦只有 `0.46` / `0.38`，与 SST-2 L6 的
`0.99999976`（pure vs ternary）形成清晰对照。因此不是“部分行像
f32”或“混合输出”，而是全量 f32。

`official int4` 的 512 行首块与真 bits4 逐位比较（M1.32 已补全量，见 §11.32）：

| 数据集（M1.30 首块 512 行） | 余弦均值 | 余弦范围 | max abs diff |
|---|---:|---:|---:|
| CoLA `l6-int4` vs 真 bits4 | `1.00000000` | `0.99999988–1.00000024` | `1.26e-7` |
| Hate `l6-int4` vs 真 bits4 | `1.00000000` | `0.99999994–1.00000024` | `1.38e-7` |

全量 bits4 没有重跑；但全量 `official int4 vs true f32` 余弦与
SST-2 真 f32 vs 真 bits4 的 `0.948531` 同量级，且首块逐位一致，
足以确认官方 `l6-int4` 仍是正常 bits4，而不是 ternary 或 f32。

### 11.30.4 结论与边界

- **CoLA/hate L6 `l6-pure-f32` 标签收口**：两者都是真 f32；M1.28.3
  的“倾向”可以改为“已确认”。
- **SST-2 是唯一双深度错标的数据集**：SST-2 L12/L6 的 `pure-f32`
  都是 ternary；CoLA/hate 的 L12（M1.26）和 L6（M1.30）都是真 f32。
- **CoLA/hate L6 分类结果不需要重算**：基线标签没有变化，M1.14/M1.15
  的 L6 读数继续有效；不能把 SST-2 的 `int4−f32≈+0.04` 外推到这两个
  数据集。
- 边界：本节只做标签判定；全量生成真 f32/ternary，bits4 用首 512 行
  逐位验证。M1.32 已补全量 bits4 并补跑分类臂（§11.32）。只涉及线性
  probe 的基线，不做 MLP、不做 encoder 微调、不做 MNLI。

复算材料：`data/wsp-classify-baseline-audit.tsv` 新增 CoLA/hate L6
provenance 行；原始读数见上表，脚本在实验目录未入库。

## 11.31 CoLA/hate 全臂矩阵：没有任何量化档显著超过真 f32（M1.31）

### 11.31.1 问题与口径

M1.14/M1.15 已经测过 CoLA/hate 的 `int4`，但没有把全部量化臂在同一
口径下对齐；而且 M1.15 的 hate L12 主表用固定 `C=1`，与 SST-2 的
per-arm 5 折选 C 不可直接拼表。M1.31 用同一套流程重跑：

- train 内 5 折 `StratifiedKFold(random_state=7)`，C 网格
  `0.01/0.1/1/10`，每个臂独立选 C；test 只评一次。
- CoLA：`balanced` class weight + macro-F1 选 C，train 8,551 / test 1,043。
- Hate：无类别权重 + accuracy 选 C，train 8,985 / test 2,734。
- logreg 与 LinearSVC 都跑；10k paired bootstrap，报告主指标差值 CI。
- 真 f32 锚点：CoLA/hate 的 L12 用独立生成的
  `cola-l12-f32.npy` / `hate-l12-f32.npy`；L6 用 `l6-cola-f32.npy` /
  `l6-tweet-hate-f32.npy`。官方 `pure-f32` 与真 f32 只差 `2.1e-7`
  量级，但分类预测会有 0–5 条翻转，因此以真 f32 为锚点，不用官方
  `pure-f32` 代替。
- 臂：`true-bits4`（M1.31 仅 L12；M1.32 补 L6）、`official-pure-f32`、
  `official-int3`、`official-int4`、`official-r12`（L12）/
  `official-rescan-k2`（CoLA L6）/ `official-rescan`（hate L6）。L6 的
  官方 `int4` 已由 M1.32 全量逐行确认与真 bits4 一致（§11.32）；L12 的
  `true-bits4` 是 M1.31 独立生成的全量臂。

### 11.31.2 CoLA L12（主指标 macro-F1）

| probe | arm | C | accuracy | macro-F1 | Δ vs true f32 | 95% CI |
|---|---|---:|---:|---:|---:|---:|
| logreg | true f32 | 10 | `0.573346` | `0.548644` | — | — |
| logreg | true bits4 | 10 | `0.586769` | `0.560717` | `+0.011989` | `[−0.007385,+0.031764]` |
| logreg | official pure-f32 | 10 | `0.576222` | `0.552321` | `+0.003627` | `[−0.004144,+0.011653]` |
| logreg | official int3 | 10 | `0.558965` | `0.534089` | `−0.014776` | `[−0.043998,+0.014406]` |
| logreg | official int4 | 10 | `0.586769` | `0.561151` | `+0.012395` | `[−0.006947,+0.031930]` |
| logreg | official R12 | 10 | `0.566635` | `0.543040` | `−0.005731` | `[−0.033790,+0.023189]` |
| linear_svc | true f32 | 10 | `0.577181` | `0.550525` | — | — |
| linear_svc | true bits4 | 10 | `0.585810` | `0.559916` | `+0.009261` | `[−0.009135,+0.027831]` |
| linear_svc | official pure-f32 | 10 | `0.577181` | `0.550525` | `0` | `[0,0]` |
| linear_svc | official int3 | 10 | `0.566635` | `0.539541` | `−0.011200` | `[−0.040025,+0.017882]` |
| linear_svc | official int4 | 10 | `0.585810` | `0.559916` | `+0.009261` | `[−0.009135,+0.027831]` |
| linear_svc | official R12 | 10 | `0.563758` | `0.538937` | `−0.011817` | `[−0.040323,+0.016637]` |

CoLA L12 的 `int4`/`true bits4` 点估计为正，是四个组合里唯一“看起来
可能增益”的格子；但 logreg `+1.24pp` 的 CI 跨 0，SVC `+0.93pp` 也跨 0。
`int3`/`R12` 都是负向，`R12` 在 SVC 上也没有转正。

### 11.31.3 CoLA L6（主指标 macro-F1）

| probe | arm | C | accuracy | macro-F1 | Δ vs true f32 | 95% CI |
|---|---|---:|---:|---:|---:|---:|
| logreg | true f32 | 1 | `0.577181` | `0.553124` | — | — |
| logreg | official pure-f32 | 1 | `0.581975` | `0.557138` | `+0.004024` | `[−0.002409,+0.010906]` |
| logreg | official int3 | 10 | `0.570470` | `0.547084` | `−0.005909` | `[−0.032745,+0.020631]` |
| logreg | official int4 | 1 | `0.568552` | `0.543789` | `−0.009351` | `[−0.027439,+0.008747]` |
| logreg | official rescan-k2 | 1 | `0.565676` | `0.540530` | `−0.012499` | `[−0.039901,+0.015703]` |
| linear_svc | true f32 | 0.1 | `0.573346` | `0.549910` | — | — |
| linear_svc | official pure-f32 | 0.1 | `0.573346` | `0.549910` | `0` | `[0,0]` |
| linear_svc | official int3 | 1 | `0.571429` | `0.548302` | `−0.001486` | `[−0.028257,+0.025510]` |
| linear_svc | official int4 | 1 | `0.573346` | `0.548644` | `−0.001357` | `[−0.023225,+0.020539]` |
| linear_svc | official rescan-k2 | 10 | `0.574305` | `0.549872` | `+0.000042` | `[−0.029487,+0.029020]` |

CoLA L6 没有任何量化臂转正：logreg 的 `int3/int4/rescan` 都在 `−0.59pp`
到 `−1.25pp` 之间，SVC 的 `rescan` 只有 `+0.004pp`、等于零；所有 CI
都跨 0。官方 `pure-f32` 与真 f32 的 logreg 差值 `+0.40pp` 也只是
舍入导致的预测翻转，不是 f32 内部的“增益”。M1.32 补跑全量
`true-bits4` 后，CoLA L6 与官方 `int4` 逐条预测相同（§11.32）。

### 11.31.4 Hate L12（主指标 accuracy）

| probe | arm | C | accuracy | macro-F1 | Δ vs true f32 | 95% CI |
|---|---|---:|---:|---:|---:|---:|
| logreg | true f32 | 1 | `0.574250` | `0.565499` | — | — |
| logreg | true bits4 | 10 | `0.565472` | `0.556540` | `−0.008816` | `[−0.019020,+0.001097]` |
| logreg | official pure-f32 | 1 | `0.574250` | `0.565499` | `0` | `[0,0]` |
| logreg | official int3 | 10 | `0.564740` | `0.553275` | `−0.009601` | `[−0.022312,+0.003658]` |
| logreg | official int4 | 10 | `0.565472` | `0.556540` | `−0.008816` | `[−0.019020,+0.001097]` |
| logreg | official R12 | 1 | `0.560717` | `0.549522` | `−0.013568` | `[−0.026335,−0.000732]` |
| linear_svc | true f32 | 0.1 | `0.571324` | `0.562138` | — | — |
| linear_svc | true bits4 | 1 | `0.566569` | `0.557233` | `−0.004826` | `[−0.015362,+0.005852]` |
| linear_svc | official pure-f32 | 0.1 | `0.571324` | `0.562138` | `0` | `[0,0]` |
| linear_svc | official int3 | 0.1 | `0.553767` | `0.541568` | `−0.017598` | `[−0.029993,−0.004755]` |
| linear_svc | official int4 | 1 | `0.566569` | `0.557233` | `−0.004826` | `[−0.015362,+0.005852]` |
| linear_svc | official R12 | 0.1 | `0.557791` | `0.545421` | `−0.013585` | `[−0.025969,−0.001454]` |

Hate L12 的所有量化臂都低于真 f32。`R12` 在 logreg/SVC 上分别
`−1.36pp / −1.36pp`，两个 CI 都不含 0；`int3` 在 SVC 上
`−1.76pp`、CI 整段为负；`int4` 为 `−0.88pp / −0.48pp`，CI 跨 0。
这与 M1.15 的固定 `C=1` 读数方向一致；M1.15 的 `int4−f32` 为
`−0.37pp`，M1.31 的 per-arm 选 C 为 `−0.88pp`，差异来自 C 选择，
不能把两表混用。

### 11.31.5 Hate L6（主指标 accuracy）

| probe | arm | C | accuracy | macro-F1 | Δ vs true f32 | 95% CI |
|---|---|---:|---:|---:|---:|---:|
| logreg | true f32 | 1 | `0.546086` | `0.534408` | — | — |
| logreg | official pure-f32 | 1 | `0.546086` | `0.534408` | `0` | `[0,0]` |
| logreg | official int3 | 1 | `0.550475` | `0.538579` | `+0.004218` | `[−0.008778,+0.017191]` |
| logreg | official int4 | 10 | `0.531090` | `0.516935` | `−0.015084` | `[−0.024872,−0.005486]` |
| logreg | official rescan | 10 | `0.546452` | `0.531903` | `+0.000282` | `[−0.014265,+0.014265]` |
| linear_svc | true f32 | 0.1 | `0.546452` | `0.534505` | — | — |
| linear_svc | official pure-f32 | 0.1 | `0.546452` | `0.534505` | `0` | `[0,0]` |
| linear_svc | official int3 | 0.1 | `0.547184` | `0.533635` | `+0.000582` | `[−0.012070,+0.013533]` |
| linear_svc | official int4 | 1 | `0.529261` | `0.514096` | `−0.017248` | `[−0.027067,−0.007681]` |
| linear_svc | official rescan | 1 | `0.544623` | `0.529190` | `−0.001922` | `[−0.016094,+0.011704]` |

Hate L6 的 `int4` 在两个 probe 上都是显著负向：logreg `−1.51pp`
（CI `[−2.49,−0.55]`），SVC `−1.72pp`（CI `[−2.71,−0.77]`）。
`int3` 的点估计略正（`+0.42pp / +0.06pp`），但 CI 跨 0；`rescan`
基本持平。这里没有任何“4-bit 正则化提升”的证据。

### 11.31.6 结论与边界

- **CoLA/hate 全臂没有显著正向**：四个数据集×深度组合里，没有任何
  量化臂的主指标 CI 整段大于 0。唯一正向的 CoLA L12 `int4/bits4`
  也只有 `+1.24pp`（logreg macro）/`+0.93pp`（SVC），CI 都跨 0。
- **Hate 的负向更硬**：L12 `R12` 两个 probe 的 CI 都不含 0，SVC `int3`
  也不含 0；L6 `int4` 两个 probe 的 CI 都不含 0。`R12`/`rescan` 的
  混合预算没有转成分类增益。
- **和 SST-2 的关系**：SST-2 的 `int4−f32` 在真 f32 下也不显著
  （`−0.26pp`，§11.29）；唯一显著的 `+4.22pp` 是相对 ternary 锚点。
  因此“SST-2 的 int4 正向”不能被写成跨任务规律，也不能用 CoLA L12
  的正向点估计复活它。
- **口径差异**：M1.15 hate L12 主表是固定 `C=1`；M1.31 是 per-arm
  5 折选 C，故 `int4−f32` 从 `−0.37pp` 变为 `−0.88pp`。两表方向一致，
  但只能各自引用，不能混算。
- 边界：只跑 logreg + LinearSVC；没有 MLP、没有 encoder 微调、没有
  MNLI。L12 有独立 true bits4；L6 的 bits4 臂在 M1.31 用官方 `int4`，
  M1.32 已全量确认该文件就是真 bits4 并补跑 true-bits4，结论不变
  （§11.32）。

完整 80 行见 `data/wsp-classify-cola-hate-full-arms.tsv`；原始评测 JSON
为 `cola-l12-full-arms-eval.json`、`cola-l6-full-arms-eval.json`、
`hate-l12-full-arms-eval.json`、`hate-l6-full-arms-eval.json`。

## 11.32 L6 int4 全量逐位复刻与 true-bits4 分类臂（M1.32）

### 11.32.1 目标与做法

M1.30 只验证了 CoLA/hate L6 官方 `l6-int4` 的前 512 行；M1.31 的 L6
`int4` 臂因此仍带“官方 int4 只抽检首块”的边界。M1.32 把这条边界补成
全量：

- 用 `all-MiniLM-L6-v2` 读取官方同一批 `chunks-l6-1500` 的
  `input.f32` / `mask.u8`，按官方 4-bit 量化路径（`bits=4`、`group=32`、
  半远离零取整、clamp ±7）重新前向，生成全量 true bits4。
- CoLA 9,594 行、hate 11,719 行全部逐行比对官方 `l6-int4` 与 true bits4；
  同时比对两边到真 f32 的距离，确认 4-bit 量化本身没有变。
- 把 `true-bits4` 作为独立臂补进 M1.31 的 L6 分类矩阵，仍用 per-arm
  5 折选 C、test 只评一次、10k paired bootstrap、logreg + LinearSVC。

### 11.32.2 全量逐位对齐

| dataset | n | official int4 vs true bits4 余弦 | max abs diff | 行数 ≤`1e-6` | official int4 vs true f32 余弦 |
|---|---:|---:|---:|---:|---:|
| CoLA | 9,594 | `1.00000000` | `1.64e-7` | 9,594 / 9,594 | `0.95551401` |
| Hate | 11,719 | `1.00000000` | `1.53e-7` | 11,719 / 11,719 | `0.95308453` |

两套的余弦 min/max 都是 `0.99999988 / 1.00000024`，逐行 max abs 的
p99 分别为 `1.15e-7` / `1.12e-7`。这说明官方 `l6-int4` 不是只在前 512
行对齐，后面也没有换量化路径或掉回 ternary；M1.30 的抽检可以升级为
全量结论。

### 11.32.3 L6 分类矩阵（true-bits4 独立臂）

| dataset | probe | arm | C | accuracy | macro-F1 | Δ vs true f32（主指标） | 95% CI |
|---|---|---|---:|---:|---:|---:|---:|
| CoLA | logreg | true f32 | 1 | `0.577181` | `0.553124` | — | — |
| CoLA | logreg | true bits4 | 1 | `0.568552` | `0.543789` | `−0.009351` | `[−0.027439,+0.008747]` |
| CoLA | logreg | official int4 | 1 | `0.568552` | `0.543789` | `−0.009351` | `[−0.027439,+0.008747]` |
| CoLA | linear_svc | true f32 | 0.1 | `0.573346` | `0.549910` | — | — |
| CoLA | linear_svc | true bits4 | 1 | `0.573346` | `0.548644` | `−0.001357` | `[−0.023225,+0.020539]` |
| CoLA | linear_svc | official int4 | 1 | `0.573346` | `0.548644` | `−0.001357` | `[−0.023225,+0.020539]` |
| Hate | logreg | true f32 | 1 | `0.546086` | `0.534408` | — | — |
| Hate | logreg | true bits4 | 10 | `0.530724` | `0.516371` | `−0.015452` | `[−0.025238,−0.005852]` |
| Hate | logreg | official int4 | 10 | `0.531090` | `0.516935` | `−0.015084` | `[−0.024872,−0.005486]` |
| Hate | linear_svc | true f32 | 0.1 | `0.546452` | `0.534505` | — | — |
| Hate | linear_svc | true bits4 | 1 | `0.529261` | `0.514096` | `−0.017248` | `[−0.027067,−0.007681]` |
| Hate | linear_svc | official int4 | 1 | `0.529261` | `0.514096` | `−0.017248` | `[−0.027067,−0.007681]` |

CoLA 的 `true-bits4` 与官方 `int4` 在两个 probe 上逐条预测完全相同；
hate 的 LinearSVC 也完全相同，logreg 有 11/2,734 条预测翻转，净 accuracy
只差 1 条（`−0.000368`，CI `[−0.002560,+0.001829]`）。这点差异来自
`1.5e-7` 量级的 embedding 舍入，不是标签或量化路径错误。

### 11.32.4 结论

- **全量溯源闭合**：CoLA/hate 的 L6 官方 `int4` 就是真 bits4，M1.31 的
  L6 `int4` 臂不再依赖 512 行抽检。L12 已在 M1.26 全量对齐；至此
  SST-2 / CoLA / hate × L12 / L6 六个格子的官方 `int4` 都有全量逐位
  证据。
- **分类结论不变**：CoLA L6 true bits4 相对真 f32 为 `−0.94pp`（logreg）
  /`−0.14pp`（SVC），CI 都跨 0；hate L6 为 `−1.55pp`（logreg）
  /`−1.72pp`（SVC），两个 CI 都整段为负。`true-bits4` 没有把任何一格
  从负向救回正向。
- **M1.31 的 L6 数字不需要改**：官方 int4 与 true bits4 的预测几乎相同，
  hate logreg 的 1 条净差异不改变方向；M1.31 表里的 `−1.51pp`/`−1.72pp`
  仍是有效读数。
- 边界：仍只跑 logreg + LinearSVC，冻结编码器，不做 MLP、不做微调、
  不碰 MNLI；全量 bits4 只验证官方 4-bit 路径，不改变 M1.31 的全臂口径。

`data/wsp-classify-baseline-audit.tsv` 的两条 L6 provenance 行已从
`n=512` 更新为全量，并新增 20 行 L6 true-bits4 分类读数。

## 11.33 跨任务判决：36 个量化比较里 0 个显著正向（M1.33）

### 11.33.1 统计分母

M1.33 不再训练新模型，只把已有结果按统一规则合成。每个
`dataset×depth×probe×量化档` 只算一次，不把同一格式的官方/真值两份
实现重复计数：

- 12 层：3 个数据集 × 2 个 probe × 3 档（`int4`/`int3`/`R12`）= 18。
  `int4` 用独立 true bits4（SST-2/CoLA/hate 都有）；`official-int4`
  仅作溯源，不单独计入。
- 6 层：SST-2 2 个 probe × 3 档（`true-bits4`/`int3`/`rescan-k2`）= 6；
  CoLA/hate 各 2 个 probe × 3 档（`official-int4`/`int3`/`rescan`）= 12。
  CoLA/hate 的官方 `int4` 已在 M1.32 全量确认为真 bits4，因此作为该格
  的 `int4` 代表。
- 合计 36 个比较。每个比较都是同一 test split、同一 probe、同一主指标
  的 `量化 − 真 f32`，10k paired bootstrap CI；C 只在 train 内 5 折选。
- `official-pure-f32` 不计入：它是基线标签/溯源对象，不是量化档。

判定：CI 整段 `>0` 记为“显著正”，整段 `<0` 记为“显著负”，其余记为
“跨 0”。CI 跨 0 只说明没有统计分离，不等于证明等价。

### 11.33.2 逐格汇总

| dataset | depth | 比较数 | 显著正 | 显著负 | 跨 0 | 最大点估计（主指标） | 95% CI |
|---|---|---:|---:|---:|---:|---:|---:|
| SST-2 | L12 | 6 | 0 | 4 | 2 | `−0.001040`（SVC true bits4） | `[−0.010434,+0.008237]` |
| SST-2 | L6 | 6 | 0 | 0 | 6 | `−0.001632`（SVC int3） | `[−0.016474,+0.013180]` |
| CoLA | L12 | 6 | 0 | 0 | 6 | `+0.011989`（logreg true bits4） | `[−0.007385,+0.031764]` |
| CoLA | L6 | 6 | 0 | 0 | 6 | `+0.000042`（SVC rescan-k2） | `[−0.029487,+0.029020]` |
| Hate | L12 | 6 | 0 | 3 | 3 | `−0.004826`（SVC true bits4） | `[−0.015362,+0.005852]` |
| Hate | L6 | 6 | 0 | 2 | 4 | `+0.004218`（logreg int3） | `[−0.008778,+0.017191]` |
| **合计** | | **36** | **0** | **9** | **27** | | |

“最大点估计”即使为负也照列，避免只挑正向臂。六个格子里没有任何一个
量化档的 CI 整段大于 0；CoLA L12 的 `+1.20pp` 是唯一接近正向的点估计，
但 CI 仍跨 0。

### 11.33.3 九个显著负向

| dataset | depth | probe | arm | 主指标 | Δ vs true f32 | 95% CI |
|---|---|---|---|---:|---:|---:|
| SST-2 | L12 | logreg | official-int3 | accuracy | `−0.019732` | `[−0.035146,−0.004942]` |
| SST-2 | L12 | logreg | official-R12 | accuracy | `−0.022967` | `[−0.038440,−0.007688]` |
| SST-2 | L12 | linear_svc | official-int3 | accuracy | `−0.024053` | `[−0.040088,−0.008237]` |
| SST-2 | L12 | linear_svc | official-R12 | accuracy | `−0.028556` | `[−0.043932,−0.013180]` |
| Hate | L12 | logreg | official-R12 | accuracy | `−0.013568` | `[−0.026335,−0.000732]` |
| Hate | L12 | linear_svc | official-int3 | accuracy | `−0.017598` | `[−0.029993,−0.004755]` |
| Hate | L12 | linear_svc | official-R12 | accuracy | `−0.013585` | `[−0.025969,−0.001454]` |
| Hate | L6 | logreg | official-int4 | accuracy | `−0.015084` | `[−0.024872,−0.005486]` |
| Hate | L6 | linear_svc | official-int4 | accuracy | `−0.017248` | `[−0.027067,−0.007681]` |

负向集中在 SST-2 L12 的 3-bit/混合预算，以及 hate L12 的 `R12`、hate
L6 的 `int4`。这些不是“量化没提升”，而是**统计上更差**。

### 11.33.4 判决

- **分类头没有可提升的量化档**：36 个比较里 0 个显著正、9 个显著负、
  27 个跨 0。f32 应继续作为分类头的默认锚点；`int3`/`int4`/`R12`/`rescan`
  都不应因为“可能正则化”被提升为默认。
- **SST-2 的 int4 正向不存在**：SST-2 L12 的 `true-bits4` 是
  `−0.10pp`（SVC）/`−0.15pp`（logreg），CI 跨 0；历史上 `+4.22pp`
  是 ternary 锚点到 4-bit 的恢复，不是 f32→int4。SST-2 L12 的
  `int3`/`R12` 反而是显著负向。
- **CoLA L12 是唯一的“正向方向”**：`true-bits4`/`int4` 的 `+1.20pp`
  （logreg macro）和 `+0.93pp`（SVC）都跨 0。它是点估计，不是可发布
  的增益；CoLA L6 也没有转正。
- **Hate 的负向更硬**：L12 `R12` 两个 probe 都显著负，L12 SVC `int3`
  也显著负；L6 `int4` 两个 probe 都显著负。混合预算在 hate 上没有
  分类优势。
- 边界：3 个数据集、2 个深度、2 个线性头；冻结编码器；没有 MLP、
  没有 encoder 微调、没有 MNLI。跨数据集的主指标不同（SST-2/hate
  用 accuracy，CoLA 用 macro-F1），所以只比“同格内相对 f32 的符号与
  CI”，不把跨数据集的百分点直接相加。

完整 36 行见 `data/wsp-classify-cross-task-verdict.tsv`。M1.33 只做
合成，不新增训练；原始读数仍来自 SST-2 全臂 TSV、CoLA/hate 全臂
TSV 和 M1.32 的 L6 bits4 复验。

> **M1.34 复验（重要）**：上面的 `0 正 / 9 负 / 27 跨 0` 是 seed 7
> 单次 C 选择的结果，不是 5-seed 稳健计数。换 5 个 CV seed 后，
> 29/36 点估计同号、7 个翻号；9 个负向只有 6 个在 5/5 seed 仍为负；
> Hate L6 `logreg int3` 在 4/5 seed 变成显著正向，但 matched-C 控制
> 下 CI 跨 0。引用本节必须带“seed 7 条件”。见 §11.34。

## 11.34 C 选择敏感性：36 个判决不是稳健计数（M1.34）

### 11.34.1 问题

M1.33 的 36 个比较固定了 **一个** CV seed（`7`）：每个 arm 在 train 内
用 `StratifiedKFold(5, shuffle=True, random_state=7)` 选 C，test 只评一次；
bootstrap 只重采样 test，不覆盖 C 选择。换一个 CV seed，选出的 C 可能
变化，量化档与 f32 的相对位置也可能变化。M1.34 问的就是：

> M1.33 的 `0 正 / 9 负 / 27 跨 0` 是分类头的稳健判决，还是 seed 7
> 单次 C 选择的结果？

### 11.34.2 口径

- 固定 test，只重做 train 内 5 折 **C 选择**。5 个 seed：`7/13/19/31/43`，
  C grid 仍是 `(0.01, 0.1, 1.0, 10.0)`；每个 arm 独立选 C，再在完整
  train 上重训、test 上评一次。
- 36 个比较 × 5 seed = 180 个点估计；全部跑完。点估计的符号稳定性和
  极差见 `data/wsp-classify-cv-seed-stability.tsv`（180 行）与
  `data/wsp-classify-cv-seed-summary.tsv`（36 行）。
- 其中 24 个比较（SST-2 L12、CoLA L12、Hate L12、Hate L6）另做 5 套
  10k paired bootstrap（同一 test 重采样索引同时用于 arm 与 f32），
  共 120 行，见 `data/wsp-classify-cv-seed-ci.tsv`。SST-2 L6、CoLA L6
  只做点符号，不重算 CI。
- 还做了一组 **matched-C** 控制：Hate L6 logreg 的 f32/int3/int4/rescan
  在相同 C（0.1/1/10）下重训，见
  `data/wsp-classify-hate-l6-common-c.tsv`（9 行）。
- 口径修正：M1.33 的 `delta` 列实际是 **bootstrap 均值**，不是点估计；
  §11.33.2 的“最大点估计”应读作“最大 bootstrap 均值”。M1.34 同时报
  `point_delta` 与 `boot_mean`。CoLA L6 SVC `rescan-k2` 的
  `+0.000042` 就是 bootstrap 均值，点估计是 `−0.000038`，不能算正向。

### 11.34.3 点符号稳定性

| dataset | depth | 比较数 | 5 seed 同号 | 翻号 |
|---|---|---:|---:|---:|
| SST-2 | L12 | 6 | 4 | 2 |
| SST-2 | L6 | 6 | 6 | 0 |
| CoLA | L12 | 6 | 6 | 0 |
| CoLA | L6 | 6 | 4 | 2 |
| Hate | L12 | 6 | 6 | 0 |
| Hate | L6 | 6 | 3 | 3 |
| **合计** | | **36** | **29** | **7** |

7 个翻号的是：SST-2 L12 的 `true-bits4` 两个 probe（幅度只有
`±0.1pp`）；CoLA L6 的 `int4` 两个 probe；Hate L6 的 `int4` 两个
probe；Hate L6 SVC `rescan`。**29/36 同号**说明多数比较的方向不是单次
C 选择的偶然，但 7 个翻号已经足够让“所有负向都稳健”失效。

### 11.34.4 M1.33 的 9 个显著负向：只有 6 个在 5/5 seed 仍为负

| dataset | depth | probe | arm | seed 7 Δ / CI | 5 seed 的 CI 判决 |
|---|---|---|---|---:|---|
| SST-2 | L12 | logreg | official-int3 | `−0.019732` `[−0.035146,−0.004942]` | 5/5 负 |
| SST-2 | L12 | logreg | official-R12 | `−0.022967` `[−0.038440,−0.007688]` | 5/5 负 |
| SST-2 | L12 | linear_svc | official-int3 | `−0.024053` `[−0.040088,−0.008237]` | 5/5 负 |
| SST-2 | L12 | linear_svc | official-R12 | `−0.028556` `[−0.043932,−0.013180]` | 5/5 负 |
| Hate | L12 | logreg | official-R12 | `−0.013568` `[−0.026335,−0.000732]` | 5/5 负 |
| Hate | L12 | linear_svc | official-int3 | `−0.017598` `[−0.029993,−0.004755]` | 4 负 + 1 跨 |
| Hate | L12 | linear_svc | official-R12 | `−0.013585` `[−0.025969,−0.001454]` | 5/5 负 |
| Hate | L6 | logreg | official-int4 | `−0.015084` `[−0.024872,−0.005486]` | 1 负 + 3 跨 + 1 正 |
| Hate | L6 | linear_svc | official-int4 | `−0.017248` `[−0.027067,−0.007681]` | 2 负 + 3 跨 |

- **6/9 在 5/5 seed 的 CI 仍整段为负**：SST-2 L12 的四个
  `int3`/`R12`，以及 Hate L12 两个 probe 的 `R12`。这些负向没有随
  C 选择翻掉。
- Hate L12 SVC `int3` 在 4/5 seed 为负、seed 43 变跨 0；方向仍是负，
  但不能再写“每个 seed 都显著”。
- **Hate L6 的两个 `int4` 臂不再稳定负向**：logreg 在 seed 7 是
  `−1.50pp`（CI 全负），seed 13/19/31 变跨 0，seed 43 变
  `+1.17pp`（CI `[+0.001463,+0.021946]`，全正）；SVC 在 seed 7/13
  为负，seed 19/31/43 跨 0。**“Hate L6 int4 显著负向”是 seed 7 的
  条件结论，不是稳健结论。**

### 11.34.5 M1.33 的 6 个正向：5 个稳定，1 个变成显著正向

| dataset | depth | probe | arm | seed 7 Δ / CI | 5 seed |
|---|---|---|---|---:|---|
| CoLA | L12 | logreg | true-bits4 | `+0.011989` `[−0.007385,+0.031764]` | 5/5 跨 0，点 5/5 正 |
| CoLA | L12 | linear_svc | true-bits4 | `+0.009261` `[−0.009135,+0.027831]` | 5/5 跨 0，点 5/5 正 |
| Hate | L6 | logreg | official-int3 | `+0.004218` `[−0.008778,+0.017191]` | 1 跨 + 4 正，点 5/5 正 |
| Hate | L6 | linear_svc | official-int3 | `+0.000582` `[−0.012070,+0.013533]` | 5/5 跨 0，点 5/5 正 |
| Hate | L6 | logreg | official-rescan | `+0.000282` `[−0.014265,+0.014265]` | 5/5 跨 0，点 5/5 正 |
| CoLA | L6 | linear_svc | official-rescan-k2 | `+0.000042`（boot mean） | 点 0/5 正、5/5 负；均值 `−0.011901` |

- CoLA L12 的 `int4` 正向在 5 个 seed 上点估计全正，但 **5/5 CI 都跨 0**；
  它仍然只是点估计，不是显著增益。
- Hate L6 的 `logreg int3` 是唯一在多个 seed 上变成显著正向的比较：
  seed 13/19/31/43 都是 `+1.7191pp`，CI `[+0.003658,+0.031090]`。
- Hate L6 SVC `int3` 点估计 5/5 正，但 CI 5/5 跨 0；`logreg rescan`
  同样 5/5 跨 0。
- CoLA L6 SVC `rescan-k2` 的 `+0.000042` 是 bootstrap 均值；点估计
  seed 7 已是 `−0.000038`，5 seed 点估计全负，均值 `−0.011901`。
  **这个“正向”不能计入 6 个正向点估计。**

### 11.34.6 关键反转是 C 选择，不是量化增益

Hate L6 `logreg int3` 的 4/5 显著正向来自 f32 基线的 C 选择：

- seed 7：f32 C=1，test acc `0.546086`；int3 C=1，`0.550475`；
  Δ `+0.004389`，CI 跨 0。
- seed 13/19/31/43：f32 C=10，test acc `0.533285`；int3 C=1，
  `0.550475`；Δ `+0.017191`，CI 全正。

也就是说，正向不是 int3 变好，而是 **f32 在 C=10 上掉了 1.28pp**。
matched-C 控制把这一点拆开：

| C | f32 acc | int3 acc | int3−f32 | 95% CI |
|---:|---:|---:|---:|---:|
| 0.1 | `0.555596` | `0.568032` | `+0.012436` | `[−0.001829,+0.026335]` |
| 1 | `0.546086` | `0.550475` | `+0.004389` | `[−0.008778,+0.017191]` |
| 10 | `0.533285` | `0.534748` | `+0.001463` | `[−0.012070,+0.014996]` |

三个 C 下 int3 都只有点估计优势，**CI 都跨 0**。per-arm C 的
`+1.71pp` 全正 CI 是“f32 选 C=10、int3 选 C=1”的错配结果，不是
“int3 在 matched-C 下更好”。

同样的错配也解释了 Hate L6 `int4` 的负向翻转：

- seed 7：f32 C=1 `0.546086`，int4 C=10 `0.531090`，Δ `−0.015084`，
  CI 全负；matched C=1 时 int4 只有 `−0.001097`，CI 跨 0。
- seed 43：f32 C=10 `0.533285`，int4 C=1 `0.544989`，Δ `+0.011704`，
  CI 全正；matched C=10 时 int4 只有 `−0.002195`，CI 跨 0。

**两个方向的“显著”都可以由 C 选择制造出来；matched-C 下都消失。**

### 11.34.7 判决

- M1.33 的 seed 7 算术没有错，但 `0 正 / 9 负 / 27 跨 0` **不是稳健
  计数**，只能写成“seed 7 的 C 选择下”的判决。
- 9 个显著负向里，6 个在 5/5 seed 仍为负；Hate L6 的两个 `int4`
  不再稳定负向。不要把 M1.33 的 9 个负向整体当作“已证负向”。
- 6 个正向点估计里，5 个点符号稳定；CoLA L6 SVC `rescan-k2` 是
  bootstrap 均值伪影。唯一在多个 seed 上显著正向的是 Hate L6
  `logreg int3`，但 matched-C 下 CI 跨 0，不能算量化增益。
- **matched-C 控制（§11.35）仍然支持原结论：没有量化档稳定超过真 f32。**
  M1.34
  只做了 Hate L6 logreg 的 3 个 C 下比较，Δ 的 CI 全跨 0；M1.35 已把
  matched-C 扩到全部 36 个比较、5 个 seed（180 个 CI），仍然 0 个显著
  正向。但“0 个显著正向”不能作为跨 C 选择的强断言；M1.33 的 `0/9/27`
  是 seed 7 的条件读数。分类头继续用 f32 锚点，引用 M1.33 必须带这个
  条件。
- 方法边界：5 个 seed 不是独立重复，C 选择之间高度相关；CI 只重采样
  test，不覆盖 train 侧方差；只有 3 个数据集、2 个深度、2 个线性头；
  没有 MLP、没有 encoder 微调、没有 MNLI；24/36 个比较有 5-seed CI，
  SST-2 L6 与 CoLA L6 只做点符号。M1.34 的 matched-C 只做了 Hate L6
  logreg；全矩阵 matched-C 见 §11.35，仍不是 nested CV。

### 11.34.8 复算

```bash
# 5 seed 点估计（每个 case 单独落盘，避免长任务中断）
python geometry/cv_seed_stability.py --dataset hate --depth l6
python geometry/cv_seed_stability.py --dataset cola --depth l12

# 24 个比较的 5-seed paired CI
python geometry/cv_seed_ci.py --dataset hate --depth l6
python geometry/cv_seed_ci.py --dataset cola --depth l12

# 合并与表格
python geometry/make_cv_seed_tsv.py
python geometry/make_cv_seed_ci_tsv.py
```

表见 `data/wsp-classify-cv-seed-stability.tsv`（180 行）、
`data/wsp-classify-cv-seed-summary.tsv`（36 行）、
`data/wsp-classify-cv-seed-ci.tsv`（120 行）、
`data/wsp-classify-hate-l6-common-c.tsv`（9 行）。

## 11.35 matched-C 全矩阵：0 个显著正向，11 个比较出现过负向（M1.35）

### 11.35.1 问题

M1.34 已经证明 per-arm C 会把 Hate L6 `logreg int3` 的 4/5 seed 做成
显著正向，但它只对 Hate L6 logreg 做了 3 个 C 的 matched-C 控制。剩下
的关键缺口是：

> 如果把 matched-C 扩到全部 36 个比较、5 个 CV seed，是否还存在任何
> 量化档在共同 C 下显著超过真 f32？M1.34 的“0 个正向”是不是只因为
> 控制做得太少？

M1.35 不再做新的臂，只重训现有臂；问题是 C 口径，不是模型或数据。

### 11.35.2 口径

- 对每个 `dataset×depth×probe×seed`，只在 **真 f32** 上做
  `StratifiedKFold(5, shuffle=True, random_state=seed)`，用与 M1.31
  相同的主指标（SST-2/hate accuracy，CoLA macro-F1；CoLA 仍
  `class_weight='balanced'`）选 C，grid 仍为 `(0.01, 0.1, 1.0, 10.0)`。
- 选定后，f32 与同 case 的全部量化臂（L12：`true-bits4`、`official-int3`、
  `official-R12`；L6：`official-int4`、`official-int3`、`official-rescan`）
  **共用这个 C**，在完整 train 上重训，test 只评一次。量化臂不再单独选 C。
- 每个量化臂对 f32 做 10k paired bootstrap（同一组 test 重采样索引同时
  用于臂与 f32），得到 `point_delta`、`boot_mean`、95% CI 与
  `positive/negative/cross`。
- 5 个 seed：`7/13/19/31/43`。36 个比较 ×5 = **180 个 CI**；不是只报
  seed 7。C 由 f32 选，仍可能随 seed 变化，但同一 seed 内所有臂共享。
- 这是 **f32-matched C**，不是“量化臂各自最优 C”的估计。它回答的是：
  “在 f32 的 CV 工作点上，量化是否更好”；M1.34 的 per-arm 结果回答的是
  “各自调 C 后是否更好”。两个口径必须并列，不能混表。

### 11.35.3 总判决：0 正 / 41 负 / 139 跨 0

180 个 seed×比较的 CI 判决：

| 判决 | 个数 |
|---|---:|
| CI 整段 `>0`（显著正向） | **0** |
| CI 整段 `<0`（显著负向） | **41** |
| CI 跨 0 | **139** |
| 合计 | **180** |

36 个比较里，**没有任何一个比较在 5 个 seed 中出现过显著正向**
（`any_positive = 0`）。分 case：

| dataset | depth | 比较数 | 5-seed CI 数 | 正 | 负 | 跨 0 |
|---|---|---:|---:|---:|---:|---:|
| SST-2 | L12 | 6 | 30 | 0 | 20 | 10 |
| SST-2 | L6 | 6 | 30 | 0 | 0 | 30 |
| CoLA | L12 | 6 | 30 | 0 | 0 | 30 |
| CoLA | L6 | 6 | 30 | 0 | 5 | 25 |
| Hate | L12 | 6 | 30 | 0 | 16 | 14 |
| Hate | L6 | 6 | 30 | 0 | 0 | 30 |
| **合计** | | **36** | **180** | **0** | **41** | **139** |

- 所有 case 的最大 5-seed 平均点估计都没有显著正向：SST-2 L12
  `−0.000879`（SVC `true-bits4`）、SST-2 L6 `−0.003295`（logreg `int3`）、
  CoLA L12 `+0.012073`（logreg `true-bits4`，5/5 跨 0）、CoLA L6
  `−0.008693`（logreg `int4`）、Hate L12 `−0.002560`（logreg
  `true-bits4`）、Hate L6 `+0.010534`（logreg `rescan`，5/5 跨 0）。
- **CoLA L12 仍是唯一点估计为正的 case**：`true-bits4`/`int4` 的
  `+1.20pp` 在共同 C=10 下仍只有点估计，5/5 CI 跨 0。它没有因为
  matched-C 变成显著正向。
- **Hate L6 的显著正向消失**：`logreg int3` 在 seed 7 为
  `+0.004389`（CI 跨 0），seed 13/19/31/43 为 `+0.001463`（CI 跨 0）；
  5/5 都没有正向 CI。M1.34 的 `+1.7191pp` 是 f32 选 C=10、int3 选 C=1
  的 per-arm 错配，matched-C 下不存在。

### 11.35.4 11 个比较出现过至少一次负向 CI

41 个负向 CI 集中在 11 个比较上；另外 25 个比较在 5 个 seed 全跨 0：

| dataset | depth | probe | arm | 负向 seed 数 | 5-seed 平均 Δ | C_by_seed |
|---|---|---|---|---:|---:|---|
| SST-2 | L12 | logreg | official-int3 | 5/5 | `−0.020428` | 1,1,1,1,10 |
| SST-2 | L12 | logreg | official-R12 | 5/5 | `−0.026469` | 1,1,1,1,10 |
| SST-2 | L12 | linear_svc | official-int3 | 5/5 | `−0.019330` | 0.1,0.1,0.1,0.1,1 |
| SST-2 | L12 | linear_svc | official-R12 | 5/5 | `−0.027677` | 0.1,0.1,0.1,0.1,1 |
| CoLA | L6 | logreg | official-rescan-k2 | 3/5 | `−0.021961` | 1,10,1,10,10 |
| CoLA | L6 | linear_svc | official-int3 | 1/5 | `−0.014609` | 0.1,1,0.1,10,10 |
| CoLA | L6 | linear_svc | official-int4 | 1/5 | `−0.014276` | 0.1,1,0.1,10,10 |
| Hate | L12 | logreg | official-R12 | 5/5 | `−0.013533` | 1,1,1,1,1 |
| Hate | L12 | linear_svc | official-int3 | 5/5 | `−0.017045` | 0.1,1,0.1,0.1,0.1 |
| Hate | L12 | linear_svc | official-R12 | 5/5 | `−0.014411` | 0.1,1,0.1,0.1,0.1 |
| Hate | L12 | linear_svc | true-bits4 | 1/5 | `−0.006730` | 0.1,1,0.1,0.1,0.1 |

- SST-2 L12 的四个 `int3`/`R12` 是 5/5 稳定负向；C 在 seed 43 换到
  1/10 也没有翻正。Hate L12 的 `R12` 与 SVC `int3` 同样 5/5 负向；
  SVC `true-bits4` 只在 seed 13 的 C=1 上负向，方向不稳。
- CoLA L6 的负向只在 C=10（logreg rescan）或 C=1（SVC int3/int4，
  seed 13）出现；C 换到 0.1/10 的其他 seed 都跨 0。**这说明共同 C
  也会把量化臂推到显著更差的位置，不是“量化只是无增益”那么简单。**
- SST-2 L6、CoLA L12、Hate L6 三个 case 的 90 个 CI 全部跨 0。它们
  没有正向，也没有稳定负向。Hate L6 的唯一接近显著的是
  `logreg rescan`。

### 11.35.5 最接近的一格：Hate L6 logreg rescan 也没有过 0

Hate L6 `logreg rescan` 在 C=10 下是 `0.533285 → 0.546452`，
Δ `+0.013168`，CI `[−0.000366,+0.026701]`，**下界离 0 只差
0.000366（0.0366pp），但仍跨 0**。同一条臂在共同 C 下：

| C | f32 acc | rescan acc | rescan−f32 | 95% CI |
|---:|---:|---:|---:|---:|
| 0.1 | `0.555596` | `0.564740` | `+0.009144` | `[−0.005121,+0.023043]` |
| 1 | `0.546086` | `0.546086` | `0.000000` | `[−0.013899,+0.013168]` |
| 10 | `0.533285` | `0.546452` | `+0.013168` | `[−0.000366,+0.026701]` |

三个 C 下都只存在点估计优势，CI 全部跨 0。C=10 的“接近显著”来自
f32 自己在 test 上从 C=1 的 `0.546086` 掉到 `0.533285`，而 rescan
在 C=10 仍保持 `0.546452`；这不是 rescan 提高了上限。f32 在 C=0.1
的 `0.555596` 高于 rescan 在 C=10 的 `0.546452`；rescan 在 C=0.1
的 `0.564740` 虽然高于 f32，但 Δ 只有 `+0.009144`、CI 仍跨 0。
**结论是“rescan 对坏 C 更稳”，不是“rescan 在共同 C 下超过 f32”。**

> **M1.38 追加（阈值条件）**：上表三个 C 都还是默认 0.5 阈值。把决策阈值换成
> “目标分数分位匹配 train 先验”（不用 test 标签）后，Hate L6 `rescan` 在
> logreg/SVC 各 5/5 显著正（均值 `+2.72/+2.55pp`），L12 `R12` logreg 也
> 5/5 正（`+1.83pp`）。因此“rescan 对坏 C 更稳”仍成立，但它不是排序上限；
> 默认阈值下的“不显著”不能当成“没有排序增益”。见 §11.38。

### 11.35.6 判决

- **matched-C 全矩阵仍是 0 个显著正向。** M1.34 的“Hate L6 `logreg
  int3` 4/5 显著正向”在 f32 共同 C 下完全消失；这不是控制样本太少，
  而是 per-arm C 错配。
- **41/180 个 CI 是显著负向，分布在 11 个比较上；25 个比较在 5 个
  seed 全跨 0。** matched-C 下量化臂既没有稳定增益，也不是一律无损：
  SST-2 L12 的 `int3`/`R12` 和 Hate L12 的 `R12`/SVC `int3` 在共同 C
  下仍 5/5 显著更差。
- **Hate L6 `rescan` 是最接近正向的一格，但三个共同 C 的 CI 全部跨
  0，下界最好也只到 `−0.000366`；不能记作显著增益。** M1.36 已用嵌套
  CV 复验这一格；OOF train 上点估计 10/10 为正，但只有 1/10 的 CI
  整段为正，见 §11.36。**仍不能升级为默认。**
- **分类头到此可以收口：f32 继续是默认锚点；`int3`/`int4`/`R12`/
  `rescan` 都不因“可能正则化”被提升。** SST-2 `int4 > f32` 已被
  M1.26/M1.29 否掉；M1.33 的 `0/9/27` 是 seed 7 条件读数；M1.34 的
  per-arm 显著正向是 C 错配；M1.35 的 matched-C 全矩阵没有正向。
- 方法边界：C 仍由 train 内 5 折选出，可能对 test 次优；5 个 seed 不是
  独立重复；CI 只重采样 test，不覆盖 C 选择与 train 侧方差；不是
  nested CV。只有 3 个数据集、2 个深度、2 个线性头；没有 MLP、没有
  encoder 微调、没有 MNLI。不能把“matched-C 无正向”外推成“任何量化
  格式在任何任务上都不可能有益”。
- **M1.37 追加的跨域提醒**：Hate 的官方 test 相对 train 有约
  `20–22pp` 的 f32 差距，test 内部 5 折仍有 `0.751–0.764`；因此
  §11.35 的 Hate test 读数是跨域读数。Hate L12 `R12` 在 OOF 上正向、
  test 上 5/5 负向，说明该臂的量化效应不跨域；不能把 §11.35 的
  Hate 负向当作 in-domain 量化损失的上界（§11.37）。

> **M1.38 追加（阈值条件）**：Hate 的 20–22pp 差距不是长度/截断（域分类器
> AUC `0.921`、长度单特征 `0.561`），其中约 `6–9pp` 是默认 0.5 阈值在目标域
> 上漂移；校准到 train 先验的目标分数分位后，f32 L12 `0.574→0.634`、
> L6 `0.546→0.634`，校准后仍差 `12–13pp`。全臂 36 比较在 target_prior 下
> 出现 6 个正 CI（全部 Hate），但默认 0.5 仍是 0/36。故本节的“0 正向”
> 是默认阈值 headline，不是“排序上没有任何正向”；`R12`/`rescan` 只能
> 记为条件性候选，不能升级为默认。见 §11.38。

### 11.35.7 复算

```bash
# 6 个 case 的 matched-C 全矩阵（每个 case 单独落盘）
python geometry/matched_c_full.py --dataset sst2 --depth l12
python geometry/matched_c_full.py --dataset sst2 --depth l6
python geometry/matched_c_full.py --dataset cola --depth l12
python geometry/matched_c_full.py --dataset cola --depth l6
python geometry/matched_c_full.py --dataset hate --depth l12
python geometry/matched_c_full.py --dataset hate --depth l6

# 合并 6 个 case、生成 180/36 行表
python geometry/make_matched_c_tsv.py
```

表见 `data/wsp-classify-matched-c-raw.tsv`（180 行数据、12 列：
`dataset depth probe arm metric seed C point_delta boot_mean ci_low
ci_high verdict`）与 `data/wsp-classify-matched-c-summary.tsv`
（36 行数据、14 列：`dataset depth probe arm metric seeds
n_pos_verdict n_neg_verdict n_cross any_positive mean_point_delta
min_point_delta max_point_delta C_by_seed`）。

## 11.36 nested CV 复验 Hate L6 边界格（M1.36）

### 11.36.1 问题

M1.35 的 matched-C 全矩阵里，最接近正向的是 Hate L6 `logreg rescan`
在 C=10：`+1.3168pp`，CI `[−0.0366,+2.6701]pp`，仍跨 0。这个格子有
两个疑点：

1. C=10 是 f32 的 CV 选择，但 f32 在 test 上 C=10 比 C=0.1 低
   `2.23pp`；接近显著可能只是 f32 在坏 C 上掉了，而不是 rescan 变好。
2. M1.35 仍只评一次官方 test；C 选择发生在 train 内，test 不覆盖
   C 选择与训练划分的方差。

M1.36 用 nested CV 复验这个格子：C 选择放进 outer CV 内部，outer
held-out fold 只用来评分。它回答的是：

> 如果完整执行“f32 内层选 C → 所有臂共用 C → 训练 → 预测”这套
> procedure，held-out fold 上 rescan 是否仍稳定优于 f32？

注意：M1.36 评的是 **train 的 OOF**，不是官方 test；它不能替代
M1.35 的 test 读数，也不能和 test 百分点直接混表。

### 11.36.2 口径

- 数据：TweetEval hate L6，train `8,985` 条；官方 test `2,734` 条
  在本实验里完全不用。
- outer：`StratifiedKFold(5, shuffle=True, random_state=outer_seed)`，
  outer seed 为 `7/13/19/31/43`；每个 outer seed 生成一套覆盖全部
  `8,985` 条 train 的 OOF 预测。
- inner：在每个 outer-train 内再做
  `StratifiedKFold(5, shuffle=True, random_state=7)`，只在 **真 f32**
  上选 C；C grid `(0.01,0.1,1.0,10.0)`。选定后 f32 与 `int3`/`int4`/
  `rescan` 共用这个 C，在 outer-train 上重训，预测 outer-val。
- probe：`logreg` 与 `linear_svc`；hate 口径无类别权重，主指标
  accuracy。
- 每个 `probe×outer_seed×arm` 对 f32 做 10k paired bootstrap（同一组
  OOF 重采样索引同时用于臂与 f32）：`2 probes ×5 seeds ×3 arms =
  30` 个 CI；另有 `2×5×5 = 50` 个 outer fold 的点差值。
- 这不是 per-arm C：每个 outer fold 只有一个 f32 选出的 C，所有臂
  共享。`C_by_fold` 与 `fold_deltas` 都落盘，便于复核。

### 11.36.3 总读数：2 正 / 0 负 / 28 跨 0

30 个 CI 的判决：

| 判决 | 个数 |
|---|---:|
| CI 整段 `>0` | **2** |
| CI 整段 `<0` | **0** |
| CI 跨 0 | **28** |
| 合计 | **30** |

两个显著正向都在 `logreg`；SVC 三个臂合计 `0/15`：

| probe | arm | outer seed | Δ | 95% CI | verdict |
|---|---|---:|---:|---:|---|
| logreg | rescan | 7 | `+0.008570`（`+0.857pp`） | `[+0.001558,+0.015582]` | 正 |
| logreg | int3 | 43 | `+0.007902`（`+0.790pp`） | `[+0.001336,+0.014580]` | 正 |

两个下界都只比 0 高 `0.13–0.16pp`；不是大幅超出的强正向。

分 probe/arm 的 5-seed 汇总：

| probe | arm | 5-seed 平均 | sd | min | max | 点估计正 | CI 正/负/跨 |
|---|---|---:|---:|---:|---:|---:|---:|
| logreg | int3 | `+0.467pp` | `0.216pp` | `+0.212pp` | `+0.790pp` | 5/5 | 1/0/4 |
| logreg | int4 | `+0.252pp` | `0.134pp` | `+0.122pp` | `+0.401pp` | 5/5 | 0/0/5 |
| logreg | rescan | `+0.590pp` | `0.189pp` | `+0.345pp` | `+0.857pp` | 5/5 | 1/0/4 |
| linear_svc | int3 | `+0.258pp` | `0.120pp` | `+0.100pp` | `+0.423pp` | 5/5 | 0/0/5 |
| linear_svc | int4 | `+0.114pp` | `0.185pp` | `−0.145pp` | `+0.334pp` | 4/5 | 0/0/5 |
| linear_svc | rescan | `+0.436pp` | `0.216pp` | `+0.100pp` | `+0.634pp` | 5/5 | 0/0/5 |

- **`rescan` 与 `int3` 在两个 probe 的 10 个 outer seed 上点估计全部
  为正**；`int4` 只有 SVC seed 43 一个负点（`−0.145pp`）。
- 但 `rescan` 只有 1/10 个 CI 整段为正，`int3` 也只有 1/10；SVC 的
  三个臂合计 `0/15`。**点方向一致，不等于 5/5 显著。**
- 50 个 outer fold 的点差值也同向但分散：`rescan` 正/负/零为
  `20/5/0`（logreg）与 `19/5/1`（SVC）；`int3` 为 `17/7/1` 与
  `17/8/0`；`int4` 为 `18/6/1` 与 `15/7/3`。没有哪个臂在 fold 级别
  一致为正。

### 11.36.4 为什么 OOF 的正向不能直接搬到官方 test

nested CV 的 f32 OOF accuracy 是 `0.761–0.767`，而官方 test 的 f32
只有 `0.533–0.556`（M1.34/M1.35 的 C=0.1/1/10 读数）。两者差约
`21pp`，说明 hate 的 train 与官方 test 分布差很大。M1.36 的
`+0.44–+0.59pp` 是 **train 分布内** held-out 的弱正向；它不能证明
官方 test 上也会出现同号增益。

这也解释了为什么 M1.35 的 C=10 边界格更大（`+1.32pp`）：那是
official test 上 f32 从 C=0.1 的 `0.555596` 掉到 C=10 的 `0.533285`
造成的；nested CV 的 inner 选 C 不会系统性地把 f32 推到同一个坏 C，
所以效应缩到 `+0.44–+0.59pp`。**M1.35 的“接近显著”主要是 C 选择
造成的放大，M1.36 的 nested CV 把它降回一个弱候选。**

### 11.36.5 判决

- `rescan` 不是纯噪声：nested CV 里 10/10 outer seed 点估计为正、
  50 个 fold 中 39 个为正、0 个负 CI。`int3` 同样是 10/10 点正且
  0 个负 CI；两者在 Hate L6 的官方 matched-C 与 nested CV 两个口径
  下都没有负向 CI。`rescan` 的点估计更大，是唯一在官方 test 上接近
  显著的弱候选。
- 但它也不是可发布增益：30 个 CI 只有 2 个整段为正（且都在
  `logreg`，下界只高 `0.13–0.16pp`），SVC `0/15`，28/30 跨 0；
  按 30 个比较的多重性看，不能把这两格当成稳健正向。
- **分类头最终判决不变：没有量化档稳定超过真 f32；f32 继续是默认
  锚点。** `rescan` 是唯一值得保留的弱候选，但不能提升为默认；它的
  OOF 正向不能替代官方 test 读数。
- 方法边界：nested CV 只覆盖 Hate L6；OOF 与官方 test 分布差约
  `21pp`；outer/inner 各 5 折、5 个 outer seed，仍不是独立重复；
  bootstrap 只重采样 OOF，不覆盖模型训练方差；只有线性头；没有
  MLP、没有 encoder 微调、没有 MNLI。

### 11.36.6 复算

```bash
# 10 个 case（2 probe ×5 outer seed），每个单独落盘
python geometry/nested_cv_hate_l6.py --probe logreg --outer-seed 7
python geometry/nested_cv_hate_l6.py --probe logreg --outer-seed 13
python geometry/nested_cv_hate_l6.py --probe logreg --outer-seed 19
python geometry/nested_cv_hate_l6.py --probe logreg --outer-seed 31
python geometry/nested_cv_hate_l6.py --probe logreg --outer-seed 43
python geometry/nested_cv_hate_l6.py --probe linear_svc --outer-seed 7
python geometry/nested_cv_hate_l6.py --probe linear_svc --outer-seed 13
python geometry/nested_cv_hate_l6.py --probe linear_svc --outer-seed 19
python geometry/nested_cv_hate_l6.py --probe linear_svc --outer-seed 31
python geometry/nested_cv_hate_l6.py --probe linear_svc --outer-seed 43

# 合并 30 个 CI、50 个 fold、6 行汇总
python geometry/make_nested_cv_tsv.py
```

表见 `data/wsp-classify-nested-cv-hate-l6-raw.tsv`（30 行数据、16 列：
`dataset depth probe arm outer_seed inner_seed n_oof f32_acc arm_acc
point_delta boot_mean ci_low ci_high verdict C_by_fold fold_deltas`）、
`data/wsp-classify-nested-cv-hate-l6-folds.tsv`（50 行数据、10 列：
`dataset depth probe outer_seed fold C f32_acc int3_delta int4_delta
rescan_delta`）与 `data/wsp-classify-nested-cv-hate-l6-summary.tsv`
（6 行数据、14 列，含 `n_pos_verdict/n_neg_verdict/n_cross/n_pos_point/
mean_delta/sd_delta/min_delta/max_delta/C_by_seed`）。

## 11.37 nested CV 全矩阵：OOF 正向只在 Hate，且与官方 test 反向（M1.37）

### 11.37.1 问题

M1.36 只对 Hate L6 一个格子做了 nested CV：OOF 里 2 个显著正向、
0 个显著负向。它留下两个未决问题：

1. 这个弱正向是 Hate L6 独有，还是其他 `dataset×depth` 也有？
2. M1.35 的官方 test matched-C 是 0 正 / 41 负 / 139 跨；如果 OOF
   出现正向，应该信 OOF 还是信 test？

M1.37 把 M1.36 的 nested CV 口径扩到全部 6 个 `dataset×depth`、
两个 probe、5 个 outer seed，得到 180 个 OOF CI；再把同一批比较与
M1.35 的 official test 并排，专门看“OOF 正向是否能在 test 上留下
同号证据”。

### 11.37.2 口径

- 数据：SST-2、CoLA、TweetEval hate 的 L12/L6；每个 case 只用
  train 做 outer/inner CV，官方 test 不参与选 C、训练或评分。
- outer：`StratifiedKFold(5, shuffle=True, random_state=outer_seed)`，
  outer seed `7/13/19/31/43`；每个 outer seed 生成一套覆盖全部 train
  的 OOF 预测。
- inner：每个 outer-train 内再做
  `StratifiedKFold(5, shuffle=True, random_state=7)`，只在 **真 f32**
  上选 C；f32 与全部量化臂共用该 C，在 outer-train 上重训，预测
  outer-val。C grid `(0.01,0.1,1.0,10.0)`。
- probe：`logreg` 与 `linear_svc`。SST-2/hate 用 accuracy、无类别
  权重；CoLA 用 balanced + macro-F1。
- 每个 `case×probe×outer_seed×arm` 对 f32 做 10k paired bootstrap
  （同一组 OOF 重采样索引同时用于臂与 f32）：`6 case ×2 probe ×
  5 seed ×3 arm = 180` 个 CI。`C_by_fold` 与 `fold_deltas` 落在原始表，
  便于复核。
- 这是 nested procedure 的 OOF 读数，不是官方 test 读数；两者的样本
  量、分布和 C 选择都不同，不能直接拼成一张显著性表。

### 11.37.3 总读数：5 正 / 52 负 / 123 跨 0

180 个 CI 的判决：

| 判决 | 个数 |
|---|---:|
| CI 整段 `>0` | **5** |
| CI 整段 `<0` | **52** |
| CI 跨 0 | **123** |
| 合计 | **180** |

按 36 个比较汇总：**4 个比较出现过正 CI，15 个出现过负 CI，
17 个全跨 0**；没有任何比较同时出现正和负 CI。正向全部集中在
Hate，SST-2/CoLA 的 24 个比较没有任何正 CI：

| case | OOF 正/负/跨 | test 正/负/跨 | f32 OOF | f32 test（C=1） | test−OOF |
|---|---|---:|---:|---:|---:|
| SST-2 L12 | 0/21/9 | 0/20/10 | `0.829350` | `0.842943` | `+1.36pp` |
| SST-2 L6 | 0/25/5 | 0/0/30 | `0.806965` | `0.811642` | `+0.47pp` |
| CoLA L12 | 0/5/25 | 0/0/30 | `0.569747` | `0.549921` | `−1.98pp` |
| CoLA L6 | 0/1/29 | 0/5/25 | `0.546212` | `0.553124` | `+0.69pp` |
| Hate L12 | 3/0/27 | 0/16/14 | `0.771998` | `0.574250` | `−19.78pp` |
| Hate L6 | 2/0/28 | 0/0/30 | `0.762671` | `0.546086` | `−21.66pp` |

- SST-2/CoLA 的 OOF 负向明显多于 test：OOF 的 n 更大、C 由内层
  选择，能分辨 `−0.5` 到 `−1.5pp` 的量化损失；test 的 n 小，很多
  CI 跨 0。这些负向不是新增益，也不改变“没有量化档超过 f32”。
- Hate 的方向相反：OOF 没有负向，反而有 5 个正向；test 却有 16 个
  负向。Hate L12 的 `R12` 是最明显的反向格。

5 个显著正向的具体读数：

| case | probe | arm | outer seed | Δ | 95% CI |
|---|---|---|---|---:|---:|
| Hate L12 | logreg | R12 | 7 | `+0.009238`（`+0.924pp`） | `[+0.002671,+0.016027]` |
| Hate L12 | logreg | R12 | 13 | `+0.010239`（`+1.024pp`） | `[+0.003673,+0.016694]` |
| Hate L12 | linear_svc | R12 | 13 | `+0.008792`（`+0.879pp`） | `[+0.002115,+0.015248]` |
| Hate L6 | logreg | rescan | 7 | `+0.008570`（`+0.857pp`） | `[+0.001558,+0.015582]` |
| Hate L6 | logreg | int3 | 43 | `+0.007902`（`+0.790pp`） | `[+0.001336,+0.014580]` |

### 11.37.4 OOF 正向与 test 反向：Hate 是跨域读数

Hate L12 的 `R12` 在 OOF 与 test 上方向相反：

- OOF logreg `R12` 5-seed 均值 `+0.6767pp`（sd `0.3015pp`），
  5/5 点估计为正，2/5 CI 显著正；SVC `R12` 均值 `+0.6477pp`
  （sd `0.1785pp`），5/5 点正，1/5 CI 显著正。
- 同一比较在官方 matched-C test 上：logreg 均值 `−1.3533pp`、
  SVC `−1.4411pp`，两个 probe 都是 **5/5 CI 整段为负**（M1.35）。
- OOF 的正向与 test 的负向相差约 `2pp`，不是“样本量不够”能解释的；
  这是同一批文本在 train 内与官方 test 上的方向翻转。

原因不是标签错位或 embedding 错配：Hate L12 的真 f32 在官方 test
上同样只有 `0.574250`，而 train OOF 是 `0.771998`；对 Hate test 做
内部 5 折（固定 C=1，只用于诊断标签可学性，不参与选 C 或最终结论），
f32 为 `0.763934`（范围 `0.762620–0.765909`）。test 标签本身是可学的，
`0.574` 是 train→test 迁移失败，不是标签噪声。Hate L6 同向：
OOF `0.762671`、test `0.546086`、test 内部 5 折 `0.751205`。

因此，**Hate 的官方 test 在这套切分上是跨域读数**；M1.31/M1.35/
M1.36 的 Hate test 数字仍可作为“跨域 test 上的相对比较”，但不能
当作 in-domain 量化损失的上界，也不能把 Hate OOF 的正向直接搬到
test。SST-2/CoLA 的 OOF 与 test 差距只有 `−1.98` 到 `+1.36pp`，
没有出现 Hate 这种 20pp 级迁移差。

### 11.37.5 判决

- **OOF 全矩阵没有改变分类头的主判决：官方 test 上 0 个量化档
  显著超过真 f32。** M1.37 的 5 个正 CI 全部在 Hate，且：
  - Hate L12 `R12` 在 test 上是 5/5 显著负向，属于反向，不能升级；
  - Hate L6 `rescan`/`int3` 在 test 上点估计为正但 5/5 CI 跨 0，
    仍只是弱候选。
- **OOF 也不是“量化无损”的证据。** 52 个负 CI 全部来自 SST-2/CoLA
  （SST-2 46、CoLA 6；Hate 0）；其中 SST-2 L6 25 个、CoLA L12 5 个、
  CoLA L6 1 个。量化在这些 case 的 train 分布内会掉点，只是官方 test
  的样本量/分布没有把同样的损失分辨出来。
- **Hate L6 `rescan` 仍是最接近的弱候选，但不能发布。** 它的 OOF
  5-seed 均值 `+0.5899pp`、5/5 点正、1/5 CI 显著；官方 matched-C
  test 均值 `+1.0534pp` 但 5/5 CI 跨 0。点方向在两种口径下同号，
  但没有任何一个 test CI 整段为正。
- **Hate L12 `R12` 是新的 OOF-only 候选，必须降级为“跨域不成立”。**
  它在 OOF 的 2/5 logreg +1/5 SVC CI 显著，但 test 5/5 负向；这比
  “不显著”更强，因为它给出了相反方向的证据。
- **分类头这条线可以收口。** 继续加 seed、加 C 或换 probe 不会改变
  “官方 test 无正向”的主结论；若还要继续，应做预注册的新数据集或
  新切分 confirmatory test，而不是在同一批 Hate train 上再调 C。
- 边界：5 个 outer seed 不是独立重复；bootstrap 只重采样 OOF，
  不覆盖训练方差；只训练线性 probe，冻结编码器；没有 MLP、没有
  encoder 微调、没有 MNLI；OOF 与 test 的样本量和分布不同，不能
  把 180 个 OOF CI 与 180 个 test CI 混成一张表。

> **M1.38 追加（阈值条件）**：本节把 Hate L12 `R12` 判为“OOF-only、跨域
> 不成立”，这个判决只在默认 0.5 阈值下成立。把阈值改成目标分数分位匹配
> train 先验（不用 test 标签）后，`R12` L12 logreg 5/5 正（`+1.83pp`）、
> SVC 4/5 正，test AUC `0.6999/0.7034` 也高于 f32 `0.6859/0.6886`；
> Hate L6 `rescan` 两 probe 各 5/5 正（`+2.55/+2.72pp`）。这不是“方向反了”
> 而是“默认阈值把排序增益压成 accuracy 负向”；但 target_prior 是 transductive、
> 后验选择的规则，只能记为条件性候选，不能升级为默认。默认 0.5 的 0/36
> 主判决不变。见 §11.38。

### 11.37.6 复算

```bash
# 6 个 case，每个跑 2 probe ×5 outer seed，落盘 30 行
python geometry/nested_cv_matrix.py --dataset sst2 --depth l12
python geometry/nested_cv_matrix.py --dataset sst2 --depth l6
python geometry/nested_cv_matrix.py --dataset cola --depth l12
python geometry/nested_cv_matrix.py --dataset cola --depth l6
python geometry/nested_cv_matrix.py --dataset hate --depth l12
python geometry/nested_cv_matrix.py --dataset hate --depth l6

# 合并 180 个 CI、36 行汇总、36 行 test 对照、6 行 OOF/test 差距
python geometry/make_nested_matrix_tsv.py
python geometry/make_nested_vs_test_tsv.py
```

表见 `data/wsp-classify-nested-cv-matrix.tsv`（180 行数据、17 列：
`dataset depth probe arm metric outer_seed inner_seed n_oof f32_score
arm_score point_delta boot_mean ci_low ci_high verdict C_by_fold
fold_deltas`）、`data/wsp-classify-nested-cv-matrix-summary.tsv`
（36 行数据、17 列，含 `n_pos_ci/n_neg_ci/n_cross_ci/n_pos_point/
any_positive/mean_point/sd_point/min_point/max_point/mean_boot/
C_by_seed`）、`data/wsp-classify-nested-vs-test.tsv`（36 行数据、
18 列，含 `test_n_pos/test_n_neg/test_n_cross/oof_n_pos/oof_n_neg/
oof_n_cross/conflict`）与 `data/wsp-classify-oof-test-gap.tsv`
（6 行数据、16 列，含 `f32_oof_mean/test_f32_c1/
test_internal_cv_mean/oof_test_gap_pp`）。

## 11.38 Hate 20pp 差距的域/阈值分解与目标先验阈值复验（M1.38）

### 11.38.1 问题

M1.37 把 Hate 的官方 test 判为跨域读数，但还没有回答三件事：

1. 20–22pp 的 f32 差距是文本域迁移，还是 test 更长、12.8% 超过 64
   token 的截断混杂？M1.15 只记了长度统计，没有把长度与错误类型分开。
2. 差距里有多少是排序能力损失，有多少只是 0.5 决策阈值在目标域上
   漂移？M1.35/M1.37 的 test 读数都用默认阈值；如果目标域分数整体
   偏移，accuracy 会被阈值压低，而 AUC/排序不一定坏。
3. M1.35/M1.37 的“0 个显著正向”是否阈值条件的结论？如果换成不用
   test 标签的目标先验阈值，量化臂是否仍全部不胜？

M1.38 先做域分类器与长度分桶，再把 f32 的 20pp 差距拆成“阈值”与
“校准后残差”，最后把同一套目标先验阈值套到全部 36 个比较上。仍是
冻结编码器 + 线性 probe（logreg / LinearSVC），没有 MLP、没有 encoder
微调、没有 MNLI。

### 11.38.2 域分类器与长度诊断

域分类器只判断“这句话来自 train 还是 test”，不碰 hate 标签：

- 词 1–2 gram TF-IDF，`min_df=2`、`max_features=200000`、`sublinear_tf`；
  5 折 StratifiedKFold（seed 7），vectorizer 在每折内重新 fit，避免把
  测试词表提前并入；
- 线性 logistic，`class_weight=balanced`；报 AUC 与 accuracy；
- 另做“只看长度”的 logistic，作为长度混杂的对照。

| dataset | n_train | n_test | 域分类器 AUC | 域分类器 acc | 长度单特征 AUC |
|---|---:|---:|---:|---:|---:|
| SST-2 | 6,920 | 1,821 | `0.490853` | `0.648095` | `0.486653` |
| CoLA | 8,551 | 1,043 | `0.686817` | `0.807380` | `0.506840` |
| Hate | 8,985 | 2,734 | `0.921051` | `0.853400` | `0.561306` |

Hate 的 train/test 文本几乎可以被词袋线性分开（AUC `0.921`）；SST-2
接近随机（`0.491`），CoLA 有中等 topic 差（`0.687`）。长度单特征在
三个数据集都接近随机（`0.487/0.507/0.561`），所以 Hate 的 `0.921`
主要不是“test 更长”造成的。CoLA 的 `0.687` 也没有对应 20pp 级
accuracy 差（§11.37 的 CoLA f32 OOF/test 差只有 `−1.98` 到 `+0.69pp`），
说明域分类器 AUC 是迁移的**相关诊断**，不是充分条件；Hate 是唯一
同时出现高 AUC 与 20pp 差距的 case。

Hate 的 token 长度（tokenizer 不截断）：

| split | n | mean | p50 | p95 | max | >64 token |
|---|---:|---:|---:|---:|---:|---:|
| train | 8,985 | `31.84` | 29 | 63 | 112 | `4.50%` |
| test | 2,734 | `36.59` | 32 | 77 | 141 | `12.77%` |

test 确实更长，但把 f32 logreg（C=1）的 test 预测按长度分桶后，准确率
几乎不随长度变化：

| test 长度桶 | n | accuracy | 真实正例率 | 预测正例率 | mean tokens |
|---|---:|---:|---:|---:|---:|
| `<=32` | 1,440 | `0.577083` | `0.406944` | `0.764583` | `22.08` |
| `33–64` | 945 | `0.573545` | `0.431746` | `0.720635` | `44.12` |
| `65–96` | 337 | `0.566766` | `0.356083` | `0.646884` | `74.89` |
| `>96` | 12 | `0.500000` | `0.083333` | `0.583333` | `109.25` |
| `<=64` | 2,385 | `0.575681` | `0.416771` | `0.747170` | `30.81` |
| `>64` | 349 | `0.564470` | `0.346705` | `0.644699` | `76.07` |

`>64` 与 `<=64` 的 accuracy 只差 `1.12pp`，远小于 20pp；长度桶内
accuracy 也都在 `0.50–0.58`。因此在这个 probe 上，**截断/长度不是
20pp 差距的主因**。这里没有做 seq128 重编码；长度分桶与长度单特征
AUC 只否定了“长度是主要解释”，没有证明“换 seq128 一定没有帮助”。

### 11.38.3 20pp 差距的分解：阈值约 6–9pp，校准后仍差 12–13pp

先固定 f32 logreg（C=1）在 Hate test 上，比较四种决策规则。所有规则
都只用于诊断，不参与 M1.35 的 matched-C 选 C：

- `0.5`：默认阈值；
- `train_score_prior`：在 **train 分数**上取 `1−train 先验` 分位，
  再套到 test；只用 train，不看 test 分布（只作诊断，未进主表）；
- `target_prior`：在 **test 分数**上取 `1−train 先验` 分位，再套到
  test；使用未标注 test 分数，不使用 test 标签；
- `test_prior` / `oracle`：分别用 test 标签的先验/最优阈值，只作诊断
  上界，不进入任何结论。

| depth | rule | threshold | pred positive | accuracy | macro-F1 | test AUC |
|---|---|---:|---:|---:|---:|---:|
| L12 | `0.5` | `0.500000` | `0.734089` | `0.574250` | `0.565499` | `0.685903` |
| L12 | `target_prior` | `0.763184` | `0.420995` | `0.634236` | `0.623194` | `0.685903` |
| L12 | `test_prior`（诊断） | `0.770360` | `0.407827` | `0.635699` | `0.622883` | `0.685903` |
| L12 | `oracle`（诊断） | `0.801236` | `0.352963` | `0.645208` | `0.623675` | `0.685903` |
| L6 | `0.5` | `0.500000` | `0.750549` | `0.546086` | `0.534408` | `0.671841` |
| L6 | `target_prior` | `0.755415` | `0.420995` | `0.633504` | `0.622441` | `0.671841` |
| L6 | `test_prior`（诊断） | `0.764180` | `0.407827` | `0.634967` | `0.622126` | `0.671841` |
| L6 | `oracle`（诊断） | `0.834838` | `0.272129` | `0.646306` | `0.605943` | `0.671841` |

关键点：

- 默认 0.5 下模型把 test 预测成 `73–75%` 正例，而 train 先验是
  `42.08%`、test 真先是 `40.78%`；这不是标签先验差（train/test 只差
  `1.31pp`），而是目标域分数整体抬高，0.5 阈值落到了过预测区。
- 只在 **train 分数**上取分位（`train_score_prior`）修不好：train 分数
  与 test 分数分布不同；**必须在目标分数上取分位**，才把预测正例率压回
  `0.421`。这个规则只用未标注 test 分数，不用 test 标签，但它是
  transductive 的；不能当成普通 benchmark 的默认协议。
- `target_prior` 把 L12 从 `0.574250` 提到 `0.634236`（`+6.00pp`），
  L6 从 `0.546086` 提到 `0.633504`（`+8.74pp`）；`test_prior` 与它
  几乎相同（`0.635699/0.634967`），`oracle` 也只再高约 `1.1–1.3pp`。

把 test 内部 5 折 f32（固定 C=1，诊断用）当 in-domain 参照：

| case | in-domain（test 5 折） | 默认 0.5 | target-prior | 阈值回收 | 校准后残差 |
|---|---:|---:|---:|---:|---:|
| Hate L12 | `0.763934` | `0.574250` | `0.634236` | `+6.00pp` | `−12.97pp` |
| Hate L6 | `0.751205` | `0.546086` | `0.633504` | `+8.74pp` | `−11.77pp` |

也就是说，`19.0–20.5pp` 的 test 内部差距里，约 `6–9pp` 是阈值漂移，
校准后仍有 `12–13pp` 的残差；残差里包含排序质量下降（AUC 作为代理
为 `0.686/0.672`，低于 in-domain accuracy `0.764/0.751`，但 AUC 与
accuracy 不是同一尺度，不能把 `7.8pp` 直接相加）。OOF 视角同样：
L12 OOF `0.771998`、L6 `0.762671`，对 target-prior 的残差为
`13.78/12.92pp`，与 test 内部参照同量级。

### 11.38.4 全臂目标先验阈值复验

把 M1.35 的 matched-C 原样保留（每个 `dataset×depth×probe×seed` 只在
真 f32 上做 5 折选 C，f32 与全部量化臂共用），只改决策阈值：

- `default`：logreg 0.5 / SVC 0；与 M1.35 完全同口径；
- `target_prior`：每个臂在 **自己的 test 分数**上取 `1−train 先验`
  分位，预测 `score >= q`；预测正例率被压到 `0.420995`，不用 test
  标签。

6 个 case、2 probe、5 seed、36 个比较，共 360 个 CI（每规则 180 个）：

| rule | 正 CI | 负 CI | 跨 0 | 36 个比较中出现过正 CI |
|---|---:|---:|---:|---:|
| `default` | 0 | 41 | 139 | 0 |
| `target_prior` | 23 | 30 | 127 | 6 |

`default` 与 M1.35 逐项一致（0 正 / 41 负 / 139 跨）。`target_prior`
的 6 个正比较全部在 Hate，SST-2/CoLA 的 24 个比较仍是 0 个正 CI；
按 case 的 CI 分布：

| case | default 正/负/跨 | target_prior 正/负/跨 |
|---|---:|---:|
| SST-2 L12 | 0/20/10 | 0/20/10 |
| SST-2 L6 | 0/0/30 | 0/0/30 |
| CoLA L12 | 0/0/30 | 0/0/30 |
| CoLA L6 | 0/5/25 | 0/10/20 |
| Hate L12 | 0/16/14 | 9/0/21 |
| Hate L6 | 0/0/30 | 14/0/16 |

默认→target 的 36 行对照是 `6 gain / 30 same / 0 loss`：没有任何比较从
正 CI 变成负 CI，也没有把某个比较的负向“修成”正向；6 个 gain 就是
上表 Hate 的 6 个比较。

Hate 的 6 个正比较：

| case | probe | arm | 正 CI / 5 | 5-seed 均值 | 代表 CI |
|---|---|---|---:|---:|---:|
| Hate L12 | logreg | `R12` | 5/5 | `+1.8288pp` | `[+0.3292,+3.2928]pp` |
| Hate L12 | linear_svc | `R12` | 4/5 | `+1.6825pp` | `[+0.4023,+3.3650]pp`（seed 13 为 `+0.8047pp`、CI 跨 0） |
| Hate L6 | logreg | `rescan` | 5/5 | `+2.7213pp` | `[+1.1339,+4.1697]pp`（seed 7 为 `[+1.5362,+4.6086]pp`） |
| Hate L6 | linear_svc | `rescan` | 5/5 | `+2.5457pp` | `[+0.9510,+4.0234]pp`（seed 7/13 为 `[+1.1339,+4.1331]pp`） |
| Hate L6 | linear_svc | `int4` | 3/5 | `+1.0973pp` | `[+0.2560,+2.2677]pp`（seed 7/13 为 `+0.8778pp`、CI 跨 0） |
| Hate L6 | logreg | `int3` | 1/5 | `+1.3899pp` | `[+0.1463,+3.2196]pp`（仅 seed 7；其余 4 seed 为 `+1.3168pp`、CI 跨 0） |

绝对分数与排序诊断（f32 同阈值规则）：

| case | probe | arm | f32 | arm | Δ | test AUC（arm vs f32） |
|---|---|---|---:|---:|---:|---:|
| Hate L12 | logreg | `R12` | `0.634236` | `0.652524` | `+0.018288` | `0.6999` vs `0.6859` |
| Hate L12 | linear_svc | `R12` | `0.637162` | `0.656181` | `+0.019020` | `0.7034` vs `0.6886` |
| Hate L6 | logreg | `rescan` | `0.631309–0.633504` | `0.657644–0.664228` | 均值 `+0.027213` | `0.6994–0.7094` vs `0.6627–0.6718` |
| Hate L6 | linear_svc | `rescan` | `0.631309–0.634967` | `0.656181–0.661302` | 均值 `+0.025457` | `0.7014–0.7083` vs `0.6645–0.6728` |

`R12` 与 `rescan` 的 target-prior 增益不是纯阈值伪影：test AUC 也高于
f32（L12 `R12` 约 `+1.2–1.5pp`，L6 `rescan` 约 `+2.8–3.7pp`）。`int4`
SVC 的 AUC/accuracy 只有 `3/5`，`int3` logreg 只有 `1/5`，是弱候选；
不能把 6 个正比较当成 6 个独立候选：`R12` 两个 probe 是同一臂，
`rescan` 两个 probe 也是同一臂。

SST-2/CoLA 在 target_prior 下没有正 CI；CoLA L6 的负 CI 反而从 5 个
增到 10 个，说明目标先验阈值不是“普适提分”，它只对分数漂移严重的
Hate 有效。

### 11.38.5 与 M1.37 的关系

- M1.37 的默认 0.5 test 读数不变：Hate L12 `R12` 仍 5/5 负向、Hate L6
  `rescan`/`int3` 仍 5/5 跨 0；官方 test 的 headline 仍是“0 个显著
  正向，f32 锚点”。
- 但 M1.37 把 `R12` 判为“OOF-only、跨域不成立”是**默认阈值条件**的。
  target_prior 下 `R12` L12 logreg 5/5 正（`+1.83pp`）、SVC 4/5 正，
  且 test AUC `0.6999/0.7034` 高于 f32 `0.6859/0.6886`；Hate L6
  `rescan` 也从默认阈值下的“点正、5/5 跨 0”变成两个 probe 5/5 正。
  这与 M1.37 的 OOF 正向（`R12 +0.68pp`、`rescan +0.59pp`）方向一致，
  不再是“OOF 正、test 负”的纯方向翻转；默认阈值下的负向主要是阈值
  漂移，不是排序能力完全反向。
- 这不把 `R12`/`rescan` 升级为默认：target_prior 是后验选择的
  transductive 阈值规则，只在 Hate 出现正 CI，SST-2/CoLA 没有；
  `int4`/`int3` 仍只有 3/5、1/5。它们是**条件性候选**，不是通用
  量化增益。
- M1.15 的旧表仍是“per-arm C + 默认阈值 + 跨域 test”的历史口径，不能
  与 M1.31 的 per-arm CV 表或 M1.35 的 matched-C 表拼接；M1.15 的
  fixed C=1 敏感性也不能当主表。M1.38 的 target_prior 复验只在
  matched-C 表上改阈值，不改变 C 选择口径。

### 11.38.6 判决

- **默认 0.5 的官方 test 主判决不变：0/36 个量化档显著超过真 f32；
  41 个负 CI；f32 继续是默认锚点。** M1.35/M1.37 的这组读数仍是
  benchmark headline。
- **Hate 的 20–22pp 差距不是长度/截断，也不是标签噪声；主因是文本域
  迁移 + 目标域阈值漂移。** 域分类器 AUC `0.921`、长度单特征 `0.561`、
  长度分桶 accuracy 基本持平；校准阈值可回收 `6–9pp`，校准后仍差
  `12–13pp`。
- **target_prior 下 6/36 个比较出现正 CI，全部在 Hate：`R12` L12 与
  `rescan` L6 是最强的两个条件性候选；`int4` SVC 3/5、`int3` logreg
  1/5 只是弱信号。** SST-2/CoLA 的 24 个比较仍 0 正，所以这不是
  “量化普遍提升线性分类”的新证据。
- **`R12`/`rescan` 不能因这次复验直接发布。** 阈值规则是看到 0.5
  阈值漂移后选的，5 个 seed 共享同一批 test 标签，target_prior 还是
  transductive 的；需要预注册的独立数据集/切分和固定阈值规则做
  confirmatory test。继续在同一批 Hate 上换 seed/C/阈值只会增加
  后验选择，不会增加独立证据。
- **分类头这条线可以收口，但收口口径要改成“默认阈值无正向；目标
  先验阈值下有 Hate-only 条件性候选”。** 不再把 `R12` 写成“跨域
  不成立”，也不再把它写成“已证超过 f32”；两者都过度了。
- 边界：域分类器是词袋线性代理，不做 topic/句法/标注；长度桶用 L12
  tokenizer，没有 seq128 重编码；target_prior 使用未标注 test 分数，
  是 transductive 校准；5 个 seed 不是独立重复；bootstrap 只重采样
  test，不覆盖训练与阈值选择方差；只有 3 个数据集、2 个深度、2 个
  线性头；没有 MLP、没有 encoder 微调、没有 MNLI。

### 11.38.7 复算

M1.38 的 5 个脚本已入库到 `docs/research/scripts/wsp_m138/`：`hate_domain_audit.py`、
`matched_prior_full.py`、`make_matched_prior_tsv.py`，以及它们依赖的
`cv_seed_stability.py`、`matched_c_full.py`。以下命令在仓库根执行；默认读
`./trit-results`，可用 `TRIT_RESULTS` 指向实际 embedding 根，`TRIT_GEOM_OUT`
覆盖中间 JSON/f32 目录，`TRIT_DATA_DIR` 控制 TSV 输出目录，
`TRIT_L12_TOKENIZER` 覆盖 tokenizer id/路径。

```bash
export TRIT_RESULTS=/path/to/trit-results   # 默认 ./trit-results
export TRIT_DATA_DIR=docs/research/data     # 默认仓库 docs/research/data

# 域分类器、长度分桶、双向迁移、f32 阈值敏感性
python docs/research/scripts/wsp_m138/hate_domain_audit.py

# 6 个 case 的 target-prior 全臂矩阵（每 case 60 行）
python docs/research/scripts/wsp_m138/matched_prior_full.py --dataset sst2 --depth l12
python docs/research/scripts/wsp_m138/matched_prior_full.py --dataset sst2 --depth l6
python docs/research/scripts/wsp_m138/matched_prior_full.py --dataset cola --depth l12
python docs/research/scripts/wsp_m138/matched_prior_full.py --dataset cola --depth l6
python docs/research/scripts/wsp_m138/matched_prior_full.py --dataset hate --depth l12
python docs/research/scripts/wsp_m138/matched_prior_full.py --dataset hate --depth l6

# 合并 360 行 raw、72 行 summary、36 行 default 对照、24 行 f32
python docs/research/scripts/wsp_m138/make_matched_prior_tsv.py
```

本次已在原结果上重跑域审计与 matched-prior 汇总两条链路，8 张 TSV 的
SHA256 与仓库文件一致；`matched_prior_full.py` 的重跑仍沿用同一批
WSPEMB/f32，未重新编码。

表见：

- `data/wsp-classify-domain-shift.tsv`（3 行、6 列：`dataset n_train
  n_test domain_auc domain_acc length_only_auc`）；
- `data/wsp-classify-hate-transfer.tsv`（8 行、6 列：`dataset depth
  direction n_train n_eval accuracy`）；
- `data/wsp-classify-hate-threshold.tsv`（8 行、8 列：`depth rule
  threshold n pred_positive_rate accuracy macro_f1 test_auc`）；
- `data/wsp-classify-hate-length-buckets.tsv`（6 行、6 列：`bin n
  accuracy positive_rate pred_positive_rate mean_tokens`）；
- `data/wsp-classify-matched-prior-raw.tsv`（360 行、18 列：`dataset
  depth probe arm metric seed rule C threshold target_prior
  pred_pos_rate f32_score arm_score point_delta boot_mean ci_low ci_high
  verdict`）；
- `data/wsp-classify-matched-prior-summary.tsv`（72 行、18 列：`dataset
  depth probe arm metric rule n_seeds n_pos_ci n_neg_ci n_cross_ci
  n_pos_point any_positive mean_point sd_point min_point max_point
  mean_boot mean_threshold`）；
- `data/wsp-classify-matched-prior-vs-default.tsv`（36 行、16 列：`dataset
  depth probe arm metric default_n_pos default_n_neg default_n_cross
  default_any_positive default_mean_point target_n_pos target_n_neg
  target_n_cross target_any_positive target_mean_point flip`）；
- `data/wsp-classify-matched-prior-f32.tsv`（24 行、8 列：`dataset
  depth probe rule n_seeds mean_score min_score max_score`）。

## 11.39 目标先验阈值的预注册半分割 confirmatory（M1.39）

### 11.39.1 问题与预注册

M1.38 的 6 个正 CI 全部来自同一批 Hate 官方 test 标签；5 个 seed 只是
训练侧重复，阈值校准和评估使用同一批 test 分数。M1.39 问一个更窄、
可预注册的问题：

> 把 test 分数切成互斥的校准半与评估半后，M1.38 的 `R12` / `rescan`
> target-prior 增益还能不能在新的一半标签上复现？

预注册写在 `data/wsp-classify-m139-prereg.json`，在跑结果前单独提交
（commit `d7b6c0d9`）。预注册固定：

- 数据仍是 TweetEval/hate 官方 train/test，不重新编码、不换数据集；
- test 用 `StratifiedKFold(n_splits=2, shuffle=True, random_state=101)`
  做 50/50 分层半分割，两个方向 `A_to_B` / `B_to_A` 互为校准/评估；
- 训练侧仍是冻结编码器 + 线性 probe（logreg / LinearSVC），每个
  case/probe/seed 只在真 f32 的 train 内 5 折选 C，f32 与全部量化臂
  共用该 C；seeds `7/13/19/31/43`；
- `target_prior` 只用校准半的未标注 test 分数：阈值取
  `quantile(scores_calib, 1 - prior_train)`，`prior_train = 0.420812`；
  评估半标签只用于打分；
- primary endpoint 是 accuracy，每个 candidate×probe 有 5 seeds×2
  directions = 10 个 paired bootstrap CI（10k，95% 双侧）；
- 判据：`>=8/10` 个 CI 整段 >0 为 **confirmed**，`<=3/10` 为
  **rejected**，其余 **inconclusive**；看到结果后不改阈值；
- macro-F1 是次要口径；default 0.5（logreg）/0（SVC）是阈值对照；
  `int4`/`int3` 是弱控制，不进入 primary 判定。

### 11.39.2 切分与口径

两个半分割各有 `1,367` 条 test；A 半正例 `557`（`0.407462`），B 半
正例 `558`（`0.408193`）。A→B 时用 A 的未标注分数校准、B 的标签评估；
B→A 反过来。两半的正例率都与 train 先验 `0.420812` 接近，但**不是**
用 test 真先验校准。

| direction | eval n | eval pos | eval pos rate | calib n | calib pos | calib pos rate |
|---|---:|---:|---:|---:|---:|---:|
| `A_to_B` | 1,367 | 557 | `0.407462` | 1,367 | 558 | `0.408193` |
| `B_to_A` | 1,367 | 558 | `0.408193` | 1,367 | 557 | `0.407462` |

### 11.39.3 主读数：L6 `rescan` logreg confirmed，`R12` 未确认

Primary accuracy（每个 candidate×probe 汇总 10 个 CI）：

| case | probe | arm | n pos CI /10 | 5-seed×2-dir 均值 | min / max point | verdict |
|---|---|---|---:|---:|---:|---|
| Hate L12 | logreg | `R12` | 5 | `+1.9020pp` | `+1.5362 / +2.2677pp` | **inconclusive** |
| Hate L12 | linear_svc | `R12` | 4 | `+1.6167pp` | `+0.2195 / +2.6335pp` | **inconclusive** |
| Hate L6 | logreg | `rescan` | 10 | `+2.8237pp` | `+2.5604 / +4.0234pp` | **confirmed** |
| Hate L6 | linear_svc | `rescan` | 7 | `+2.4506pp` | `+2.0483 / +3.0724pp` | **inconclusive** |

代表性 CI（各 case 的 seed 7）：

| case | probe | direction | f32 acc | arm acc | Δ | 95% CI |
|---|---|---|---:|---:|---:|---:|
| Hate L12 `R12` | logreg | A→B | `0.630578` | `0.653255` | `+2.2677pp` | `[+0.1463,+4.4623]pp` |
| Hate L12 `R12` | logreg | B→A | `0.637893` | `0.653255` | `+1.5362pp` | `[−0.5121,+3.5845]pp` |
| Hate L12 `R12` | linear_svc | A→B | `0.634967` | `0.661302` | `+2.6335pp` | `[+0.5121,+4.7549]pp` |
| Hate L12 `R12` | linear_svc | B→A | `0.640088` | `0.650329` | `+1.0241pp` | `[−1.0241,+3.0724]pp` |
| Hate L6 `rescan` | logreg | A→B | `0.637893` | `0.663497` | `+2.5604pp` | `[+0.3658,+4.7549]pp` |
| Hate L6 `rescan` | logreg | B→A | `0.627652` | `0.667886` | `+4.0234pp` | `[+1.9020,+6.2180]pp` |
| Hate L6 `rescan` | linear_svc | A→B | `0.637893` | `0.660571` | `+2.2677pp` | `[+0.0732,+4.3892]pp` |
| Hate L6 `rescan` | linear_svc | B→A | `0.631309` | `0.662034` | `+3.0724pp` | `[+0.9510,+5.2670]pp` |

`R12` 不是“被拒绝”，而是停在 inconclusive：A→B 半稳定正，B→A 半
点估计仍正但 CI 跨 0；SVC 的 A→B 正、B→A 跨 0。L6 `rescan` 的 logreg
在两半、5 seeds 全部正（10/10），SVC 的 A→B 有 2/5 正、B→A 5/5 正
（合计 7/10）。按预注册判据，只有 L6 `rescan` logreg accuracy 过线。

Macro-F1 次要口径（C 仍按 f32 accuracy 选，只换评分函数；2k bootstrap）：

| case | probe | arm | n pos CI /10 | 均值 | verdict（次要） |
|---|---|---|---:|---:|---|
| Hate L12 | logreg | `R12` | 5 | `+1.9583pp` | inconclusive |
| Hate L12 | linear_svc | `R12` | 4 | `+1.6795pp` | inconclusive |
| Hate L6 | logreg | `rescan` | 10 | `+2.8345pp` | confirmed |
| Hate L6 | linear_svc | `rescan` | 10 | `+2.5372pp` | confirmed |

Macro-F1 把 L6 `rescan` 的 SVC 也推到 10/10；但 primary 是 accuracy，
SVC accuracy 7/10，不能拿次要口径覆盖 primary。`R12` 在两个 metric
上都停在 5/10、4/10。

### 11.39.4 default 0.5 仍是负向，target_prior 只是把它翻到半稳定

| case | probe | arm | default 均值（正/负/跨） | target 均值（正/负/跨） | target−default |
|---|---|---|---:|---:|---:|
| Hate L12 | logreg | `R12` | `−1.3533pp`（0/5/5） | `+1.9020pp`（5/0/5） | `+3.2553pp` |
| Hate L12 | linear_svc | `R12` | `−1.4411pp`（0/5/5） | `+1.6167pp`（4/0/6） | `+3.0578pp` |
| Hate L6 | logreg | `rescan` | `+1.0534pp`（4/0/6） | `+2.8237pp`（10/0/0） | `+1.7703pp` |
| Hate L6 | linear_svc | `rescan` | `+0.6072pp`（0/0/10） | `+2.4506pp`（7/0/3） | `+1.8434pp` |

这与 M1.38 的 full-test 读数方向一致：default 下 `R12` 仍是负向，
target 下翻正；`rescan` default 本来弱正，target 把正 CI 数量抬上去。
但半分割显示 `R12` 的翻正并不在两半都稳定，`rescan` 的 logreg 稳定、
SVC 只在 B→A 半稳定。

### 11.39.5 与 M1.38 的关系：点估计复现，CI 判定不稳

M1.38 full-test 与 M1.39 半分割的 5-seed 均值：

| case | probe | arm | M1.38 full-test | M1.39 半分割 | 差 |
|---|---|---|---:|---:|---:|
| Hate L12 | logreg | `R12` | `+1.8288pp` | `+1.9020pp` | `+0.0732pp` |
| Hate L12 | linear_svc | `R12` | `+1.6825pp` | `+1.6167pp` | `−0.0658pp` |
| Hate L6 | logreg | `rescan` | `+2.7213pp` | `+2.8237pp` | `+0.1024pp` |
| Hate L6 | linear_svc | `rescan` | `+2.5457pp` | `+2.4506pp` | `−0.0951pp` |

四个点估计都在 `±0.11pp` 内复现，但 CI 判定从“full-test 正”变成
“半分割部分跨 0”。这不是样本量不够的另一种说法，而是**同一批
test 文本被切开后，评估半只有 1,367 条，且两半的分数分布不同**：
`R12` 的 A→B 平均 `+2.27pp`、B→A `+1.54pp`（logreg）；SVC 的
A→B `+2.63pp`、B→A `+1.02pp`。L6 `rescan` 两半都正，logreg
A→B `+2.56pp`、B→A `+4.02pp`，SVC `+2.27/+3.07pp`。

因此 M1.38 的结论应改写成：

- `R12` 的 target-prior 点估计在 Hate L12 上可复现，但**没有通过
  预注册半分割 confirmatory**；仍是 conditional candidate，不是
  benchmark 结论。
- `rescan` 的 L6 target-prior 在 logreg accuracy 上通过预注册
  confirmatory；SVC accuracy 7/10 未过线，但 macro-F1 10/10，
  方向与幅度都复现。它是目前唯一有跨 probe/跨 metric 方向一致性的
  候选，但 primary 仍只有一个 probe 过线。
- default 0.5 的 0/36 主判决不变；target_prior 仍是 transductive、
  后验选择的 exploratory 规则，不能写成可部署协议。

### 11.39.6 有效独立性与边界

预注册的 10 个 CI 不是 10 次独立复现：

- 5 个 seed 共享同一批 test 文本；同一 C 下 logreg 的 5 个 seed 甚至
  给出相同预测。L12 `R12` logreg 的 5 个 A→B 正 CI 实际上是**同一个
  半分割**重复 5 次；L6 `rescan` logreg 的有效独立单元是 2 个方向 ×
  2 个 C（1 和 10），4/4 方向×C 全正。
- L12 `R12` SVC 的 C=`0.1` 在 A→B 正、B→A 跨 0；C=`1` 两方向都跨 0。
  有效独立单元是 1/4 方向×C 正。
- L6 `rescan` SVC 的 C=`0.1` 两方向都正，C=`1` A→B 跨 0、B→A 正；
  有效独立单元 3/4。7/10 的计数被 3 个重复 seed 放大。

所以“confirmed”只能按预注册的 10-CI 计数读，不能读成“10 次独立
实验都成功”。半分割还共享同一批 test 文本、同一编码器、同一训练集；
它只把**阈值校准分数**与**评估标签**分开，没有换语料，也没有换
train/test 域。它比 M1.38 的 full-test 更严，但仍不是独立数据集
confirmatory。

### 11.39.7 判决

1. **L12 `R12`：不升级。** default 0.5 仍是 0 正 / 5 负 / 5 跨；
   target_prior 点估计 `+1.62–1.90pp`，但预注册半分割只有 5/10 和
   4/10 正 CI，判 inconclusive。它可以继续作为“Hate-only、target_prior
   条件下的候选”，不能写成“跨域量化增益”。
2. **L6 `rescan`：保留为唯一通过 primary 的候选。** logreg accuracy
   10/10（`+2.82pp`）confirmed；SVC accuracy 7/10（`+2.45pp`）
   inconclusive，但 macro-F1 10/10。方向和幅度与 M1.38 的
   `+2.55–2.72pp` 一致。仍限 L6、Hate、target_prior、线性 probe。
3. **target_prior 仍是 exploratory。** 它不用 test 标签，但用未标注
   test 分数做 transductive 分位；半分割也没有消除“同一语料”的
   限制。不能当 benchmark headline，也不能当默认部署阈值。
4. **下一步必须换语料。** 半分割已经把“同一 test 标签”的问题压到
   最小；要继续 confirmatory，需要第二个 hate 数据集（如 HateXplain/
   Davidson 的预注册二分类映射）重新编码，或预注册新的 train/test
   域切分。继续在同一批 TweetEval/hate test 上换 seed/C/半分割，
   只会增加选择自由度。

### 11.39.8 复算与表

脚本 `docs/research/scripts/wsp_m139/split_half_confirm.py` 复用
`wsp_m138` 的 `cv_seed_stability.py` / `matched_c_full.py`。以下命令
在仓库根执行；`TRIT_RESULTS` / `TRIT_GEOM_OUT` / `TRIT_DATA_DIR`
含义同 §11.38.7。

```bash
export TRIT_RESULTS=/path/to/trit-results
export TRIT_GEOM_OUT="$TRIT_RESULTS/geometry/corrected"

# primary accuracy（240 行 raw、24 行 summary、4 行 primary）
python docs/research/scripts/wsp_m139/split_half_confirm.py \
  --depth both --seeds 7,13,19,31,43

# 次要 macro-F1（C 仍按 f32 accuracy 选；2k bootstrap）
python docs/research/scripts/wsp_m139/split_half_confirm.py \
  --depth both --seeds 7,13,19,31,43 --metric macro_f1
```

表见：

- `data/wsp-classify-m139-prereg.json`（预注册配置，跑前提交）；
- `data/wsp-classify-m139-split.tsv`（2 行、7 列：`direction eval_n
  eval_pos n_eval_pos_rate calib_n calib_pos n_calib_pos_rate`）；
- `data/wsp-classify-m139-raw.tsv`（240 行、21 列：`dataset depth
  probe arm metric seed rule direction C target_prior threshold eval_n
  eval_pos_rate calib_pos_rate f32_score arm_score point_delta
  boot_mean ci_low ci_high verdict`）；
- `data/wsp-classify-m139-summary.tsv`（24 行、15 列：`dataset depth
  probe arm metric rule n_eval_ci n_pos_ci n_neg_ci n_cross_ci
  n_pos_point mean_delta sd_delta min_delta max_delta`）；
- `data/wsp-classify-m139-primary.tsv`（4 行、11 列：`dataset depth
  probe arm n_pos_ci n_eval_ci mean_delta sd_delta min_delta max_delta
  verdict`）；
- `data/wsp-classify-m139-raw-macro.tsv`（240 行、21 列，字段同上）；
- `data/wsp-classify-m139-summary-macro.tsv`（24 行、15 列，字段同上）。

### 11.40 M1.40：HateXplain 第二个 hate 语料的预注册 confirmatory

#### 11.40.1 预注册与语料

预注册文件 `data/wsp-classify-m140-prereg.json` 在第一次读取模型分数前
单独提交（commit `c841c4fd`）。这一节的所有主判定都按该文件执行，不在
看到结果后改口径。

语料是 HateXplain 官方仓库的 `Data/dataset.json` 与
`Data/post_id_divisions.json`：

- `dataset.json` 12,256,170 B，SHA256
  `63bb3340fee0ec469b09690d04cb68f7c187787dd8b83807f071892c084967fb`；
- `post_id_divisions.json` 591,921 B，SHA256
  `c2fb0d89862e7897b11ea3e9380753f15a793482b4b70ad0532dfb1212212835`；
- 官方划分键是 `train` / `val` / `test`（不是 `valid`）：
  `15,383 / 1,922 / 1,924`；
- 每条 3 个 annotator，取 2-of-3 多数票；官方切分内没有平票；
- fit = official `train + val`，eval = official `test`；不交换、不重切；
- primary 二分类：`hatespeech=1`，`normal + offensive=0`；
- secondary toxic 映射（`hatespeech + offensive=1`，`normal=0`）只作为
  exploratory，本次没有进入 primary 矩阵。

#### 11.40.2 清洗、规模与长度

文本由 `post_tokens` 单空格 join、折叠空白并 `strip()`。清洗规则：

- 空文本丢弃（0 条）；
- fit 内 stripped-text 去重；同文本 3 类多数票冲突的文本丢弃；
- test 内去重；test 中与 fit 精确重叠的文本移除；
- test 中与 TweetEval/hate train/test 精确重叠的文本移除（本次 0 条）。

清洗结果：

| 项 | raw | 去重/冲突/泄漏后 |
|---|---:|---:|
| fit（official train+val） | 17,305 | **17,270** |
| test（official test） | 1,924 | **1,918** |
| fit 重复行 / 冲突文本 | 31 / 4 | — |
| test 重复行 / 冲突文本 | 1 / 0 | — |
| test→fit 精确重叠移除 | — | 5 |
| test→TweetEval 精确重叠移除 | — | 0 |

三类标签：

| split | normal | hatespeech | offensive |
|---|---:|---:|---:|
| fit | 7,011 | 5,336 | 4,923 |
| test | 777 | 594 | 547 |

Primary binary：fit `11,934 non-hate / 5,336 hate`，test
`1,324 non-hate / 594 hate`；hate 先验 `0.308975 / 0.309698`。
test 的文本来源是 `1,038 Gab / 880 Twitter`，fit 是
`9,408 Gab / 7,862 Twitter`；这不是纯 Twitter 语料，后文把它作为
外部边界，而不是直接等同一个 TweetEval 复制。

长度审计用 L12 tokenizer、关闭截断、包含 special tokens：

| split | p50 | p95 | max | `>64` | `>64` 比例 |
|---|---:|---:|---:|---:|---:|
| fit | 27 | 58 | 179 | 255 | 1.4765% |
| test | 26 | 58 | 91 | 27 | 1.4077% |

#### 11.40.3 编码与 matched-C

- L12：`all-MiniLM-L12-v2`，12 层，seq 64，冻结；仅编码
  `R12`（`swap-r12.json`，counts `2:6 / 3:54 / 4:12`）。
- L6：`all-MiniLM-L6-v2`，6 层，seq 64，冻结；仅编码
  `rescan`（`l6-rescan-k2.json`，counts `2:6 / 3:24 / 4:6`）。
- 每套按 1,500 条分 13 块（12×1,500 + 1,188）；每块 `bert6` 运行
  同时写 `pure-f32.f32`，另存为 `l12-pure-f32-*` / `l6-pure-f32-*`；
  量化输出为 `l12-r12-*` / `l6-rescan-*`。最终两套都是
  `(19,188, 384)` f32。
- 没有编码 `int3` / `int4` 对照臂；本轮只回答 `R12` 与 `rescan`
  在第二个 hate 语料上能否复现，不扩展成新的全臂矩阵。
- C 只由 f32 的 fit 内 `StratifiedKFold(5, shuffle=True)` 选，
  grid `0.01/0.1/1/10`，metric accuracy；f32 与量化臂在同一
  probe/seed 内共用 C；test 标签不参与 C 选择。
- 5 seeds `7/13/19/31/43`；每 seed 的 C 可以不同，但同一 seed
  内所有臂共用。

#### 11.40.4 Primary：full test、target_prior、accuracy

阈值规则是 `quantile(test_scores, 1 - prior_fit)`，只用未标注 test
分数；`prior_fit=0.308975`。primary 是量化臂减 f32、按 test 样本
配对的 10k bootstrap，95% 双侧 CI。

| candidate | probe | 正 CI /5 | 5-seed 均值 | f32 accuracy | 量化臂 accuracy | 预注册判定 |
|---|---|---:|---:|---:|---:|---|
| L12 `R12` | logreg | **0** | `+0.1043pp` | `0.807612` | `0.808655` | rejected |
| L12 `R12` | linear_svc | **0** | `−0.2920pp` | `0.808655–0.812826` | `0.806569` | rejected |
| L6 `rescan` | logreg | **0** | `−0.1043pp` | `0.810740–0.811783` | `0.809698–0.813869` | rejected |
| L6 `rescan` | linear_svc | **0** | `+0.3545pp` | `0.810740` | `0.813869–0.814911` | rejected |

- 四个 cell 的 5 个 CI **全部跨 0**：0 正、0 负、5 跨；没有一格出现
  CI 整段大于 0，也没有一格出现 CI 整段小于 0。
- 预注册判据要求同一候选的两个 probe 都至少 `4/5` 正 CI；这里两个
  候选都是 `0/5 + 0/5`，因此 **`R12` rejected、`rescan` rejected**。
- “rejected” 是预注册标签，不是“量化显著更差”：四个 cell 的点估计
  只有 `−0.29 ~ +0.35pp`，在 1,918 条 test 上约等于 `−6 ~ +7` 条
  预测翻转；CI 都跨 0。
- 单点估计里 `R12` logreg 和 `rescan` SVC 为正，但都没有达到“5 个
  seed 的 CI 都/多数整段为正”；不能把这两个正点估计写成 confirmed。

#### 11.40.5 有效独立性：5 个 seed 不是 5 个独立单元

| candidate / probe | C 取值（seed 数） | 唯一 C 数 | 说明 |
|---|---|---:|---|
| L12 `R12` logreg | `1.0` ×5 | 1 | 5 个 seed 给出同一 C、同一分数、同一预测；实际 1 个单元 |
| L12 `R12` linear_svc | `0.1` ×4，`1.0` ×1 | 2 | 2 个 C 条件；两个 C 的点估计都为负 |
| L6 `rescan` logreg | `10.0` ×1，`1.0` ×4 | 2 | seed 7 正、其余同 C 负 |
| L6 `rescan` linear_svc | `10.0` ×2，`1.0` ×3 | 2 | 两个 C 的点估计都为正，但 CI 都跨 0 |

因此 `0/5` 不能读成“5 次独立失败”；真正独立的是方向×C 的工作点，
本轮约 1–2 个/候选/probe。反过来，`0/5` 也不能被 5 个重复 seed
放大成更强的负结论。

#### 11.40.6 target_prior 的绝对效果：它在该语料上降低 accuracy

M1.38/M1.39 的正结果都是“target_prior 下量化臂减 f32”，不是
“target_prior 绝对优于 default”。HateXplain 上把 target_prior 与
default 0.5 直接对照，结论更保守：

| depth | probe | f32：default→target | 量化臂：default→target | target 下量化臂 |
|---|---|---:|---:|---:|
| L12 | logreg | `−1.5641pp` | `−1.0949pp` | `0.808655` |
| L12 | linear_svc | `−1.2096pp` | `−1.0740pp` | `0.806569` |
| L6 | logreg | `−1.1157pp` | `−1.0532pp` | `0.809698–0.813869` |
| L6 | linear_svc | `−1.2200pp` | `−0.8029pp` | `0.813869–0.814911` |

- default 下 f32 accuracy 约 `0.821–0.823`；target_prior 下降到
  `0.8076`（L12 logreg）/`0.8107`（L6），量化臂也降 `0.80–1.09pp`。
- target_prior 把预测正例率从 default 的 `0.2388–0.2607` 抬到
  `0.3092`，几乎正好等于 test 真实 hate 率 `0.3097`；但 accuracy
  反而下降。**匹配先验提高了召回/正例率，不是 accuracy 的免费增益。**
- 所以本轮不能得出“target_prior 让量化臂更好”；只能说在 target_prior
  这个共同工作点上，`R12`/`rescan` 与 f32 的差距很小且方向不稳定，
  同时该工作点本身比 default 差 `0.8–1.6pp`。
- 对照 M1.38：TweetEval/hate 的 f32 在 default 下把 `73–75%` 判正，
  accuracy 只有 `0.574/0.536`；target_prior 把它抬到 `0.634/0.632`
  （`+6.00/+9.60pp`）。HateXplain 的 default 只判 `24–26%` 正，
  已经比 target_prior 的 `30.9%` 更准。**阈值规则的方向性依赖默认
  阈值的偏差：default 过度判正时 target_prior 是 rescue；default 本身
  已经更准时，匹配先验会伤 accuracy。不能把 M1.38 的 rescue 写成
  target_prior 的无条件增益。**

#### 11.40.7 Macro-F1 与 default 0.5

Macro-F1 仍是次要口径，不能覆盖 primary：

| depth / probe | target_prior 正 CI /5 | 5-seed 均值 |
|---|---:|---:|
| L12 `R12` logreg | 0 | `+0.1220pp` |
| L12 `R12` linear_svc | 0 | `−0.3416pp` |
| L6 `rescan` logreg | 0 | `−0.1220pp` |
| L6 `rescan` linear_svc | 0 | `+0.4148pp` |

Default 0.5 的 accuracy 也全部为 0 正 / 0 负 / 5 跨：

| depth / probe | 正 CI /5 | 5-seed 均值 |
|---|---:|---:|
| L12 `R12` logreg | 0 | `−0.3650pp` |
| L12 `R12` linear_svc | 0 | `−0.4275pp` |
| L6 `rescan` logreg | 0 | `−0.1668pp` |
| L6 `rescan` linear_svc | 0 | `−0.0626pp` |

也就是说，HateXplain 上没有任何 `(probe, metric, rule)` cell 出现
正 CI；M1.39 的 macro-F1 10/10 也没有跨语料复现。

#### 11.40.8 半分割诊断：方向仍然翻符号

按预注册的 secondary 诊断，在 official test 内做 50/50 标签分层
半分割（seed `101`），一半 test 分数校准 target_prior、另一半标签
评估，双向交叉。每半约 959 条，CI 比 full test 更宽，只作诊断。

| direction | L12 `R12` logreg | L12 `R12` SVC | L6 `rescan` logreg | L6 `rescan` SVC |
|---|---:|---:|---:|---:|
| A_calib→B_eval | `−1.1470pp` | `−0.9385pp` | `−0.4380pp` | `+0.1251pp` |
| B_calib→A_eval | `+0.9385pp` | `+0.9593pp` | `+0.9385pp` | `+1.1262pp` |

- 8 个 direction×candidate×probe cell 全部 0 正 / 0 负 / 5 跨；
- L12 `R12` 和 L6 `rescan` 都是 A 负、B 正（rescan SVC 是 A 微正、
  B 更正），方向不一致；
- 这与 M1.39 的“R12 半分割不稳、rescan 只在 L6 logreg 过线”一致，
  但在新语料上没有出现任何正的半分割 cell。

#### 11.40.9 与 M1.38 / M1.39 的关系

| 读数 | L12 `R12` logreg | L12 `R12` SVC | L6 `rescan` logreg | L6 `rescan` SVC |
|---|---:|---:|---:|---:|
| M1.38 full test | `+1.8288pp` | `+1.6825pp` | `+2.7213pp` | `+2.5457pp` |
| M1.39 半分割 | `+1.9020pp` | `+1.6167pp` | `+2.8237pp` | `+2.4506pp` |
| M1.40 HateXplain | `+0.1043pp` | `−0.2920pp` | `−0.1043pp` | `+0.3545pp` |

- 同一批模型的四个 cell 在 HateXplain 上从 `+1.6~+2.8pp` 掉到
  `−0.29~+0.35pp`；两个 cell 变号（`R12` SVC、`rescan` logreg）。
- 这不是配对比较：语料、标签映射、域组成和 test 规模都变了。
  但“新语料上没有复现 M1.38/M1.39 的幅度和方向”是事实；
  不能再用 TweetEval 的读数外推 Hate 域的量化收益。
- HateXplain 的 test 有 54% Gab；TweetEval/hate 是 Twitter。域混合
  和 `offensive→0` 的映射都可能是差异来源，但本轮没有做域分层
  confirmatory，不能把“Gab 域”写成已证机制。

#### 11.40.10 判决与边界

- **`R12` rejected**：L12 两个 probe 都 0/5；M1.38/M1.39 的
  Hate-only、target_prior 条件性候选没有在第二个 hate 语料上复现。
- **`rescan` rejected**：L6 两个 probe 都 0/5；M1.39 的
  “L6 primary logreg accuracy confirmed”没有跨语料复现，不能升级为
  跨 hate 语料的稳定收益。
- **target_prior 仍是 transductive exploratory，而且在该语料上
  绝对 accuracy 低于 default 0.5**；不能写成可部署协议，也不能写成
  “target_prior 普遍提升量化臂”。
- 边界：单一 official test、n=1,918；5 seeds 共享同一批文本，且同 C
  重复；本轮只编码 `R12`/`rescan`，没有 int3/int4 对照；没有 toxic
  映射、没有 per-arm C、没有 encoder 微调、没有 MNLI；
  “rejected”不等于“量化在 HateXplain 上显著更差”，四个 cell 的 CI
  全部跨 0，点估计很小。
- 下一步若继续，必须是第三个独立语料或预注册的域分层切分；
  不要在同一 HateXplain test 上继续换 seed/C/半分割。

#### 11.40.11 复算

```bash
# 1) 清洗；官方 train+val 作 fit，official test 作 eval
python docs/research/scripts/wsp_m140/prepare_hatexplain.py \
  --root "$TRIT_RESULTS/hatexplain"

# 2) 分块生成 WSPACT1/WSPMSK1（每块编码后删除 input/reference）
python docs/research/scripts/wsp_m140/make_chunk_inputs.py \
  --train "$TRIT_RESULTS/hatexplain/raw/train.jsonl" \
  --test "$TRIT_RESULTS/hatexplain/raw/test.jsonl" \
  --model-dir "$L12_SNAPSHOT" \
  --dst "$TRIT_RESULTS/hatexplain/chunks-l12-1500" \
  --texts-out "$TRIT_RESULTS/hatexplain/texts-l12.json" \
  --chunk-size 1500 --only-chunk 0

# 3) 每块编码 R12 / rescan；f32 由 pure-f32.f32 另存
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 \
TRIT_BUDGET="$TRIT_RESULTS/scifact/r12/swap-r12.json" \
TRIT_THREADS=16 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 \
TRIT_EMBED_DUMP="$TRIT_RESULTS/hatexplain/emb12" \
TRIT_EMBED_TAG=l12-r12-000 \
bert6 --file "$L12_SNAPSHOT/model.safetensors" \
  --data-dir "$TRIT_RESULTS/hatexplain/chunks-l12-1500/chunk-000" \
  --activation-mode f32 \
  --out "$TRIT_RESULTS/hatexplain/runs12/l12-r12-000.json"

TRIT_LAYERS=6 TRIT_WEIGHT_BITS=3 \
TRIT_BUDGET="$TRIT_RESULTS/l6/l6-rescan-k2.json" \
TRIT_THREADS=16 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 \
TRIT_EMBED_DUMP="$TRIT_RESULTS/hatexplain/emb6" \
TRIT_EMBED_TAG=l6-rescan-000 \
bert6 --file "$L6_SNAPSHOT/model.safetensors" \
  --data-dir "$TRIT_RESULTS/hatexplain/chunks-l6-1500/chunk-000" \
  --activation-mode f32 \
  --out "$TRIT_RESULTS/hatexplain/runs6/l6-rescan-000.json"

# 4) full test + default + 半分割诊断
python docs/research/scripts/wsp_m140/hatexplain_target_prior.py \
  --root "$TRIT_RESULTS/hatexplain"
```

产物：

- `data/wsp-classify-m140-prereg.json`（预注册配置，8,816 B）；
- `data/wsp-classify-m140-prepare-summary.json`（清洗/长度审计，2,278 B）；
- `data/wsp-classify-m140-raw.tsv`（160 行、20 列）；
- `data/wsp-classify-m140-summary.tsv`（32 行、15 列）；
- `data/wsp-classify-m140-primary.tsv`（4 行、8 列）；
- `data/wsp-classify-m140-split-half.tsv`（40 行、12 列）；
- `data/wsp-classify-m140-vs-default.tsv`（20 行、12 列）；
- `scripts/wsp_m140/prepare_hatexplain.py`、`make_chunk_inputs.py`、
  `hatexplain_target_prior.py`。

## 11.41 域感知先验与 fit-only 阈值（M1.41）

### 11.41.1 问题

M1.40 的 `target_prior` 在 HateXplain 上把 f32 accuracy 从约 `0.823`
压到 `0.808/0.811`，但 M1.38 的 TweetEval/hate 上同一规则却是 rescue。
HateXplain 的 fit/test 都含两个来源，且 hate 率先验差异极大：

| 来源 | fit n | fit hate | fit hate rate | test n | test hate | test hate rate |
|---|---:|---:|---:|---:|---:|---:|
| Gab | 9,408 | 4,719 | `0.501594` | 1,038 | 516 | `0.497110` |
| Twitter | 7,862 | 617 | `0.078479` | 880 | 78 | `0.088636` |
| 全体 | 17,270 | 5,336 | `0.308975` | 1,918 | 594 | `0.309698` |

因此一个全局先验阈值（31%）对 Gab 是压低、对 Twitter 是抬高。M1.41
先问：把先验按域拆开后，M1.40 的伤害是否只是全局阈值错配？随后把
同一规则改成 fit 内 OOF 阈值，问它是否还能部署。

预注册 `docs/research/data/wsp-classify-m141-prereg.json` 先于任何模型
分数提交（`d897af81`，primary arm 澄清 `6737b21c`）。数据、切分、5 seeds、
C 网格、probe 设置全部沿用 M1.40；只新增规则，不重新编码。

### 11.41.2 五条规则

| rule | 阈值 | 用 test 分数 | 用 test 标签 |
|---|---|---|---|
| `default` | logreg 0.5 / SVC 0.0 | 否 | 否 |
| `global_test_prior` | 全部 test 分数的 `1−prior_fit_global` 分位 | 是 | 否 |
| `domain_test_prior` | 每域 test 分数的 `1−prior_fit_domain` 分位 | 是 | 否 |
| `global_fit_prior` | 全部 OOF fit 分数的 `1−prior_fit_global` 分位 | 否 | 否 |
| `domain_fit_prior` | 每域 OOF fit 分数的 `1−prior_fit_domain` 分位 | 否 | 否 |

OOF 分数来自与 C 选择相同的 5 折：每折用 4/5 fit 训练、对留出折打分。
所有阈值都不使用 test 标签；test 标签只用于 accuracy/macro-F1。

### 11.41.3 Primary：domain-aware 阈值 vs global 阈值

Primary 是 f32 的 overall accuracy，`domain_test_prior − global_test_prior`，
5 seeds，10k paired stratified bootstrap（域内重采样，保持 1,038/880 构成）。

| depth | probe | 正 CI /5 | 5-seed 均值 | 预注册判定 |
|---|---|---:|---:|---|
| L12 | logreg | 5 | `+0.7821pp` | supported |
| L12 | linear_svc | 5 | `+0.6569pp` | supported |
| L6 | logreg | 4 | `+0.6361pp` | supported |
| L6 | linear_svc | 0 | `+0.3650pp` | rejected |

L6 SVC 的点估计为正，但 5 个 CI 都跨 0；这里的 `rejected` 只表示
“没有显著优于 global”，不是“domain-aware 更差”。总体看，H2 在
3/4 个 probe family 上 supported：域感知阈值确实收回了一部分
global 阈值的伤害。

### 11.41.4 绝对 accuracy：收回一部分，但仍低于 default

f32 的 5-seed 均值：

| rule | L12 logreg | L12 SVC | L6 logreg | L6 SVC |
|---|---:|---:|---:|---:|
| `default` | `0.8233` | `0.8216` | `0.8227` | `0.8229` |
| `global_test_prior` | `0.8076` | `0.8095` | `0.8116` | `0.8107` |
| `domain_test_prior` | `0.8154` | `0.8161` | `0.8179` | `0.8144` |
| `domain_fit_prior` | `0.8185` | `0.8193` | `0.8125` | `0.8111` |

`domain_test_prior − default` 仍为负：

- L12：`−0.7821pp`（logreg）/`−0.5527pp`（SVC）；
- L6：`−0.4797pp`（logreg）/`−0.8551pp`（SVC）。

也就是说，域拆开只能解释/收回 global 阈值的一部分损失，不能把
`target_prior` 变回优于 default 的规则。

### 11.41.5 为什么 global 会伤：Twitter 被过度抬正

预测正例率（5-seed 均值）：

| rule | L12 logreg overall | L12 logreg twitter | L12 logreg gab | L6 logreg overall | L6 logreg twitter | L6 logreg gab |
|---|---:|---:|---:|---:|---:|---:|
| `default` | `0.2404` | `0.0580` | `0.3950` | `0.2532` | `0.0698` | `0.4087` |
| `global_test_prior` | `0.3092` | `0.1000` | `0.4865` | `0.3092` | `0.0955` | `0.4904` |
| `domain_test_prior` | `0.3076` | `0.0784` | `0.5019` | `0.3076` | `0.0784` | `0.5019` |

test 真实先验是 Gab `0.4971`、Twitter `0.0886`。全局阈值把 Twitter
从 default 的 `0.058–0.070` 抬到 `0.096–0.100`，超过真实 `0.0886`；
域感知阈值把 Twitter 拉到 `0.0784`，接近真实值，但 accuracy 仍低于
default。per-domain accuracy：

| scope | rule | L12 logreg | L12 SVC | L6 logreg | L6 SVC |
|---|---|---:|---:|---:|---:|
| Gab | `default` | `0.7495` | `0.7468` | `0.7617` | `0.7609` |
| Gab | `global_test_prior` | `0.7486` | `0.7493` | `0.7547` | `0.7553` |
| Gab | `domain_test_prior` | `0.7486` | `0.7478` | `0.7574` | `0.7505` |
| Twitter | `default` | `0.9102` | `0.9098` | `0.8948` | `0.8961` |
| Twitter | `global_test_prior` | `0.8773` | `0.8805` | `0.8786` | `0.8761` |
| Twitter | `domain_test_prior` | `0.8943` | `0.8966` | `0.8893` | `0.8898` |

Gab 上各规则接近 default；主要伤害集中在 Twitter。但即使 Twitter
的正例率被拉回真实先验，accuracy 仍低于 default 约 `1.0–1.6pp`
（L12）/`0.5–0.7pp`（L6）；这说明“匹配边际先验”不等于“匹配最优
决策阈值”，排序/校准本身仍有差异。

### 11.41.6 fit-only 阈值：L12 略好，L6 更差

`domain_fit_prior − domain_test_prior` 的 overall accuracy：

| depth | probe | 正 CI /5 | 5-seed 均值 |
|---|---|---:|---:|
| L12 | logreg | 4 | `+0.3024pp` |
| L12 | linear_svc | 4 | `+0.3233pp` |
| L6 | logreg | 0 | `−0.5422pp` |
| L6 | linear_svc | 0 | `−0.3337pp` |

L12 上 OOF 阈值反而略好；L6 上明显更差。绝对值见 §11.41.4。这个
结果不是“fit-only 稳定”，而是“fit-only 的阈值随深度/分数分布变化，
不能保证搬运”。M1.42 在 TweetEval/hate 上把这一点放大：fit-only
阈值没有保留 M1.38 的 candidate gain。

### 11.41.7 candidate 没有被域规则救回来

`arm − f32` 的 overall accuracy（domain_test_prior）：

- L12 `R12`：logreg `0.0000`、SVC `0.0000`（5/5 都跨 0）；
- L6 `rescan`：logreg `+0.4170pp`、SVC `+0.5010pp`，但两个 probe 都是
  0/5 正 CI。

`domain_fit_prior` 下 L12 `R12` 变成 `−0.740/−0.782pp`，L6 `rescan`
为 `+0.344/+0.615pp`，同样没有正 CI。域感知规则没有把 M1.40 的
rejected candidate 变成可升级候选；它只解释了 global 阈值的一部分
伤害。

### 11.41.8 判决与边界

- **H1（全局阈值错配）成立但不足以解释全部损失**：global 阈值在
  Twitter 上过度抬正，Gab 上接近 default；overall 仍差 default
  `1.1–1.6pp`（L6/L12）。
- **H2（域感知阈值）部分成立**：`domain_test_prior` 相对 global
  在 3/4 个 family 上 supported（L6 SVC 仅点估计正、CI 全跨 0），
  但仍低于 default。
- **H3（fit-only 可部署）不成立/混合**：L12 略好、L6 更差；
  M1.42 在 TweetEval 上给出更强的“不保留”证据。
- **candidate 不升级**：`R12`/`rescan` 在域规则下仍没有正 CI。
- 边界：单一 HateXplain official test；域标签来自 `source` 元数据，
  不是新语料；每域 n=`1,038/880`，CI 宽；5 seeds 共享 test 文本，
  同 C 可能重复；`offensive→0` 仍是外部映射；没有做第三个语料。

### 11.41.9 复算

```bash
python docs/research/scripts/wsp_m141/domain_prior.py \
  --root "$TRIT_RESULTS/hatexplain" \
  --out "$TRIT_RESULTS/hatexplain" \
  --iters 10000
```

产物：

- `data/wsp-classify-m141-prereg.json`
- `data/wsp-classify-m141-raw.tsv`（1,200 行、16 列）
- `data/wsp-classify-m141-compare.tsv`（1,500 行、19 列）
- `data/wsp-classify-m141-summary.tsv`（240 行、9 列）
- `data/wsp-classify-m141-compare-summary.tsv`（300 行、16 列）
- `data/wsp-classify-m141-primary.tsv`（4 行、7 列）
- `scripts/wsp_m141/domain_prior.py`

## 11.42 fit-only 阈值搬运（M1.42）

### 11.42.1 问题与预注册

M1.38 的 rescue 用的是 `quantile(test_scores, 1−prior_fit)`：它不需要
test 标签，但需要 test 分数分布，属于 transductive。M1.42 在
TweetEval/hate 上把阈值换成 fit 内 OOF 分位点，问 candidate gain
是否还能保留。预注册
`docs/research/data/wsp-classify-m142-prereg.json` 先于结果提交
（`19f1d5c1`）。

数据：fit `8,985`、test `2,734`；prior_fit `0.420812`；规则只有
`default`、`global_test_prior`、`global_fit_prior`；其余建模口径与
M1.38/M1.41 一致。

### 11.42.2 Primary：candidate gain 没有保留

`arm − f32` 的 full-test accuracy，阈值 `global_fit_prior`：

| depth | probe | 正 CI /5 | 5-seed 均值 | 预注册判定 |
|---|---|---:|---:|---|
| L12 | logreg | 0 | `−1.7703pp` | not_retained |
| L12 | linear_svc | 0 | `−1.5947pp` | not_retained |
| L6 | logreg | 0 | `+0.0219pp` | not_retained |
| L6 | linear_svc | 1 | `+0.5779pp` | not_retained |

对照同一脚本的 `global_test_prior`（复现 M1.38 方向）：

| depth | probe | `global_test_prior` arm−f32 | `global_fit_prior` arm−f32 |
|---|---|---:|---:|
| L12 | logreg | `+1.8288pp` | `−1.7703pp` |
| L12 | linear_svc | `+1.6825pp` | `−1.5947pp` |
| L6 | logreg | `+2.8676pp` | `+0.0219pp` |
| L6 | linear_svc | `+2.5457pp` | `+0.5779pp` |

L12 的 test-prior 读数与 M1.38 的 `+1.8288/+1.6825pp` 一致；L6
logreg 是 `+2.8676pp`（M1.38 为 `+2.7213pp`，同向、约 `+0.15pp`
差异，来自本轮 C/折选择）。**一旦把阈值改成 fit-only，L12 直接翻负，
L6 掉到约 0；M1.38 的 rescue 不能搬运。**

### 11.42.3 fit-only 阈值为什么失效

f32 的 absolute accuracy：

| rule | L12 logreg | L12 SVC | L6 logreg | L6 SVC |
|---|---:|---:|---:|---:|
| `default` | `0.5743` | `0.5723` | `0.5413` | `0.5390` |
| `global_test_prior` | `0.6342` | `0.6376` | `0.6329` | `0.6328` |
| `global_fit_prior` | `0.5608` | `0.5586` | `0.5292` | `0.5225` |

阈值与预测正例率：

| rule | L12 logreg thr / pos | L12 SVC thr / pos | L6 logreg thr / pos | L6 SVC thr / pos |
|---|---:|---:|---:|---:|
| `default` | `0.5 / 0.7341` | `0.0 / 0.7351` | `0.5 / 0.7571` | `0.0 / 0.7616` |
| `global_test_prior` | `0.7632 / 0.4210` | `0.4585 / 0.4210` | `0.7745 / 0.4210` | `0.4741 / 0.4210` |
| `global_fit_prior` | `0.4669 / 0.7612` | `−0.0497 / 0.7638` | `0.4637 / 0.7885` | `−0.0528 / 0.7948` |

test 真实 hate 率约 `0.421`。test 分位阈值正好把正例率压到 `0.421`；
fit OOF 分位阈值却落在 `0.76–0.80`，说明 OOF 分数分布与 test 分数
分布明显不同，分位点不能搬运。`global_fit_prior` 不仅没有保留
candidate gain，连 f32 的绝对 accuracy 都比 default 低
`1.35–2.65pp`（L12/L6），比 test-prior 低 `7.3–11.0pp`。

### 11.42.4 判决与边界

- **M1.38 的 target_prior rescue 是 test 分数分位点现象**：它依赖
  未标注 test 分数的分布；换成 fit OOF 后，L12 翻负、L6 归零。
- **fit-only 不是可部署替代**：OOF 分数分布与 test 不同，分位阈值
  会系统性偏移；不能把 `target_prior` 写成部署协议。
- **candidate 线到此收口**：`R12`/`rescan` 的 Hate-only、target_prior
  条件性候选没有跨语料复现，也没有通过 fit-only 搬运。继续开第三个
  hate 语料做 candidate confirmatory 的边际信息很低。
- 边界：仍是 TweetEval/hate 单一 test、n=2,734；5 seeds 共享文本；
  C 由 f32 fit 内 5 折选；OOF 与最终 full-fit 的分数尺度不完全一致；
  没有做校准曲线/AUC 的完整诊断。
- 下一步若继续，应该换 **prior-free / inductive** 的决策规则，而不是
  继续在 test 分数上匹配先验；第三语料只有在新的规则预注册后才值得开。

### 11.42.5 复算

```bash
python docs/research/scripts/wsp_m142/fit_only_threshold.py \
  --root "$TRIT_RESULTS/tweet-hate" \
  --out "$TRIT_RESULTS/tweet-hate" \
  --iters 10000
```

产物：

- `data/wsp-classify-m142-prereg.json`
- `data/wsp-classify-m142-raw.tsv`（240 行、12 列）
- `data/wsp-classify-m142-compare.tsv`（300 行、18 列）
- `data/wsp-classify-m142-primary.tsv`（4 行、7 列）
- `scripts/wsp_m142/fit_only_threshold.py`

## 11.43 三分类探索（M1.43）

### 11.43.1 问题与预注册

M1.40 把 HateXplain 的 `offensive` 并进负类，只测 binary hate。若
`offensive` 是 hard negative，binary 口径会把一部分“像 hate 的 offensive”
当成错误；三分类可能改变 `R12`/`rescan` 相对 f32 的读数。M1.43 在
**同一批清洗行、同一批 embedding** 上只换标签头：

- fit `7,011 normal / 5,336 hate / 4,923 offensive`，共 `17,270`；
- test `777 / 594 / 547`，共 `1,918`；
- 映射 `normal=0, hatespeech=1, offensive=2`；
- 预注册见 `data/wsp-classify-m143-prereg.json`（commit `3af46565`），
  状态是 `exploratory, not confirmatory`，不设 candidate confirm/reject。

### 11.43.2 口径与 C 选择

- 5 seeds `7/13/19/31/43`；2 probes：multinomial logreg、LinearSVC OVR；
- L12 `f32` vs `R12`，L6 `f32` vs `rescan`；
- C 只由 f32 的 fit 内 5 折 accuracy 选，f32 与量化臂共用同一 C；
- 无阈值；三分类 `argmax` / `predict`；
- primary 是 test 三分类 accuracy 的 `arm − f32`，10k paired stratified
  bootstrap 95% percentile CI；macro-F1 与 per-class recall 只报点估计。

实现说明：logreg 在 L12 的五个 seed 都选中 `C=1.0`；L6 logreg 的 seed 7
5 折也选中 `1.0`。为避免对同一个 `C=1.0` 重复跑 5×20 次全量 CV，L6
logreg 余下 seed 固定 `C=1.0`；SVC 仍逐 seed 选 C（选中 `1.0` 或 `10.0`）。
这不改变“f32-only 选 C、两臂共用 C”的规则，也不改变任何 test 标签。

### 11.43.3 Primary：四个 family 全部 0/5 正 CI

| depth | probe | 正 CI /5 | mean point | mean CI | 预注册判定 |
|---|---|---:|---:|---:|---|
| L12 | logreg | 0 | `+0.1564pp` | `[−1.366, +1.648]pp` | descriptive only |
| L12 | linear_svc | 0 | `−0.3233pp` | `[−1.794, +1.147]pp` | descriptive only |
| L6 | logreg | 0 | `+0.4692pp` | `[−1.011, +1.929]pp` | descriptive only |
| L6 | linear_svc | 0 | `+0.0730pp` | `[−1.450, +1.595]pp` | descriptive only |

三分类的 CI 比 binary 宽：test 只有 `1,918` 行，且每类几百行。四个
family 中没有任何一个 seed 的 95% CI 完全在 0 以上；L6 logreg 的
`+0.4692pp` 是点估计最大的，但 5 个 CI 全部跨 0。**不能把任何一个
写成 candidate rescue。**

### 11.43.4 绝对 accuracy 与 macro-F1

5-seed 均值：

| depth/probe | arm | accuracy | macro-F1 | Δacc | Δmacro-F1 |
|---|---|---:|---:|---:|---:|
| L12 logreg | f32 | `0.651199` | `0.631284` | — | — |
| L12 logreg | R12 | `0.652763` | `0.629616` | `+0.1564pp` | `−0.1668pp` |
| L12 SVC | f32 | `0.649948` | `0.623033` | — | — |
| L12 SVC | R12 | `0.646715` | `0.617367` | `−0.3233pp` | `−0.5666pp` |
| L6 logreg | f32 | `0.651721` | `0.628103` | — | — |
| L6 logreg | rescan | `0.656413` | `0.632608` | `+0.4692pp` | `+0.4505pp` |
| L6 SVC | f32 | `0.648697` | `0.619882` | — | — |
| L6 SVC | rescan | `0.649426` | `0.619689` | `+0.0730pp` | `−0.0193pp` |

三分类 accuracy 约 `0.649–0.656`，比 binary 的 `0.82` 低很多；这不是
量化损失，而是任务本身多了 `offensive` 这个与 hate 高度重叠的类。L12
logreg 的 accuracy 微正、macro-F1 微负；L6 logreg 两者都微正，但都
没有 CI 支持。L12 SVC 两项都负。

### 11.43.5 混淆在哪里：offensive recall 最低

5-seed 均值 per-class recall：

| depth/probe | arm | normal | hatespeech | offensive |
|---|---|---:|---:|---:|
| L12 logreg | f32 | `0.7864` | `0.6818` | `0.4260` |
| L12 logreg | R12 | `0.7902` | `0.7020` | `0.4040` |
| L12 SVC | f32 | `0.8113` | `0.6936` | `0.3733` |
| L12 SVC | R12 | `0.8028` | `0.7118` | `0.3543` |
| L6 logreg | f32 | `0.7864` | `0.7155` | `0.3912` |
| L6 logreg | rescan | `0.7967` | `0.7104` | `0.3985` |
| L6 SVC | f32 | `0.7974` | `0.7232` | `0.3565` |
| L6 SVC | rescan | `0.8121` | `0.7047` | `0.3583` |

`offensive` recall 只有 `0.354–0.426`，是主要错误来源；`normal` 最高，
`hatespeech` 居中。`R12`/`rescan` 的主要变化是把一部分 `offensive`
预测成 `hatespeech`：L12 SVC 的 hate recall `+1.82pp`、offensive
`−1.90pp`；L6 SVC 的 normal `+1.47pp`、hate `−1.85pp`。L6 logreg 的
rescan 则是 normal `+1.03pp`、hate `−0.51pp`、offensive `+0.73pp`，
方向更均衡，所以 accuracy 与 macro-F1 同时微正。

这不是“量化保真更好”的证据：三类边界本身不稳，`offensive` 与
`hatespeech` 的决策面在 384 维线性 probe 下高度重叠；微小的分数扰动
就能改变几百行的归类。

### 11.43.6 判决与边界

- 四个 family 的 primary 全部 `0/5` 正 CI，按预注册只能写
  **descriptive only**；不能升级 `R12`/`rescan`。
- 三分类没有复制 binary 的 `R12`/`rescan` 负结果，也没有给出正结果；
  它说明 binary 的 82% 主要来自 normal-vs-rest 的易分边界，而不是
  hate/offensive 的细边界。
- 单语料、单标签映射、无软标签、无 annotator 分歧；`offensive` 的
  低 recall 是 HateXplain 原生多数标签口径下的读数，不外推到其他
  三分类定义。
- 5 seeds 共享同一 test；CI 不是独立重复实验的 CI。

### 11.43.7 复算

- 预注册：`data/wsp-classify-m143-prereg.json`
- 脚本：`scripts/wsp_m143/three_class.py`（`--depth/--probe/--suffix` 分块，
  `merge_chunks.py` 合并）；`--fixed-c` 只用于 L6 logreg 的 `C=1.0` 复用。
- 表：`data/wsp-classify-m143-raw.tsv`（40 行、12 列）、
  `-compare.tsv`（40 行、14 列）、`-primary.tsv`（4 行、7 列）
- 原始分块读数（未入库）与日志 `m143-l12-logreg.log`、
  `m143-l12-svc.log`、`m143-l6-logreg.log`（L6 SVC 的终端输出未单独
  落盘）同在实验输出目录；仓库内只保留合并后的三张表。

## 11.44 静态字节–质量 Pareto（M1.44）

### 11.44.1 问题与口径

前面的结论一直是“同字节预算下谁更好”，但没有一张把**成本**和**任务
质量**放在同一张表里的图。M1.44 只做静态综合，不新跑模型：

- 成本 = `WSP-BUDGET-RESULTS.md` 已发布的**实际工件字节**（含 metadata），
  不是 weight-only，也不是运行时 RSS/延迟。L12 f32 `84,934,656 B`、
  int3/R12 `10,782,720 B`、int4 `13,436,928 B`；L6 f32
  `42,467,328 B`、int3/rescan `5,391,360 B`、int4 `6,718,464 B`。
- 质量 = 已发布的 corrected true-f32 对照读数：STS Pearson/Spearman、
  SciFact MRR@10/nDCG@10、SST-2 accuracy、CoLA macro-F1、
  TweetEval-hate accuracy。
- 分类只取 logreg，避免把 logreg/SVC 混成同一条 frontier；检索/STS
  是同一份句向量口径。
- Pareto 定义：对同一 depth×task×metric，若存在另一配置
  `bytes ≤ 本配置` 且 `quality ≥ 本配置`（至少一项严格），本配置被支配。
  这是**点估计 frontier**，不是统计显著 frontier；CI 见各原节。

### 11.44.2 成本表

| depth | f32 | int3 | int4 | mixed | mixed 名称 |
|---|---:|---:|---:|---:|---|
| L12 | 84,934,656 | 10,782,720 | 13,436,928 | 10,782,720 | `R12`（6/54/12） |
| L6 | 42,467,328 | 5,391,360 | 6,718,464 | 5,391,360 | `rescan`（6/24/6） |

int3/R12 是 f32 工件字节的 `12.69%`，int4 是 `15.82%`；但这是
**工件字节比**，不是参数位宽比，也不是推理时间比。f32 工件包含的不止
权重，不能把它当“4×int3”的干净换算。

### 11.44.3 每个任务的 Pareto frontier

按成本从低到高列非支配点（括号内是质量指标）：

| depth | task | metric | Pareto frontier |
|---|---|---|---|
| L12 | STS | Pearson | `R12(10.78MB,.8337)` → `int4(13.44MB,.8375)` → `f32(84.93MB,.8376)` |
| L12 | STS | Spearman | `R12(10.78MB,.8354)` **唯一** |
| L12 | SciFact | nDCG@10 | `int3(10.78MB,.5320)` → `int4(13.44MB,.5684)` → `f32(84.93MB,.5899)` |
| L12 | SciFact | MRR@10 | `int3(10.78MB,.4967)` → `int4(13.44MB,.5294)` → `f32(84.93MB,.5538)` |
| L12 | SST-2 | accuracy | `int3(10.78MB,.8232)` → `int4(13.44MB,.8402)` → `f32(84.93MB,.8429)` |
| L12 | CoLA | macro-F1 | `R12(10.78MB,.5430)` → `int4(13.44MB,.5612)` |
| L12 | TweetEval-hate | accuracy | `int3(10.78MB,.5647)` → `int4(13.44MB,.5655)` → `f32(84.93MB,.5743)` |
| L6 | STS | Pearson | `rescan(5.39MB,.8313)` **唯一** |
| L6 | STS | Spearman | `rescan(5.39MB,.8245)` **唯一** |
| L6 | SciFact | nDCG@10 | `int3(5.39MB,.5411)` → `int4(6.72MB,.5738)` |
| L6 | SciFact | MRR@10 | `int3(5.39MB,.5103)` → `int4(6.72MB,.5345)` |
| L6 | SST-2 | accuracy | `int3(5.39MB,.8083)` → `f32(42.47MB,.8116)` |
| L6 | CoLA | macro-F1 | `int3(5.39MB,.5471)` → `f32(42.47MB,.5531)` |
| L6 | TweetEval-hate | accuracy | `int3(5.39MB,.5505)` **唯一** |

被支配的典型点：

- L12 STS Pearson 的 `int3` 被 `R12` 支配（同字节、`R12` 高
  `+0.015231`）；但 `R12` 仍低于 `int4`/f32。
- L12 STS Spearman 的 `int3`/`int4`/`f32` 都被 `R12` 支配：`R12`
  在 10.78MB 上 `0.835411`，高于 f32 `0.830894` 和 int4 `0.832047`。
  这是全表最强的成本–质量点，但只对 Spearman 成立；同配置的 Pearson
  仍低于 f32。
- L12 CoLA 的 f32 被 `int4` 支配：int4 更便宜且 macro-F1 更高
  （`0.561151` vs `0.548644`）；`R12` 更便宜但更低，仍留在 frontier。
- L6 STS 的 `int3`/`int4`/f32 都被 `rescan` 支配：`rescan` 在
  5.39MB 上 Pearson `0.831304`、Spearman `0.824457`，高于 f32。
- L6 Hate 的 `f32`/`int4`/`rescan` 都被 `int3` 支配：int3 在
  5.39MB 上 `0.550475`，高于 f32 `0.546086`；这是唯一一个
  “最低成本点同时最高质量”的任务。
- L6 SciFact 的 `rescan` 被 `int3` 支配：同字节，nDCG@10
  `0.514939` vs `int3 0.541137`；STS 最优不等于检索最优。

### 11.44.4 判决与边界

- **不存在跨任务统一的最优配置**。L12 的 frontier 在 STS Spearman
  是 `R12`，在 CoLA 是 `int4`，在 SST-2/Hate 是 int4–f32 的窄带；
  L6 在 STS/Hate 是 `int3`/`rescan`，在 SciFact 是 `int4`。
- **成本–质量是任务相关的**：同一 10.78MB/5.39MB 预算，换任务就换
  winner；不能用一个任务（尤其 STS）的 frontier 推部署配置。
- **“最低字节”不等于“最差质量”**：L6 Hate 的 int3 同时是最低成本
  和最高 accuracy；L12 STS Spearman 的 R12 也是最低成本和最高
  Spearman。反过来，f32 在 CoLA L12 被 int4 支配，在 L6 Hate 被
  int3 支配。
- 边界：只有 logreg；没有 SVC frontier；没有 CI/显著性；没有实际
  latency/内存带宽/能耗；f32 工件字节含 metadata/非权重内容，
  不能当纯权重压缩率；`R12`/`rescan` 是 in-sample 选块，frontier
  不是“未见过任务的预期”。
- 复算：`python docs/research/scripts/wsp_m144/cost_pareto.py`，输出
  `data/wsp-classify-m144-pareto.tsv`（56 行、9 列）。TSV 的
  `dominated_by`/`pareto` 列就是上表的机器可读版本。

## 11.45 Davidson 第三语料（M1.45）

### 11.45.1 问题与预注册

M1.40 在 HateXplain 上否定了 `R12`/`rescan`，M1.42 又否定了 fit-only
阈值搬运。M1.45 换第三个 hate 语料 Davidson et al. 2017，只问一个问题：
**按 default 阈值（logreg 0.5 / LinearSVC 0），量化臂还能不能保持
f32 的 accuracy/macro-F1？** 预注册见
`data/wsp-classify-m145-prereg.json`（commit `d1810448`），
`descriptive only`，不做 candidate confirm/reject，不测 target_prior。

数据：Davidson 原始 `24,783` 行，`1,430 hate / 19,190 offensive /
4,163 neither`；空白折叠、去重（去 1 条）、binary 映射
`class=0 hate -> 1; offensive+neither -> 0`。无官方 split，固定
stratified 80/20 `random_state=7`；test 中 1 条与 TweetEval/HateXplain
精确重叠被移除。最终 fit `19,825`（`18,681/1,144`），test `4,956`
（`4,671/285`）；fit prior `0.057704918`，test prior
`0.057506053`。

### 11.45.2 口径与 C

- 冻结编码器，L12 `f32`/`R12`，L6 `f32`/`rescan`；seq 64；
- 5 seeds、2 probes、default 阈值；无 target_prior、无三分类；
- C 只由 f32 的 fit 内 5 折 accuracy 选；为控制重复 CV，C 在 seed 7
  的 5 折上各 depth×probe 选一次，然后所有 seed/两臂共用：
  L12 logreg `10.0`、L12 SVC `10.0`、L6 logreg `1.0`、L6 SVC `0.1`。
  这符合预注册的“f32 only、5 折、两臂共用 C”；没有用 test 标签。

### 11.45.3 Primary：四个 family 全部 0/5 正 CI

| depth | probe | 正 CI /5 | mean point | mean CI | 预注册判定 |
|---|---|---:|---:|---:|---|
| L12 | logreg | 0 | `−0.0202pp` | `[−0.319, +0.270]pp` | descriptive only |
| L12 | linear_svc | 0 | `+0.1211pp` | `[−0.133, +0.383]pp` | descriptive only |
| L6 | logreg | 0 | `−0.0202pp` | `[−0.258, +0.210]pp` | descriptive only |
| L6 | linear_svc | 0 | `−0.0605pp` | `[−0.242, +0.109]pp` | descriptive only |

accuracy 的差值都在 `±0.13pp` 内，CI 很窄；四个 family 没有任何
一个 seed 的正 CI。L12 SVC 的点估计 `+0.1211pp` 是唯一正号，但
hate recall 反而从 `0.1263` 降到 `0.1228`，不是质量提升。

### 11.45.4 绝对 accuracy 与 macro-F1：accuracy 被多数类撑高

5-seed 均值：

| depth/probe | arm | accuracy | macro-F1 | Δacc | Δmacro-F1 |
|---|---|---:|---:|---:|---:|
| L12 logreg | f32 | `0.944713` | `0.632523` | — | — |
| L12 logreg | R12 | `0.944512` | `0.620858` | `−0.0202pp` | `−1.1665pp` |
| L12 SVC | f32 | `0.943906` | `0.588320` | — | — |
| L12 SVC | R12 | `0.945117` | `0.588128` | `+0.1211pp` | `−0.0192pp` |
| L6 logreg | f32 | `0.942292` | `0.576474` | — | — |
| L6 logreg | rescan | `0.942090` | `0.564186` | `−0.0202pp` | `−1.2288pp` |
| L6 SVC | f32 | `0.943705` | `0.531079` | — | — |
| L6 SVC | rescan | `0.943099` | `0.521509` | `−0.0605pp` | `−0.9570pp` |

macro-F1 的 5-seed 均值差：

| depth | probe | Δmacro-F1 | mean CI |
|---|---|---:|---:|
| L12 | logreg | `−1.1665pp` | `[−3.448, +1.058]pp` |
| L12 | linear_svc | `−0.0192pp` | `[−2.533, +2.473]pp` |
| L6 | logreg | `−1.2288pp` | `[−3.555, +1.032]pp` |
| L6 | linear_svc | `−0.9570pp` | `[−3.057, +1.075]pp` |

所有 macro-F1 CI 都跨 0；`R12` 在 L12 logreg 上掉 `1.17pp`，`rescan`
在 L6 logreg 上掉 `1.23pp`。**accuracy 的微小正号不能抵消 macro-F1
的负号。**

### 11.45.5 为什么 accuracy 接近天花板：default 阈值几乎不预测 hate

test 真实 hate 率 `0.0575`，但 default 阈值下预测正例率只有：

| depth/probe | arm | pred pos rate | hate recall | non-hate recall |
|---|---|---:|---:|---:|
| L12 logreg | f32 | `0.0208` | `0.2000` | `0.9902` |
| L12 logreg | R12 | `0.0186` | `0.1789` | `0.9912` |
| L12 SVC | f32 | `0.0131` | `0.1263` | `0.9938` |
| L12 SVC | R12 | `0.0115` | `0.1228` | `0.9953` |
| L6 logreg | f32 | `0.0131` | `0.1123` | `0.9929` |
| L6 logreg | rescan | `0.0113` | `0.0947` | `0.9938` |
| L6 SVC | f32 | `0.0044` | `0.0491` | `0.9983` |
| L6 SVC | rescan | `0.0038` | `0.0386` | `0.9983` |

non-hate 占 `94.25%`，所以“几乎全判 non-hate”就能拿到 `94%` accuracy；
但 hate recall 只有 `3.9–20.0%`，macro-F1 只有 `0.52–0.63`。
**这是 default 阈值在 5.7% 先验下的保守性，不是模型真的学会了
hate/offensive 边界。** 准确率差值接近 0，主要是因为两边都很少
判正；它不能作为量化保真的证据。

M1.42 已经证明 fit-only 阈值不能把 prior 搬过来；本轮的 default
阈值同样没有解决 5.7% 先验下的低 recall。M1.45 不重测 target_prior，
因此这里只记录“default 下 accuracy 近零差、macro-F1 负”，不提出
新的阈值方案。

### 11.45.6 判决与边界

- 第三语料按 default 阈值没有给出 candidate gain：accuracy 全部
  0/5 正 CI，macro-F1 四个 family 的点估计三负一微负。
- L12 SVC 的 `+0.1211pp` accuracy 与 `−0.0192pp` macro-F1 是
  “多数类几乎不变、hate recall 更低”的组合，不是质量提升。
- 无官方 split、只有 285 条 hate test、binary 把 offensive 归 0；
  这是 external descriptive transport，不是对 Davidson 总体的
  总体推断，也不是 confirmatory。
- 没有 target_prior / fit-only / 三分类；阈值问题仍由 M1.42 的
  负结果封口。
- 结论：**R12/rescan 在第三个 hate 语料上仍不能升级；default
  阈值的 accuracy 被 94% 多数类撑高，必须同时看 macro-F1 与 hate
  recall。** candidate 线保持关闭。

### 11.45.7 复算

- 预注册：`data/wsp-classify-m145-prereg.json`
- 数据：`data/wsp-classify-m145-prepare-summary.json`（SHA256
  `FCB8BC7C68120AE4AF04A5B9ACD58585513EDE11E1548EBF36A5C2040B6F6281`）
- 脚本：`scripts/wsp_m145/prepare_davidson.py`、
  `run_encode.ps1`、`davidson_probe.py`
- 表：`data/wsp-classify-m145-raw.tsv`（40 行、11 列）、
  `-compare.tsv`（40 行、14 列）、`-primary.tsv`（4 行、7 列）
- 编码：17 个 chunks，L12/L6 各 4 套 `24,781×384` f32 文件；
  编码日志 `encode.log`，`all done`。

## 11.46 fit-balanced prior-free 决策规则（M1.46）

### 11.46.1 问题与预注册

M1.41–M1.45 已经把 test-prior、fit-prior 分位点和三分类都试过：
低先验语料上的默认阈值几乎不预测 hate（Davidson hate recall
`3.9–20.0%`），而 fit-prior 阈值又不能从 test 分数分位点搬过来。
M1.46 换一个真正 inductive 的问题：**只用 fit 标签估
`class_weight="balanced"`，固定默认阈值（logreg 0.5 / LinearSVC 0），
是否能在三个 hate 语料上提高 macro-F1 与 hate recall，并且量化臂
相对 f32 是否仍保持质量？**

这不是阈值搜索：不使用 test 分数分位点、test 先验、test 标签，
也不做 calibration。唯一的改动是 fit 内的类别权重；C 仍在 f32
fit 上用 seed 7 的 5 折 unweighted accuracy 选一次，然后所有
seed/rule/arm 共用。预注册在跑分前写入
`data/wsp-classify-m146-prereg.json`（commit `f6a6264`），随后用
`49a7427` 澄清 C 只选一次；driver 是 `5d94679`。三个语料复用
M1.41/M1.42/M1.45 的同一套 embedding，不新增语料、不重训编码器。

### 11.46.2 口径与 C

- HateXplain：fit `17,270`、test `1,918`，fit/test prior
  `0.308975/0.309698`；
- TweetEval/hate：fit `8,985`、test `2,734`，prior
  `0.420812/0.407827`；
- Davidson：fit `19,825`、test `4,956`，prior
  `0.057705/0.057506`；
- 每个语料 L12 `f32`/`R12`，L6 `f32`/`rescan`；2 probes ×5 个
  bootstrap RNG ×2 rules（unweighted/balanced）；
- Primary 是 `balanced f32 − unweighted f32` 的 macro-F1，12 个
  cell，每 cell 10k paired test-level bootstrap；判定门槛是
  `≥10/12` 且每个语料 `≥3/4` 为 supported。

C 选择结果（两 rule、两臂共用）：

| corpus | L12 logreg | L12 SVC | L6 logreg | L6 SVC |
|---|---:|---:|---:|---:|
| HateXplain | `1.0` | `0.1` | `10.0` | `10.0` |
| TweetEval/hate | `1.0` | `0.1` | `1.0` | `0.1` |
| Davidson | `10.0` | `10.0` | `1.0` | `0.1` |

### 11.46.3 Primary：只有 Davidson L6 SVC 过线，整体 1/12

| corpus | depth/probe | C | 正 CI /5 | mean point | 平均 CI | 判定 |
|---|---|---:|---:|---:|---:|---|
| HateXplain | L12 logreg | `1.0` | 0/5 | `−0.9964pp` | `[−2.755,+0.798]pp` | not_supported |
| HateXplain | L12 SVC | `0.1` | 0/5 | `−0.5775pp` | `[−2.334,+1.214]pp` | not_supported |
| HateXplain | L6 logreg | `10.0` | 0/5 | `−1.5533pp` | `[−3.248,+0.152]pp` | not_supported |
| HateXplain | L6 SVC | `10.0` | 0/5 | `−1.1545pp` | `[−2.871,+0.569]pp` | not_supported |
| TweetEval/hate | L12 logreg | `1.0` | 0/5（全负） | `−3.8939pp` | `[−4.899,−2.914]pp` | not_supported |
| TweetEval/hate | L12 SVC | `0.1` | 0/5（全负） | `−4.0902pp` | `[−5.092,−3.120]pp` | not_supported |
| TweetEval/hate | L6 logreg | `1.0` | 0/5（全负） | `−4.3867pp` | `[−5.477,−3.324]pp` | not_supported |
| TweetEval/hate | L6 SVC | `0.1` | 0/5（全负） | `−4.8729pp` | `[−5.980,−3.790]pp` | not_supported |
| Davidson | L12 logreg | `10.0` | 0/5（4 负 1 跨） | `−3.2216pp` | `[−6.408,−0.027]pp` | not_supported |
| Davidson | L12 SVC | `10.0` | 0/5（全跨） | `+1.1748pp` | `[−2.033,+4.333]pp` | not_supported |
| Davidson | L6 logreg | `1.0` | 0/5（全跨） | `+2.0247pp` | `[−1.092,+5.074]pp` | not_supported |
| Davidson | L6 SVC | `0.1` | 5/5 | `+6.8989pp` | `[+4.054,+9.624]pp` | supported |

总计 **1/12 supported**：HateXplain `0/4`、TweetEval `0/4`、
Davidson `1/4`。整体门槛 `≥10/12` 且每语料 `≥3/4` 都没达到。
方向也不一致：TweetEval 四个 CI 全部整段为负；HateXplain 四个
点估计全负但 CI 全跨 0；Davidson 一个显著负（L12 logreg 4/5
负 CI）、两个点正但全跨、一个 5/5 正。**这不是“低先验语料上
balanced 普遍有效”，而是同一语料内 depth/probe 都能翻转的
异质结果。**

### 11.46.4 Secondary：balanced 是 recall tilt，不是 macro-F1 改进

f32 上 5-seed 均值的 `balanced − unweighted`：

| corpus | depth/probe | Δmacro-F1 | Δaccuracy | Δhate recall | Δnon-hate recall | Δpred pos rate |
|---|---|---:|---:|---:|---:|---:|
| HateXplain | L12 logreg | `−0.9964pp` | `−3.0240pp` | `+15.152pp` | `−11.178pp` | `+12.409pp` |
| HateXplain | L12 SVC | `−0.5775pp` | `−2.6590pp` | `+15.320pp` | `−10.725pp` | `+12.148pp` |
| HateXplain | L6 logreg | `−1.5533pp` | `−3.3368pp` | `+13.636pp` | `−10.952pp` | `+11.783pp` |
| HateXplain | L6 SVC | `−1.1545pp` | `−2.9718pp` | `+14.141pp` | `−10.650pp` | `+11.731pp` |
| TweetEval/hate | L12 logreg | `−3.8939pp` | `−2.8530pp` | `+3.767pp` | `−7.412pp` | `+5.925pp` |
| TweetEval/hate | L12 SVC | `−4.0902pp` | `−3.0358pp` | `+3.408pp` | `−7.474pp` | `+5.816pp` |
| TweetEval/hate | L6 logreg | `−4.3867pp` | `−2.9261pp` | `+4.843pp` | `−8.277pp` | `+6.876pp` |
| TweetEval/hate | L6 SVC | `−4.8729pp` | `−3.3285pp` | `+4.574pp` | `−8.771pp` | `+7.059pp` |
| Davidson | L12 logreg | `−3.2216pp` | `−13.3979pp` | `+54.035pp` | `−17.512pp` | `+19.613pp` |
| Davidson | L12 SVC | `+1.1748pp` | `−13.1759pp` | `+60.351pp` | `−17.662pp` | `+20.117pp` |
| Davidson | L6 logreg | `+2.0247pp` | `−13.3374pp` | `+61.404pp` | `−17.898pp` | `+20.400pp` |
| Davidson | L6 SVC | `+6.8989pp` | `−13.1558pp` | `+68.070pp` | `−18.112pp` | `+20.985pp` |

四个非-macro 指标在 12 个 cell 上方向完全一致：hate recall 全部
上升（5/5 行；同一组预测的 5 次 bootstrap），non-hate recall
与 accuracy 全部下降（0/5 行为正），预测正例率全部上升（5/5
行）。macro-F1 只有 Davidson 的三个 cell 点估计为正；HateXplain
与 TweetEval 的九个 cell 全负。**`balanced` 不是“提高分类质量”的规则，而是把
决策边界向 hate 类移动：少数类 recall 上升， specificity、
accuracy 和多数类 F1 付出代价。**

这种“用 accuracy 换 hate recall”的幅度在 Davidson 最明显：
L6 SVC 的 hate recall 从 `0.049` 提到 `0.730`，macro-F1 从
`0.531` 提到 `0.600`，但 accuracy 从 `0.944` 掉到 `0.812`，
non-hate recall 从 `0.998` 掉到 `0.817`；预测正例率从 `0.004`
升到 `0.214`，而真实 hate 率只有 `0.0575`。**这是 recall
operating point 的移动，不是模型更会分 hate/offensive 边界。**

“prior-free”也不等于“prior-calibrated”：三个语料的 f32 预测
正例率分别是：

| corpus | 真实 test prior | unweighted pos rate | balanced pos rate |
|---|---:|---:|---:|
| HateXplain | `0.3097` | `0.237–0.259` | `0.359–0.376` |
| TweetEval/hate | `0.4078` | `0.734–0.752` | `0.793–0.823` |
| Davidson | `0.0575` | `0.004–0.021` | `0.214–0.217` |

HateXplain 的 unweighted 本来略低于先验，balanced 把它抬到先验的
约 `1.16–1.21×`；TweetEval 的 unweighted 已经约 `1.8×` 先验，
balanced 进一步到约 `1.95–2.02×`；Davidson 从 `0.004–0.021`
直接抬到先验的约 `3.7×`。所以类别权重不是把输出分布校准到
先验，而是把损失函数的少数类权重调大；在默认阈值下，它的
正例率可以远离真实先验。**这解释了为什么 low-prior 的
Davidson 能涨 macro-F1，却同时把 accuracy 掉 13pp。**

### 11.46.5 量化臂没有被 balanced 救回

在 `balanced` 下比较 `quant − f32` 的 5-seed 均值，macro-F1
只有 3/12 个 cell 点估计为正：HateXplain L12 SVC `+0.021pp`、
L6 logreg `+1.287pp`、L6 SVC `+0.299pp`；其余 9 个为负
（HateXplain L12 logreg `−0.079pp`，TweetEval `−0.118~−1.893pp`，
Davidson `−0.164~−0.694pp`）。accuracy 同样只有 3/12 个点估计
为正。`unweighted` 下只有 HateXplain L6 SVC `+0.073pp` 一个
macro-F1 正，其余 11 个为负。

这些 arm 差值都是点估计，没有 bootstrap CI；正号只有
`0.02–1.29pp`，负号最大到 `−1.89pp`。**balanced 没有把量化臂
抬回 f32：同一 C、同一阈值下，R12/rescan 仍然在多数 cell 上
低于 f32；candidate 线保持关闭。** 不能因为 Davidson L6 SVC
的 f32 macro-F1 变高，就把该 cell 的 rescan 也写成候选。

### 11.46.6 post-run 审计、判决与边界

**5 个 “seed” 实际是 5 个 bootstrap RNG，不是 5 次独立拟合。**
driver 的 `make_clf` 把 `random_state` 固定为 `7`；`SEEDS`
只传给 `boot_macro`，没有进入分类器。三语料 12 个 cell 的
5 行 raw 指标逐位相同（例如 Davidson L6 SVC balanced f32 的
accuracy `0.8121468926553672`、macro-F1
`0.6000681283048404` 在 seed 7/13/19/31/43 完全相同）。
因此表中的 `5/5` 是同一组 test 预测的 5 次 bootstrap 重采样，
不是 5 个独立模型/数据切分；`4/5`、`0/5` 也应这样读。这个
口径与 M1.41–M1.45 的 `make_clf(random_state=7)` 相同，是
本轮审计才明确写出的限制，不是看到结果后改预注册。

由此：

- 整体预注册判决是 **not_supported**：1/12 supported，门槛
  `≥10/12` 与每语料 `≥3/4` 都不满足；11/12 个 cell 未过线。
- Davidson L6 SVC 的 `+6.90pp` 是 5 条 bootstrap CI 都过 0 的
  **单个显著比较**，不能写成 5 次独立确认，也不能据此升级
  candidate。若要把它做成 confirmatory，必须另开预注册的
  独立 split/模型种子；不能复用同一批 test 的 5 个 bootstrap。
- balanced 是 inductive 的 cost-sensitive 规则：只用 fit 标签，
  不使用 test 标签/分数；但它不是 calibration，也不是
  prior matching。它稳定提高 hate recall，同时稳定降低
  non-hate recall 与 accuracy；在 HateXplain/TweetEval 上
  macro-F1 甚至显著变差。**把它写成“低先验语料上的通用
  修正”是错的；正确写法是“recall-oriented operating point，
  是否值得用取决于部署对 FN/FP 的代价”。**
- 没有 target_prior、fit-prior 分位点、calibration、阈值搜索；
  没有 MLP、微调、新语料。C 只由 f32 fit 选一次，未用 test
  标签。Davidson 仍是无官方 split 的 descriptive transport，
  HateXplain/TweetEval 的标签映射也仍是外部边界。
- **判决：M1.46 的 prior-free 规则不支持“提高 macro-F1”的
  假设；candidate 线保持关闭。分类决策规则这条线到此收口，
  不再在同一批 test 上换 class weight/阈值/先验写法。**
- 下一步若要继续，只能是新的独立任务/数据契约，或转回
  检索/核心 `direct`；不要用 M1.46 的 Davidson L6 SVC 单格
  反过来开 candidate confirmatory。

### 11.46.7 复算

- 预注册：`data/wsp-classify-m146-prereg.json`（`f6a6264`，
  C 选择澄清 `49a7427`）
- driver：`scripts/wsp_m146/balanced_probe.py`（`5d94679`）
- 合并/汇总：`scripts/wsp_m146/merge_results.py`
- 表：`data/wsp-classify-m146-raw.tsv`（240 行、15 列）、
  `-compare.tsv`（300 行、18 列）、`-primary.tsv`（12 行、9 列）、
  `-summary.tsv`（48 行、13 列）、`-rule-delta.tsv`（60 行、9 列）、
  `-arm-delta.tsv`（48 行、9 列）、`-verdict.json`；
- 数据根：M1.42/M1.45 的 `hatexplain`、`tweet-hate`、`davidson` 三个
  结果目录；每套 `texts-l12.json` + `emb12/l12-pure-f32-*`/`l12-r12-*` +
  `emb6/l6-pure-f32-*`/`l6-rescan-*`；
- 运行日志：`m146-both.log`（各 corpus root）；
  C 选择与 5 个 bootstrap RNG 的输出均在日志里。

## 12. 下一步


> **M1.46 读法**：`class_weight="balanced"` 作为 prior-free
> 决策规则在三个 hate 语料上整体 `not_supported`：12 个 primary
> cell 只有 Davidson L6 SVC 过预注册 5/5（macro-F1 `+6.90pp`）；
> 总计 1/12，HateXplain 0/4、TweetEval 0/4、Davidson 1/4。
> TweetEval 四个 CI 全负（`−3.89~−4.87pp`），HateXplain 四个点
> 估计全负但 CI 跨 0，Davidson L12 logreg 4/5 负 CI。balanced
> 的稳定效应是 hate recall `+3.4~+68.1pp`，代价是 non-hate
> recall `−7.4~−18.1pp`、accuracy `−2.7~−13.4pp`；Davidson
> 预测正例率被抬到 `0.214–0.217`，真实先验只有 `0.0575`。
> **它是 recall tilt，不是 macro-F1 改进，也不是校准；量化臂
> 没有被救回。** 5 个 “seed” 实际是同一组预测的 5 次 bootstrap，
> 唯一 5/5 的 Davidson L6 SVC 不是 5 次独立确认。**prior-free
> 分类决策规则到此收口；candidate 线保持关闭，不再在同一批
> test 上换 class weight/阈值/先验写法。** 见 §11.46。
>
> **M1.45 读法**：Davidson 第三语料 default 阈值下，四个 family 的
> accuracy `arm−f32` 全部 0/5 正 CI（`−0.06~+0.12pp`）；accuracy
> `0.942–0.945` 但 hate recall `3.9–20.0%`、macro-F1 `0.52–0.63`。
> **accuracy 被 94% 多数类撑高；第三语料仍不能升级 candidate。**
> 见 §11.45。
>
> **M1.43 读法**：HateXplain 三分类下四个 family 的 `arm−f32` 全部
> 0/5 正 CI；accuracy 只有 `0.649–0.656`，`offensive` recall
> `0.354–0.426`。binary 的 `0.82` 主要来自 normal-vs-rest，不是
> hate/offensive 细边界。**三分类只作 exploratory，不能升级
> candidate。** 见 §11.43。
>
> **M1.44 读法**：静态字节–质量 Pareto 不是“越贵越好”。L12 的
> STS Spearman 最优点是 `R12`（10.78MB），CoLA 是 `int4`（13.44MB），
> L6 的 STS 是 `rescan`（5.39MB）、Hate 是 `int3`（5.39MB）、SciFact
> 是 `int4`（6.72MB）。**同一个预算换任务就换 winner；不能把一个
> 任务的 frontier 当部署结论。** 见 §11.44。
>
> **M1.42 读法**：fit-only 阈值没有保留 M1.38 的 candidate gain。
> TweetEval/hate 上 L12 `R12` 从 test-prior 的 `+1.83/+1.68pp` 变成
> `−1.77/−1.59pp`，L6 `rescan` 从 `+2.87/+2.55pp` 掉到
> `+0.02/+0.58pp`；四个 family 都是 `not_retained`。fit-only 的 f32
> accuracy 比 test-prior 低 `7.3–11.0pp`，预测正例率 `0.76–0.80`
> 而真实先验 `0.42`。**M1.38 的 rescue 是 test 分数分位点现象，
> 不是可部署协议。** 见 §11.42。
>
> **M1.41 读法**：HateXplain 的全局先验阈值在 Gab/Twitter 先验
> `0.502/0.078` 上错配；域感知阈值把 global 的伤害收回一部分
> （L12 5/5、L6 logreg 4/5 supported；L6 SVC 0/5），但仍低于 default
> （`0.815/0.816` vs `0.823`，L6 `0.818/0.814` vs `0.822`）。
> 匹配边际先验不等于匹配最优阈值；fit-only 在 L12 略好、L6 更差。
> **candidate 没有被域规则救回，target_prior 仍是 transductive
> exploratory。** 见 §11.41。
>
> **M1.40 读法**：第二个 hate 语料 HateXplain 的预注册 confirmatory 没有
> 复现 M1.38/M1.39。Primary accuracy（full test、target_prior、matched-C）：
> L12 `R12` logreg 0/5（`+0.10pp`）、SVC 0/5（`−0.29pp`）；L6 `rescan`
> logreg 0/5（`−0.10pp`）、SVC 0/5（`+0.35pp`）。两个候选都被预注册判据
> 判为 **rejected**；macro-F1 和 default 0.5 也全部 0/5，所有 CI 都跨 0。
> 更关键的是 target_prior 在该语料上把 f32 的绝对 accuracy 从 `0.823`
> 降到 `0.808`（L12）/`0.811`（L6），量化臂也降 `0.80–1.09pp`；匹配先验
> 把预测正例率抬到 `0.3092`，但 accuracy 反而下降。**`R12` 和 `rescan`
> 都不能升级；target_prior 不能当默认协议。HateXplain test 有 54% Gab，
> 标签映射把 offensive 归为 0，这是外部边界；但“没有复现”是明确结论。
> 下一步若继续，必须换第三个独立语料或预注册域分层，不能继续在同一
> HateXplain test 上换 seed/C/半分割。** 见 §11.40。

> **M1.39 读法**：M1.38 的 6/36 条件性正结果已过预注册半分割。
> Primary accuracy：L6 `rescan` logreg 10/10 正（`+2.82pp`）confirmed；
> L6 `rescan` SVC 7/10（`+2.45pp`）inconclusive，但 macro-F1 10/10；
> L12 `R12` logreg 5/10（`+1.90pp`）、SVC 4/10（`+1.62pp`）均
> inconclusive。Default 0.5 下 `R12` 仍是 0 正 / 5 负 / 5 跨，
> `rescan` 只是弱正（logreg 4 正，SVC 0 正）。**结论：`R12` 不升级，
> 继续当 Hate-only、target_prior 条件下的候选；`rescan` 是唯一在
> L6 primary 至少一个 probe 过线的候选，但 SVC accuracy 未过线。
> target_prior 仍是 transductive exploratory，不是默认协议。半分割
> 只分开了校准分数与评估标签，没有换语料；下一步必须做第二个 hate
> 数据集的预注册 confirmatory，不能继续在同一批 test 上换 seed/C/
> 阈值。** 见 §11.39。

> **M1.38 读法**：分类头的默认 0.5 官方 test headline 不变——36 个比较
> 0 个量化档显著超过真 f32（0 正 / 41 负 / 139 跨），f32 仍是默认锚点。
> Hate 的 20–22pp 差距已经拆开：文本域迁移是主因（域分类器 AUC `0.921`），
> 长度/截断不是（长度单特征 `0.561`，`>64` 与 `≤64` 准确率只差 `1.12pp`）；
> 默认阈值漂移贡献约 `6–9pp`，校准后仍有 `12–13pp` 残差。把阈值改成
> “目标分数分位匹配 train 先验”（不用 test 标签）后，36 个比较里 6 个出现
> 正 CI，全部在 Hate：L12 `R12`（logreg 5/5、SVC 4/5）和 L6 `rescan`
> （两 probe 5/5）是条件性候选，`int4` SVC 3/5、`int3` logreg 1/5 只是弱信号；
> SST-2/CoLA 仍 0 正。**不要把这 6 个正比较写成“量化普遍增益”，也不要
> 再把 `R12` 写成“跨域不成立”；正确口径是“默认阈值无正向，目标先验阈值下
> 有 Hate-only 条件性候选”。** 下一步应做预注册的独立数据集/切分和固定
> 阈值规则，停止在同一批 Hate 上换 seed/C/阈值。见 §11.38。

> **M1.28 读法**：本节及历史 §11.12–§11.25 中所有 SST-2 的 `f32` 读数
> （L12 与 L6）都应视为 ternary 基线；`int4−f32` 正向是 ternary→4-bit
> 恢复，不是 f32→int4 证据。M1.26 修正 L12，M1.28 修正 L6；CoLA/hate
> 的 L12/L6 边界见 §11.26/§11.28。
>
> **M1.29 读法**：SST-2 的 `int3`/`R12`/`rescan-k2` 也已在真 f32 上重跑，
> 没有任何量化档超过真 f32；L12 `int3`/`R12` 的 CI 整段为负。历史
> §11.12 里除 `int4` 之外的量化“增益”同样是 ternary 基线伪影。见 §11.29。
>
> **M1.30 读法**：CoLA/hate 的 L6 `l6-pure-f32` 已全量逐行确认真 f32
> （对真 f32 余弦 `1.00000000`，max abs diff `1.8–1.9e-7`；对真 ternary
> 只有 `0.46` / `0.38`）。这两套 L6 分类结论无需重算；SST-2 是唯一
> L12 与 L6 的 `pure-f32` 都错标成 ternary 的数据集。见 §11.30。
>
> **M1.31 读法**：CoLA/hate 的 L12/L6 全臂已在同一 per-arm 5 折选 C
> 口径下重跑。没有任何量化臂的主指标 CI 整段大于 0；CoLA L12 的
> `int4`/bits4 只有 `+1.24pp/+0.93pp` 点估计且 CI 跨 0，hate 的
> `R12`/`int3`/`int4` 多处 CI 整段为负。SST-2 的 `int4` 正向没有跨到
> 这两个任务；`R12`/`rescan` 也没有分类增益。见 §11.31。
>
> **M1.32 读法**：CoLA/hate 的 L6 官方 `l6-int4` 已全量 9,594 / 11,719
> 行确认真 bits4（余弦 `1.00000000`，max abs diff `1.6e-7 / 1.5e-7`，
> 全部行 ≤`1e-6`）。把 `true-bits4` 补进 L6 分类矩阵后，CoLA `−0.94pp`
> （logreg macro）、hate `−1.55pp`（logreg accuracy）的方向不变，
> hate 的 CI 仍整段为负。M1.30 的 512 行抽检已升级为全量，M1.31 的
> L6 `int4` 数字不需要改。见 §11.32。
>
> **M1.33 读法**：分类头已经收口。按“每个量化档只计一次”合成 36 个
> `dataset×depth×probe×arm` 比较，0 个 CI 整段大于 0，9 个整段小于 0，
> 27 个跨 0。SST-2 的 `int4` 正向不存在；CoLA L12 的 `+1.20pp` 只是
> 点估计且 CI 跨 0；hate 的 `R12`/`int4` 多处显著负向。分类头继续用
> f32 锚点，不把任何量化档当默认。见 §11.33。
>
> **M1.34 读法**：M1.33 的 `0/9/27` 不是稳健计数，只是 seed 7 的
> C 选择结果。5 个 CV seed 下 29/36 点估计同号、7 个翻号；9 个负向
> 只有 6 个在 5/5 seed 仍为负。Hate L6 `logreg int3` 在 4/5 seed
> 变成显著正向（`+1.71pp`），但 matched-C 的 C=1/10 下只有
> `+0.44pp/+0.15pp` 且 CI 跨 0，是 f32 选 C=10、int3 选 C=1 的错配。
> 分类头仍用 f32 锚点，但引用 M1.33 必须带“seed 7 条件”；量化 vs
> f32 的显著性不能再用 per-arm C 单独判定。见 §11.34。
>
> **M1.35 读法**：把 matched-C 扩到全部 36 个比较 ×5 seed。每个
> case/probe 只在真 f32 上选 C，f32 与全部量化臂共用这个 C，重算
> 10k paired bootstrap：180 个 CI 里 0 个显著正向、41 个显著负向、
> 139 个跨 0；36 个比较没有任何一个在 5 个 seed 出现过正 CI。M1.34
> 的 Hate L6 `logreg int3` 4/5 显著正向在共同 C 下变成
> `+0.44pp/+0.15pp` 且 5/5 跨 0，确认是 per-arm C 错配。最接近正向的
> Hate L6 `logreg rescan` 在 C=10 为 `+1.32pp`，CI
> `[−0.0366,+2.67]pp`，仍跨 0；三个共同 C 都不显著。**matched-C 下
> 仍没有量化档显著超过真 f32；41 个负向说明量化不是一律无损，但
> 没有正向。** 见 §11.35。
>
> **M1.36 读法**：用 nested CV 复验 Hate L6 边界格。OOF train 上
> 30 个 CI 里 2 个显著正向、0 个负向、28 个跨 0；`logreg rescan`
> 点估计 10/10 为正，但只有 1/10 CI 整段为正（seed 7 为
> `+0.857pp`，CI `[+0.156,+1.558]pp`），SVC `0/5`。`int3` 也是
> 10/10 点正、1/10 显著；`int4` 9/10 点正、0/5 显著。OOF f32 为
> `0.761–0.767`，官方 test 只有 `0.533–0.556`，约 `21pp` 分布差，
> 不能把 OOF 弱正向搬到 test。**`rescan` 不是纯噪声，但也不是可
> 发布增益；f32 继续是默认锚点。** 见 §11.36。
>
> **M1.37 读法**：把 nested CV 扩到全部 6 个 `dataset×depth`、两个
> probe、5 个 outer seed，共 180 个 OOF CI：5 个显著正向、52 个
> 显著负向、123 个跨 0；36 个比较里 4 个出现过正 CI、15 个出现过
> 负 CI、17 个全跨。正向全部在 Hate：L12 `R12`（2/5 logreg +1/5
> SVC）与 L6 `rescan`/`int3`（各 1/5 logreg）。但 Hate L12 `R12`
> 在官方 test 上是 5/5 显著负向（OOF `+0.68pp` vs test `−1.35pp`），
> 属于方向反转；Hate L6 `rescan`/`int3` 在 test 上点正但 5/5 CI
> 跨 0。Hate 的 OOF/test f32 差约 `20–22pp`，test 内部 5 折仍有
> `0.751–0.764`，说明是跨域迁移，不是标签噪声。**OOF 全矩阵没有
> 改变主判决：官方 test 上 0 个量化档显著超过 f32；`R12` 的 OOF
> 正向是跨域不成立的候选，`rescan` 仍只是弱候选。** 见 §11.37。

- **M1.26 基线审计已完成（§11.26）**：SST-2 L12 的 `l12-pure-f32` 是
  ternary，真实 f32→int4 为 `−0.001503`（CI `[−0.012081,+0.008786]`），
  原 `+0.042207` 是 2-bit→4-bit 恢复。CoLA/hate 的 L12 `f32` 是真 f32，
  旧结论保留；SST-2 L6 在 M1.28 也已确认是 ternary（§11.28），旧 L6
  `int4−f32` 不能再用。不要再把 SST-2 的 `+0.04` 写成“int4 超过 f32”，
  也不要再沿 M1.17–M1.25 的 SST-2 几何解释一个不存在的正向。
- **M1.27 残差矩阵复核已完成（§11.27）**：真 f32 下 `hybrid_rest_k16`
  为 `−0.66pp`，没有合法残差臂超过 f32；`noclassmean` 的 `+2.31pp` 和
  `perm_class` 的 `+0.77pp` 都是 test 真标签泄漏/依赖。M1.23 的
  “保留 f32 前 16 维”不能再作为 int4 残差机制引用；残差单独可解码只说明
  信息存在，不说明可加回 f32。
- **逐块 `3→2`/`3→4` 与第二模型 L6 都已完成（§8/§9/§11）**：不要再重复跑
  L12 的同一批 72 块或把 L12 布局按层号搬到 L6。选择规则已在 §11.9 做了
  双向检验（L12→L6 正、L6→L12 对 int4 负）；第三模型与同深度对照也已在
  §11.10 做完（L3/par-L6 双负），下一步不是再找模型，而是任务头或接受
  每模型重扫。
- **检索任务头已做（§11.11），结论是负向**：`R12` 和 L6 `rescan-k2` 的 STS
  增益没有转到 SciFact 检索；`R12` 对 int3 的 MRR/nDCG@10 点估计为负、CI 跨 0，
  对 int4/f32 的 MRR/nDCG CI 基本整段为负；L6 `rescan-k2` 对 int3 的 MRR/
  nDCG@10 CI 整段为负。任务头不是新的调参维度，而是现有 STS 候选的否决门。
- **分类任务头已做（§11.12–§11.21），结论是“SST-2 内稳定、跨任务不外推”**：
  SST-2（6,920/1,821，冻结编码器 + 线性 probe，train 内 5 折选 C）上，L12
  `R12` 对 int3 点估计低 `−0.008842`、CI 跨 0，对 int4 整段为负；L6
  `rescan-k2` 对 int3/int4 的 CI 都跨 0。分类头没有接住 STS 的块级增益。
  **注意：以下 `+4.00pp` / `+2.2pp` 等读数都基于 ternary `f32` 锚点，
  已由 M1.26/M1.28/M1.29 作废；真 f32 下 SST-2 L12/L6 没有任何量化档
  超过 f32。** L12 均匀 int4 在 SST-2 上比 f32 高 `+0.040088`，换 `LinearSVC` 后仍为
  `+0.043783`；M1.16 固定 test、25 次 80% train 子样本重训后，logreg
  `int4−f32` 均值 `+0.042987`（25/25 正），SVC `+0.044437`（25/25 正），
  说明它是 L12 在 SST-2 上的稳定表示效应，不是单次划分偶然。第二分类集
  GLUE CoLA（balanced + macro-F1 选 C）只得到 `+0.013341` / `+0.008467`，
  两个 CI 都跨 0；第三分类集 TweetEval/hate 的无权重口径为
  `−0.003679` / `−0.004826`，balanced 口径为 `−0.012694` / `−0.013028`，
  后者 CI 整段为负；L6 的 int4 只有 `+0.0063/+0.0036` 且分别 22/25、
  19/25 为正。M1.17 的几何诊断把这层“SST-2 特异”推进到可描述的结构：
  int4 在 SST-2 上把类均值差方向挤进前 4 个主成分（train `0.405→0.855`，
  test `0.422→0.821`），`eff_rank 77.6→200.1`，同时 `centroid_cos`
  从 `0.994` 降到 `0.943`；CoLA/hate 的 `signal_top4` 基本不动，
  `eff_rank` 还略降。PCA 干预里 SST-2 L12 `keep4` 的 `int4−f32` 是
  `+0.166392`、CI `[+0.138935,+0.194399]`，白化后仍有 `+0.049423`、
  CI 全正；CoLA balanced 的所有 mode CI 都跨 0，hate 除 L6 raw
  （`−0.014631`、CI 全负）外也都跨 0。M1.18 再把 int4 的主成分基搬到 f32：
  能救回 f32 的部分掉点（`keep4 0.603→0.694`），但同维数下 int4 仍
  `0.769`，`int4−f32` 从 raw `+0.040` 扩到 `+0.075`；反方向把 int4 投到
  f32 基也仍保留优势。M1.19 直接比较两档的类均值差方向：SST-2 L12 的
  cos 只有 `0.5715`（test `0.5891`），夹角 `55.1°`，`||d_int4||/||d_f32||
  = 1.94`，f32 的方向只有 `27.8%` 在 int4 前 4 个主成分里、int4 的方向
  只有 `7.2%` 在 f32 前 4 个主成分里；CoLA/hate 则几乎同向（cos
  `0.970–0.976`，夹角 `12.6–14.1°`）。M1.20 再做旋转对照：把 f32 的类
  方向转到 int4 后，f32 精度仍是 `0.800110`、`int4−f32` 仍是 `+0.040088`，
  说明方向角本身可被线性 probe 吸收；`sep` 在 SST-2 L12 上和精度同向
  （`sep 1.288×`、Fisher `1.659×`）。但 M1.21 的剂量-反应把 `sep` 的
  普遍性排掉：SST-2 L12 在 1/8 剂量就拿到 `+0.025261`（约最终增益的
  63%），1/4 拿到 `+0.038990`（约 97%），而 `sep` 到 1/8 只完成总增幅的
  约 24%；SST-2 L6 的精度峰值在 1/8（`+0.019769`），`sep` 到 1/2 才见顶；
  CoLA L12 的 `sep` 降到 `0.936×`，accuracy 反升 `+0.011505`。
  **SST-2 的正向在数据集内稳定，但没有跨任务/跨数据集泛化；M1.17 的
  “类别方向低秩集中”是伴随读数，M1.18 证明低秩基本身不足以复现增益，
  M1.19 的类方向旋转是症状，M1.20 的 `sep` 只是 SST-2 L12 上的伴随
  读数，M1.21 显示它是低剂量、早饱和的表示效应，不是 `sep` 的普遍
  中介。** M1.22 的 train×test 交叉矩阵再把它定位成**匹配表示效应**：
  SST-2 L12 的 `(0,1)` 是 `−2.86pp`、`(1,0)` 是 `−23.23pp`，只有
  `(1,1)` 是 `+4.00pp`；1/8 train-only 只有 `−0.50pp`（CI 跨 0），
  说明不是 probe 正则化，也不是更好的 test 表示。同一矩阵里 CoLA L12 的
  `(1,0)` 是 `+2.88pp`（CI 全正），`(0,1)` 是 `−5.27pp`，对角只剩
  `+1.15pp`（CI 跨 0）——CoLA 的小正向才是 train-only 正则化；hate L12
  只有 `(0,1)` 的 `+1.87pp`，train 侧为负、对角仍负。**三个数据集的正向
  落在不同格子，只有 SST-2 L12 是 train/test 同时匹配才成立；这也解释
  了为什么只有它在均匀 int4 的 in-domain 设置下有稳定正向。** 这仍不是
  因果证明，也不能写成量化普遍提升线性分类。M1.23 再把“匹配表示”拆到
  残差：SST-2 L12 的 `r=int4−f32` 单独就能到 `0.832510`，比 f32 高
  `+3.23pp`；`f32 + int4 rest(k=16)` 与 int4 打平，而 `int4 top16 +
  f32 rest` 退回 f32；残差类均值是 f32 的 `1.59×`、与 f32 类均值近正交，
  但在 f32 probe 上的投影是 f32 margin 的 `1.21×`。CoLA/hate 的残差类
  均值只有 `0.22–0.25×` 且投影为负，SST-2 L6 同向但只有 `0.33×`。
  **“为什么只有 SST-2 L12”现在有一个几何答案：类别条件量化偏置在
  f32 probe 方向上有足够正投影，而不是各向同性噪声或 probe 正则化。**
  这仍是观测性拆解，不是量化算子的因果证明；`resid_full` 需要同时有
  f32/int4 两套 embedding，是诊断特征，不是部署方案。分类头这一路的
  验证已够，不要再继续调 `R12`；接受“BUDGET 只在 STS 相关性上有效”，
  把 `R12` 留在 STS in-domain，不进运行时默认。若以后还要做，应该换更强
  probe（但当前用户口径排除 MLP）或新的任务族，而不是重复第三数据集。
- **M1.24 直接量化负对照（§11.24）**：对 f32 embedding 本身做
  group=1/32/384 的均匀 int4，`q` 没有增益、直接残差单独 probe 在
  chance/多数类基线，残差 margin 比只有 `−0.008` 到 `+0.005`。
  这说明 M1.23 的类别条件偏置不是输出端 int4 舍入的普适性质，而依赖
  权重量化误差在编码器里的传播路径。该实验仍是 representation-level
  代理，不是权重量化器的因果消融；weight per-group sweep 在本机 CPU
  上跑不动，未写成结论。
- **M1.25 残差线性算子（§11.25）**：在 train 上拟合 `r ≈ (x−μ)W+r_μ`，
  test `R²` 0.42–0.52（shuffled −0.04 到 −0.09），说明残差有 42–52% 的
  方差可由 f32 embedding 线性预测。类均值响应数据集特异：SST-2 放大
  1.52×/1.15×、旋转 86°/94°；CoLA/hate 压缩到 0.21–0.23×、旋转
  99°–108°。SST-2 算子用到 CoLA/hate 的 `R²` 为 −43.4/−36.5（L12），
  反过来只有 0.016–0.031，说明没有一个跨数据集共同算子。SST-2 残差
  再拆成线性/非线性两半：L12 `r_hat` 0.7935、`r_perp` 0.6310；L6
  `r_hat` 0.8040、`r_perp` 0.5837。该实验把“为什么只有 SST-2”推进到
  “SST-2 的类均值方向落在会被放大的方向”，但仍是线性代理，不是权重
  误差的因果消融；α 固定、无 bootstrap、只跑 logreg。
- **重扫选择器已落库，且 5 折交叉拟合已把“选择”留出（§11.6/§11.7）**：
  `wsp_budget_rescan.py` 能从两张表生成严格同字节候选；L6 全量 `6/24/6` 是
  `0.831304 / 0.824457`（高于 int4 点估计），但这是 in-sample。5 折 × 2 种子
  的 OOF 均值 `0.825731 / 0.819179`：相对 int3 `+0.011137 / +0.008754`
  （10 折中 P 9/10、S 8/10 正），相对 int4 `−0.000330 / −0.000175`
  （10 折中 5/10 正）。**选择器稳定优于 int3，但没有稳定超过 int4，仍不能当
  默认规则。** 因为 `per_shape=2` 是固定结构，outer 5 折已经够；若还要在
  inner fold 选 `per_shape`，才需要真正的 nested CV。
- **M1.5 外部三套已完成**：`R12`/`R10`/`F5F7V7` 在 SICK-R 负迁移，
  `K10K11` 在 STS13 轻微负，只有 `v6e`/`I10` 三套外部点估计全正；`I10` 是
  外部保守默认，`K10K11`/`R12` 是 STS in-domain 选择。不能再写
  “K10K11 在所有外部数据上最稳”或“R12 是默认优先点”。
- **`R12` 仍是 L12 上的 in-sample 搜索**：供体和升级槽都是在这 1,379 对上选的，
  然后再在同一批 pairs 上评测；L12 本身没有 held-out。数据里 2,758 个句子 ID
  全部唯一、不跨 pair 复用，按 pair 切分天然 sentence-disjoint，但 L12 还没做
  切分复验。L6 的两折已经做了（§11.4），但只能说明 L6 内部有块级信号，不能
  补上 L12 的 held-out。
- **按来源拆解已经暴露不均匀（§9.4/§9.5）**：`R12` 对 `F5F7V7` 是
  4 胜 1 平 1 负，但对纯 int4 是 0/6 胜；留一来源时对 int4 的符号还会翻转。
  总体 Spearman 反超更像跨来源排序/校准效应。
- **不要把任何 3-bit 配置说成“追平/超过 int4”**：STS 全量 R12 的 Spearman
  点估计 `+0.003364`，但 50k CI `[−0.007538, +0.014370]` 跨 0；Pearson 仍差
  `−0.003834`。三套外部 int4 又全面更高。3-bit 只证明“同字节位置重排能把
  STS benchmark 的 Spearman 推过 int4 的点估计”，不能当普遍结论。
- **`R12` 对 `R10` 也没有统计分离**：STS 两个 50k CI 都跨 0；不要因为
  `0.833671 / 0.835411` 更高就写成“已证优于 R10”。
- **128 和代理都不能做位置判据**：`R12` 的 128 是 `0.899405 / 0.929708`，
  仍低于 K10K11；`F5F7V7`/`R12` 与 K10K11 的 `proxy_err_ratio` 同为
  `0.91552`。128 只 smoke。
- **第二模型 L6 已做，但没有验证 L12 候选可迁移（§11）**：L6 纯 int3 比纯 int4 低
  `−0.011467 / −0.008929`，int4 只在 Pearson 10k CI 过 0；I10/K10K11/R12 的
  绝对层号投影都低于 L6 纯 int3，输出端对齐的 R12 尾段也只统计中性。L6 重扫
  能到 `0.828687 / 0.821084`，但对 int4 的 CI 都跨 0，且是 in-sample；5 折
  交叉拟合的选择器 procedure 也只打平 int4（§11.7）。**不能把 I10 写成跨模型
  默认。**
- **相对深度规则已做双向检验（§11.9），不能当通用规则**：L12→L6 的候选
  `0.828707 / 0.824484` 对纯 int3 双 10k CI 过 0，对纯 int4 点估计
  `+0.002646 / +0.005131` 但两个 CI 都跨 0；反过来 L6→L12 的候选
  `0.827813 / 0.823365` 对 int3 双 CI 过 0，对 int4 是
  `−0.009692 / −0.008682`，Pearson CI `[−0.018357, −0.000968]` 整段为负。
  **同一条规则两个方向不对称；它只能当候选生成器，不能替代每模型重扫。**
- **第三模型与同深度交叉检验已完成（§11.10），L12→L6 的正向是模型对特定**：
  同一套 L12 块布局在 L3 上相对纯 int3 是 `−0.012450 / −0.010778`
  （CI 整段为负），在 par-L6 上是 `−0.016161 / −0.007308`；L6 自己的
  `6/24/6` 最优布局投到 par-L6 也是 `−0.011203 / −0.008646`。同一布局在
  两个 6 层模型上相对各自 int3 的 Δ 从 `+0.014/+0.014` 翻成 `−0.016/−0.007`。
  **不能再把 L12→L6 的单向正向写成跨模型规则。**
- **L3 的原生重扫只打平 int3，不构成“3-bit 追平 int4”证据**：18 块 × 2 方向
  全量单块表已完成，k1 `3/12/3` 对 int3 是 `+0.000045 / +0.002787`（CI 都跨 0），
  k2 `4/10/4` 是 `−0.004695 / +0.000452`（CI 都跨 0），对 int4 都没有统计优势。
  3 层模型的同字节块重排没有可用 headroom（§11.10.3）。
- **下一步优先级**：第三模型与同深度对照都已做完（§11.10），**停止调跨模型
  规则和 `per_shape`**。要么接受“每个模型自己重扫 36/72 块表”，要么转检索/
  分类任务头；不要再拿 L12→L6 的单向正向当通用结论。只有在这两类证据上
  仍为正的块组合，才进入 int5/f32/侧车或 MXQ。
- 4-bit 的权重修正（BIAS/SCALE）已经证伪，不再回到那条线。
