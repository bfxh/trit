# WSP-BUDGET：同字节预算分配（L12 / STS 1,379 对）

> 状态：M1 完成。严格同字节 `10,782,720 B`（含 per-group scale 与 per-row bias）上，
> L12 的 Spearman 从 `0.813362` 抬到 `0.818137`，10k 配对 bootstrap 95% CI
> `[+0.002017, +0.007754]`；Pearson 从 `0.818440` 抬到 `0.820686`，CI
> `[-0.000307, +0.004941]`，仍跨 0。**结论限定为“Spearman 同字节小胜”，
> 不是“两个指标都赢”，更不是“追平 int4”。**

## 1. 结论先行

WSP-BUDGET 的第一版规划器按“权重重建误差代理”分配，在 128 对上反而低于纯 int3；
但全量 1,379 对是 `+0.001033 / +0.002893`，说明 128 子集不足以判死。

真正严格同字节、且全量更好的版本是 `v6e`：

- 9 张 384×384 的 q/k 从 int3 升到 int4：层 0–3 的 query/key，加层 4 的 key；
- 3 张矩阵从 int3 降到 int2：层 9 的 `attention.output.dense`、层 11 的
  `intermediate.dense` 与 `output.dense`；
- 其余 60 张保持 int3；
- 总字节 `10,782,720`，与纯 int3 完全相等。

全量结果 `0.820686 / 0.818137`，相对纯 int3 `+0.002246 / +0.004775`。
配对 bootstrap 的 Spearman 下界 `+0.002017`，是本文唯一通过“至少一个指标 95% CI
下界 > 0”的判据；Pearson 下界 `-0.000307`，只能记方向性。

因此 BUDGET 的第一条可落地结论不是“找到更好的位宽”，而是：

> **同一总字节下，块的位置比权重重建误差更重要；权重 MSE 代理无法区分两个同形状、
> 同字节的块，必须换成端到端/位置敏感的敏感度。**

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

`v6e` 的逐块变化：

| 变化 | 块数 | 单块字节变化 | 合计 |
|---|---:|---:|---:|
| int3 → int4（384×384 q/k） | 9 | +18,432 | +165,888 |
| int3 → int2（层 9 o，384×384） | 1 | −18,432 | −18,432 |
| int3 → int2（层 11 i/f） | 2 | −73,728 | −147,456 |
| **净变化** | | | **0** |

单块字节：384×384 的 int2/int3/int4 分别是 `56,832 / 75,264 / 93,696`；
1536×384 的 int3/int2 是 `301,056 / 227,328`；384×1536 的 int3/int2 是
`296,448 / 222,720`。

| 方案 | int2 | int3 | int4 | int5 | 实际字节 | 相对 int3 |
|---|---:|---:|---:|---:|---:|---:|
| 纯 int3 | 0 | 72 | 0 | 0 | 10,782,720 | 0 |
| 规划器 v1 | 3 | 60 | 9 | 0 | 10,782,720 | 0 |
| **v6e（本文）** | **3** | **60** | **9** | **0** | **10,782,720** | **0** |
| v6（非严格） | 3 | 59 | 10 | 0 | 10,801,152 | +18,432 |
| 纯 int4 | 0 | 0 | 72 | 0 | 13,436,928 | +2,654,208 |

`v6e` 与规划器 v1 的区别只有一处：v1 把层 11 的 `attention.output.dense` 降 int2，
v6e 把这一个降级挪到层 9 的同一类矩阵。两者形状、字节、代理误差比完全相同
（`proxy_err_ratio=0.91552`），但 STS 不同——这正是代理看不见位置。

## 3. 读数

128 对（用于 smoke，不作最终判据）：

| 配置 | Pearson | Spearman | 实际字节 |
|---|---:|---:|---:|
| 纯 int3 | 0.897582 | 0.927222 | 10,782,720 |
| 规划器 v1 | 0.885716 | 0.922785 | 10,782,720 |
| **v6e** | **0.901878** | **0.931000** | **10,782,720** |
| 纯 int4 | 0.928494 | 0.950720 | 13,436,928 |

全量 1,379 对：

| 配置 | Pearson | Spearman | 实际字节 | 同字节？ |
|---|---:|---:|---:|---|
| 纯 int3 | 0.818440 | 0.813362 | 10,782,720 | 是 |
| 规划器 v1 | 0.819473 | 0.816255 | 10,782,720 | 是 |
| **v6e** | **0.820686** | **0.818137** | **10,782,720** | **是** |
| v6 | 0.820745 | 0.818020 | 10,801,152 | 否，+18,432 |
| 纯 int4 | 0.837505 | 0.832047 | 13,436,928 | 否 |
| 纯 f32 | 0.837595 | 0.830894 | 84,934,656 | 否 |

相对纯 int3 的全量增量：

| 对比 | Δ Pearson | Δ Spearman |
|---|---:|---:|
| v1 | +0.001033 | +0.002893 |
| **v6e** | **+0.002246** | **+0.004775** |
| v6（非严格） | +0.002304 | +0.004659 |
| v6e vs v1 | +0.001213 | +0.001882 |

v6e 距纯 int4：Pearson `−0.016819`、Spearman `−0.013910`；距 f32：
Pearson `−0.016908`、Spearman `−0.012757`。**预算分配没有关闭 int3 到 int4 的差距。**

v6 比 v6e 多花 18,432 B，把层 4 的 query 也升到 int4；全量 Pearson 高 `0.000059`，
Spearman 反而低 `0.000117`。在 128 对上 v6 也比 v6e 低 `0.000047 / 0.000155`。
因此这 18,432 B 不值得花；严格字节版本删掉它。

