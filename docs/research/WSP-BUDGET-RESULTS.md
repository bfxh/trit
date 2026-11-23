# WSP-BUDGET：同字节预算分配与块交换（L12 / STS 1,379 对）

> 状态：M1.2 完成。严格同字节 `10,782,720 B`（含 per-group scale 与 per-row
> bias）上，`K10K11` 把 L12 全量抬到 Pearson `0.823479`、Spearman `0.820549`；
> 相对纯 int3 `+0.005039 / +0.007187`，10k 配对 bootstrap 95% CI 分别为
> `[+0.001919, +0.008219]` / `[+0.003737, +0.010819]`，两个下界都过 0。
> `K10K11` 相对 `I10` 是 `+0.001858 / +0.001651`；50k 重采样 CI 为
> `[+0.000591, +0.003120]` / `[+0.000048, +0.003272]`，Spearman 下界只勉强过 0。
> `K10K11` 相对 `v6e` 的 Pearson CI 过 0、Spearman CI 跨 0。它仍不是
> “3-bit 追平 int4”：距纯 int4 Pearson `−0.014026`、Spearman `−0.011498`。

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
> 消融测。`K10K11` 是当前严格同字节的优先点；它对 I10 的 Spearman 优势很薄，
> 对 v6e 的 Spearman 仍不显著，更不是“3-bit 追平 int4”。**

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
| v6（非严格） | 3 | 59 | 10 | 0 | 10,801,152 | +18,432 |
| 纯 int4 | 0 | 0 | 72 | 0 | 13,436,928 | +2,654,208 |

`v6e` 与规划器 v1 的区别只有一处：v1 把层 11 的 `attention.output.dense` 降 int2，
v6e 把这一个降级挪到层 9 的同一类矩阵。两者形状、字节、代理误差比完全相同
（`proxy_err_ratio=0.91552`），但 STS 不同——这正是代理看不见位置。
`I10` 与 `v6e` 的区别也只有一处：把层 11 的 `intermediate.dense` 平移到层 10，
形状、字节、代理误差比仍然相同。
`K10K11` 再把 Q3、K4 两个 int4 升级平移到 K10、K11；全部 9 张 int4 仍是同形状，
总字节、`proxy_err_ratio=0.91552` 仍然不变，只有位置变了。

运行时报告现在也按预算图复算实际字节（`crates/trit-wspace/examples/bert6/run/weight_format/`）：
`TRIT_BUDGET` 存在时 `weight_bytes` 改为 `actual_bytes_with_metadata`，
并额外给出 `budget_actual_bytes / budget_weight_bytes / budget_metadata_bytes /
mixed_bits / budget_group`。L12/K10K11 的 128 smoke 报告为：
`budget_actual_bytes=10,782,720`、`budget_weight_bytes=7,962,624`、
`budget_metadata_bytes=2,820,096`、`mixed_bits={2:3,3:60,4:9}`、
`ratio_vs_f32=0.126953125`。

## 3. 读数

128 对（用于 smoke，不作最终判据）：

| 配置 | Pearson | Spearman | 实际字节 |
|---|---:|---:|---:|
| 纯 int3 | 0.897582 | 0.927222 | 10,782,720 |
| 规划器 v1 | 0.885716 | 0.922785 | 10,782,720 |
| **v6e** | **0.901878** | **0.931000** | 10,782,720 |
| **I10** | **0.903645** | **0.935691** | 10,782,720 |
| **K10K11** | **0.904783** | **0.937186** | 10,782,720 |
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

`K10K11` 距纯 int4：Pearson `−0.014026`、Spearman `−0.011498`；距 f32：
Pearson `−0.014116`、Spearman `−0.010345`。**预算分配没有关闭 int3 到 int4 的差距。**

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

判据（见 `WSP-ALT.md` §5.4）是“Pearson/Spearman 不低于纯 int3，且至少一个指标
10k CI 下界 > 0”。`v6e` 只有 Spearman 过；`I10` 两个指标都过；`K10K11` 对纯 int3
两个指标都过，对 I10 的 50k CI 两个下界也过 0。但 `K10K11` 对 v6e 的 Spearman
CI 跨 0；`K10K11` 对 I10 的 Spearman 下界只有 `+0.000048`，只能算“薄优势”。
所以当前写法是：

> `K10K11` 是严格同字节的优先点，对纯 int3 双指标显著；对 I10 是薄的双指标优势，
> 对 v6e 只有 Pearson 显著。不能写“已证全面优于 v6e”，也不能写“追平 int4”。

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
7. **扫描器不是完整敏感度表**：目前降级块扫了 O/I/F 的 33 个同形状交换，
   升级块扫了 Q3/K4 各 15 个目标，另在 K10K11 上试了 4 个 K9 目标；
   全量只验证 I10、F3、I10+F3、K4→K11、Q3→K11、K10K11 等少数点。
   没有覆盖全部 72 块，也没有建模多块同时交换的交互。
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
  升级目标，全量只验证 I10/F3/I10+F3/K4→K11/Q3→K11/K10K11 等少数点。
  没有搜索全部 72 块的组合，也没有 int5/f32/侧车候选。
- **I10 与 v6e 的差距不显著**：两者只差一个同形状 i 块的位置，10k bootstrap
  两个指标都跨 0；不要把点估计 `+0.000934 / +0.000761` 写成“已证改进”。
- **K10K11 与 I10 的 Spearman 优势很薄**：50k CI 下界 `+0.000048`；K10K11 对 v6e
  的 Spearman CI 跨 0。`K10K11` 只是当前优先点，不能写成“已证全面优于 v6e”。
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
```

`wsp_budget_variant.py` 默认把非 up/down 块重置为 int3，避免把规划器原来的
int2/int4 带进消融；要保留 base 的非 3-bit 块，加 `--keep-base`。
`wsp_budget_swap.py` 默认 `--require-exact`，只允许同形状交换；要接受字节变化
加 `--allow-bytes-change`。

## 8. 下一步

- **不要把 K10K11 说成“3-bit 追平 int4”**：还差 `0.0115` Spearman / `0.0140` Pearson；
  它只是同字节下比纯 int3 好，且对 int3 的两个指标都过了 10k CI。
- **K10K11 对 I10 的优势很薄**：50k Spearman 下界 `+0.000048`，对 v6e 的 Spearman
  CI 跨 0。可以把它当当前优先点，不能当强机制结论。
- **下一步不是再手调一个块**：`wsp_budget_swap.py` 已把“同字节、同形状、不同位置”
  做成可复算消融，但 128 筛仍会高估/低估。K9 再加一个 late key 在 128 上只
  `+0.0002–0.0003`，没有全量追。要继续扩，就得做逐块全量扰动表，或者把交换扫描
  从“单片”扩到“成对/成组”，否则每次仍靠全量撞。
- **在位置敏感度没解决前，不把 BUDGET 扩到 int5/f32/侧车，也不宣称它能自动搜索**；
  先把“同字节、同形状、不同位置”这一件事做成可预测。
- MXQ 仍可作为下一步内层自由度，但必须在这套同字节 STS 口径下验收；不能再用
  层内重建 −26–30% 当结论。
- 4-bit 的权重修正（BIAS/SCALE）已经证伪，不再回到那条线。