## 4. 配对 bootstrap

输入：

- baseline：`l12-int3-preds-full.json`
- candidate：`l12-budget-v6e-preds-full.json`
- 标签：`miniLM-l12-sts-full/pairs.json` 的 `pairs[].score`
- 预测路径：`modes.f32.sts.preds`
- `10,000` 次按 pair 有放回重采样，seed `20261124`

| 指标 | baseline | candidate | Δ | bootstrap mean | 95% CI | 候选更好比例 |
|---|---:|---:|---:|---:|---:|---:|
| Pearson | 0.818440 | 0.820686 | +0.002246 | +0.002263 | [−0.000307, +0.004941] | 95.71% |
| **Spearman** | **0.813362** | **0.818137** | **+0.004775** | **+0.004808** | **[+0.002017, +0.007754]** | **99.98%** |

判据（见 `WSP-ALT.md` §5.4）是“Pearson/Spearman 不低于纯 int3，且至少一个指标
10k CI 下界 > 0”。Spearman 过，Pearson 不过；所以只能写 **Spearman-only 同字节小胜**。

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

## 6. 边界

- **单模型、单数据**：all-MiniLM-L12-v2，STS 1,379 对；128 对只作 smoke。
  没有第二模型、没有检索/分类任务头，不能外推“BUDGET 普遍有效”。
- **非 sentence-disjoint**：BUDGET 代理只用权重，不用校准标签；但评测对与之前
  BIAS/SCALE 实验复用同一 STS 集，仍属探索性读数，不是留出验证。
- **2-bit 语义不完全一致**：规划器按“uniform 2-bit + per-group scale”计费；
  运行时 `bits==2` 实际走已有的三值 `QuantLinear`（absmean/GPTQ 三值），
  两者字节数相同，但误差语义不同。2-bit 块的代理误差不能当三值路径的真实误差。
- **字节账不含容器对齐/打包**：只算线性权重 + scale + bias；实际格式若加头、
  对齐、索引，总字节会变。运行时 JSON 的 `weight_format` 仍按全局
  `TRIT_WEIGHT_BITS` 报账，**mixed 的实际总字节还没有接进报告**；本文的
  10,782,720 是离线按块复算值，不是报告里的 `weight_bytes=7,962,624`。
- **只有一种分配族**：9 升 3 降，且只在层 0–4 q/k 与层 9/11 的 o/i/f 上试；
  没有搜索全部 72 块的组合，也没有 int5/f32/侧车候选。
- 没有公开文献逐篇核对；混合精度、预算分配都不是新原理。本文只声明本仓同口径的
  L12/STS 读数，不声明“首次/独有”。

## 7. 复算

```bash
# 1) 规划器：预算 = 纯 int3 的实际字节
python scripts/wsp_budget.py \
  --model /path/all-MiniLM-L12-v2.safetensors \
  --budget 10782720 --out budget-l12-int3.json

# 2) 严格同字节变体：9 个 q/k 升 int4，3 个块降 int2；其余重置为 int3
python scripts/wsp_budget_variant.py \
  --base budget-l12-int3.json \
  --up 0:q,0:k,1:q,1:k,2:q,2:k,3:q,3:k,4:k \
  --down 9:o,11:i,11:f \
  --require-exact --out budget-l12-int3-v6e.json

# 3) 跑全量并导出 preds（Windows PowerShell 先逐条 $env:TRIT_...）
TRIT_LAYERS=12 TRIT_WEIGHT_BITS=3 TRIT_BUDGET=budget-l12-int3-v6e.json \
TRIT_THREADS=24 TRIT_SKIP_DIAG=1 TRIT_NO_LAYERS=1 TRIT_DUMP_PREDS=1 \
TRIT_F32_CACHE=l12-f32-cache-full-preds.json \
bert6 --file all-MiniLM-L12-v2.safetensors \
  --data-dir miniLM-l12-sts-full --activation-mode f32 \
  --out l12-budget-v6e-preds-full.json

# 4) 配对 bootstrap
python scripts/wsp_budget_bootstrap.py \
  --baseline l12-int3-preds-full.json \
  --candidate l12-budget-v6e-preds-full.json \
  --pairs miniLM-l12-sts-full/pairs.json \
  --out bootstrap-v6e-vs-int3.json
```

`wsp_budget_variant.py` 默认把非 up/down 块重置为 int3，避免把规划器原来的
int2/int4 带进消融；要保留 base 的非 3-bit 块，加 `--keep-base`。

## 8. 下一步

- **不要把 v6e 说成“3-bit 追平 int4”**：还差 `0.0139` Spearman / `0.0168` Pearson；
  它只是同字节下比纯 int3 好一点。
- **BUDGET 的下一个真问题**是敏感度，不是再手调块：需要一张位置敏感的逐块扰动表
  （例如逐块 int2/回 f32 后对最终句向量的影响），替代权重 MSE 代理；
  否则每次分配都只能靠全量消融撞。
- 在位置敏感度没解决前，不把 BUDGET 扩到 int5/f32/侧车，也不宣称它能自动搜索；
  先把“同字节、同形状、不同位置”这一件事做成可预测。
- MXQ 仍可作为下一步内层自由度，但必须在这套同字节 STS 口径下验收；不能再用
  层内重建 −26–30% 当结论。
- 4-bit 的权重修正（BIAS/SCALE）已经证伪，不再回到那条线。
