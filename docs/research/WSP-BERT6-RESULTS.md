# MiniLM 完整 6 层：f32 对照与三值线性权重

把 `bias_e2e` 的三段线性链升级成 MiniLM 的真实 6 层前向：attention（12 heads）、
LayerNorm、GELU、残差都在 f32 里跑，只把线性权重三值化。目标是回答两个问题：

1. 三值权重在**真实 transformer 链**上到底掉多少，而不是只掉在三段线性段上；
2. 第 0 层单段 `output` 的“随机输入低估”结论，能不能外推到完整模型。

## 1. 协议

- 模型：`sentence-transformers/all-MiniLM-L6-v2`（6 层、384 hidden、12 heads、1536 FFN）。
- 文本：`mteb/stsbenchmark-sts` test 前 8 句，padding 到 `seq=64`。
- 输入：PyTorch 第 0 层输入 hidden states（`q0` hook）+ attention mask。
- 参照：PyTorch `last_hidden_state`；逐层另存 `layer0..5` 用于对齐。
- Rust 前向：6 层 BERT，Q/K/V/O + FFN 线性权重三值化（absmean 逐行 scale，
  保留 f32 bias）；LayerNorm/GELU/softmax/残差保持 f32。只比较 `mask=1` 的 token。
- GELU 用 erf 解析近似；f32 对齐读数见下（6e-7，足够把实现误差与三值化误差分开）。

导出脚本（只作对齐基线）：

```bash
TRIT_DATA=<数据根> HF_HUB_OFFLINE=1 \
  python crates/trit-wspace/examples/export_bert6.py \
  "$TRIT_DATA/miniLM-bert6" 8
```

复算：

```bash
cargo run --release -p trit-wspace --example bert6 -- \
  --file "$HF_HOME/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors" \
  --data-dir "$TRIT_DATA/miniLM-bert6"
```

## 2. 读数

| 项 | `rel_l2` | `cosine` |
|---|---:|---:|
| Rust f32 vs PyTorch 参照 | **6.06e-7** | **1.0000** |
| 三值（真实输入）vs f32 | **1.1594** | **0.4690** |
| 三值（随机输入）vs 同输入 f32 | **1.5488** | **0.3117** |

逐层 f32 对齐：`layer0..4` 的 `rel_l2 = 0`，`layer5 = 1e-6`；实现与 PyTorch 实质逐位一致。

output 段（FFN 的 GELU 后输入）统计：

| 层 | real std | real max | real 超额峰度 | random std | random max | random 超额峰度 |
|---|---:|---:|---:|---:|---:|---:|
| 0 | 0.176 | 8.83 | **99.6** | 0.145 | 2.82 | 13.0 |
| 1 | 0.128 | 2.30 | 35.3 | 0.125 | 2.21 | 15.7 |
| 2 | 0.120 | 2.23 | 13.5 | 0.117 | 1.87 | 17.6 |
| 3 | 0.134 | 3.21 | 15.8 | 0.114 | 1.60 | 12.3 |
| 4 | 0.128 | 3.24 | 15.5 | 0.121 | 1.94 | 9.4 |
| 5 | 0.131 | 2.05 | 18.9 | 0.142 | 2.17 | 11.6 |

## 3. 读法

1. **f32 对齐 6e-7**：Rust 6 层前向可以当 PyTorch 的近似等价实现；下面的三值读数
   不是实现误差。
2. **三值化所有线性权重后，真实输入 `rel_l2 1.16 / cosine 0.47`**：完整 6 层不是
   “三段线性链的 0.92”，也不是“单层 0.49”；误差在 attention/FFN 之间重新分配。
3. **随机输入在完整 6 层上反而更差**：`rel_l2 1.55 / cosine 0.31`，比真实输入高
   **34%**（1.549/1.159）。这与第 0 层单段 `output` 的“随机低估 1.72×”**方向相反**。
4. 因此：**单层 output 段的重尾（kurt 99.6）不足以解释完整模型**；经过后续
   LayerNorm/残差/GELU 后，随机输入落到了更不利的分布。任何“随机输入是乐观还是悲观”
   的结论都必须写明是哪一层、哪一段、是不是完整链。
5. output 段真实输入的 layer0 重尾（99.6、max 8.83）仍然是最强的局部信号；它解释了
   第 0 层单段为什么对输入分布敏感，但不能外推到 6 层最终读数。

## 3.5 全三值激活（LN 也三值）

`--activation-mode` 三档：`f32`（LN/激活保持 f32，只三值化线性权重）、`ternary-ln`
（LN 的 weight 三值化，bias 保 f32）、`ternary-all`（LN weight 三值化 + LN/GELU 输出
符号三值化）。

| 模式 | 真实 `rel_l2` | 真实 `cosine` | 随机 `rel_l2` | 随机 `cosine` |
|---|---:|---:|---:|---:|
| f32（仅线性权重） | 1.1594 | 0.4690 | 1.5488 | 0.3117 |
| ternary-ln | 1.3647 | 0.3516 | 1.6907 | 0.2312 |
| ternary-all | **2.4475** | **0.0345** | **3.0896** | **0.0382** |

读法：

1. 只把 LN 的 weight 三值化，真实 cosine 从 0.469 掉到 0.352（−25%）；
2. 再把 LN/GELU 输出符号三值化，cosine 掉到 **0.03**，方向信息几乎全丢；
3. **当前 absmean LN weight + sign 激活方案在 `ternary-all` 下真实 cosine 只有 0.0345**
   （复算：`--activation-mode ternary-all`）。要保留方向，
   需要 QAT/学习式 scale、混合精度（LN 输出保 f32）或残差侧车；
4. 随机输入在三种模式下都比真实输入更差（rel_l2 高 25%–34%），方向与 §3 一致。

## 3.6 STS 任务头（128 句 / 64 对）

把 `bias_e2e` 的 hidden 读数接到 STS benchmark：`export_bert6.py` 现在同时导出
`pairs.json`（64 对、人工分）；Rust 侧按 mask 做 mean pooling + L2 normalize，
对每对算余弦，再与人工分算 Pearson / Spearman。

| 模型 | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---|---:|---:|---:|---:|
| PyTorch f32 参照 | — | — | **0.9230** | **0.9243** |
| 三值线性权重（f32 激活） | 1.1660 | 0.4695 | **0.7330** | **0.7437** |
| ternary-ln | 1.3999 | 0.3583 | **0.5461** | **0.5310** |
| ternary-all | 2.5051 | 0.0275 | **0.2472** | **0.2494** |

读法：

1. **仅三值化线性权重**，STS 相关性从 0.923 掉到 0.733（−0.19；相对 −21%）。
   这比 hidden `cosine 0.47` 看起来“好看”，但仍是明确掉点。
2. **LN weight 也三值化**：STS 相关性再掉到 0.546；**全三值激活**只剩 0.247，
   基本丢失 STS 相关性。
3. 任务头方向与 hidden 读数一致：越往激活侧三值化，相关性越低；`ternary-all`
   的 0.247 已经是“弱相关”而不是可用的句向量。

## 3.7 激活 scale：per-token vs per-channel

在 `ternary-all`（LN/GELU 输出纯 sign）之上加两种幅度 scale：

- `ternary-all-scale`：每个 token 的 `mean(|x|)`（per-token）；
- `ternary-all-channel`：用 f32 forward 在校准集上统计的 per-channel absmean
  （`scales.json`，由 `export_bert6.py` 导出）。

| 模式 | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---|---:|---:|---:|---:|
| 仅线性权重（f32 激活） | 1.1660 | 0.4695 | **0.7330** | **0.7437** |
| ternary-ln | 1.3999 | 0.3583 | 0.5461 | 0.5310 |
| ternary-all（纯 sign） | 2.5051 | 0.0275 | 0.2472 | 0.2494 |
| ternary-all-scale（per-token） | 1.5598 | 0.1837 | 0.3576 | 0.3846 |
| ternary-all-channel（per-channel，f32 校准） | 1.2373 | 0.0100 | **0.4322** | 0.3889 |

读法：

1. per-token scale 把 STS Pearson 从 0.247 拉到 **0.358**（+0.11），cosine 0.03→0.18；
2. per-channel scale 再拉到 **0.432**，**超过 0.4 目标**（Pearson），但 Spearman 0.389 仍差一点；
   cosine 只有 0.01——固定 per-channel 幅度能把排序救回一些，但 hidden 方向仍严重偏离；
3. 学习式 scale 的闭式解 `s=(q·y)/(q·q)`：当 `q=sign(y)` 时它**就是** per-channel absmean，
   所以 `ternary-all-channel` 的 0.432 已经是最小二乘闭式解在这个量化口径下的读数；
   用三值模型的 q 与 f32 目标 y 求 scale 会更小，需要额外对齐，未做；
4. 距仅线性权重的 0.733 仍有 0.30；要逼近它，需要残差侧车或混合精度，而不是继续调激活 scale。

## 3.8 残差侧车：top-k 列 f32 修正（负结果）

对每个三值线性层算 `R = W − s·T`，按列范数保留 top-k 列，推理时
`y = s·T x + R_k x_k`；`ternary-all-channel` + 侧车，扫 k=0/4/8/16。

| rank | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---:|---:|---:|---:|---:|
| 0（仅三值） | 1.2373 | 0.0100 | **0.4322** | 0.3889 |
| 4 | 1.2363 | 0.0116 | 0.4091 | 0.4109 |
| 8 | 1.2356 | 0.0128 | 0.3940 | 0.3961 |
| 16 | 1.2386 | 0.0077 | 0.4259 | 0.4028 |

结论：

1. **top-k 列侧车没有把 0.432 推向 0.733**：Pearson 0.432 → 0.409 / 0.394 / 0.426，
   `rel_l2` 几乎不变（1.2373 → 1.2363/1.2356/1.2386）；
2. 误差主导在**激活三值化**，不在权重残差；k≤16 的列修正量太小，且与激活量化误差
   不同向，不能救回 hidden 方向；
3. 要逼近 0.733，需要混合精度（LN/残差流保 f32）、更大的 rank（接近 in_dim 时等价于
   f32 线性层），或改激活侧量化——不是继续加列侧车。

## 3.9 混合精度：LN/残差 f32、matmul 输入三值（负结果）

`--activation-mode mixed`：LN/残差流保持 f32，权重三值，Q/K/V、intermediate、FFN 输出的
matmul 输入在进入线性层前符号三值化；attention context 保持 f32。

| 模式 | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---|---:|---:|---:|---:|
| mixed | 1.6053 | 0.0082 | 0.2191 | 0.1896 |
| 全三值 + per-channel | 1.2373 | 0.0100 | 0.4322 | 0.3889 |
| 仅线性权重（f32 激活） | 1.1660 | 0.4695 | 0.7330 | 0.7437 |

读法：

1. 只把 matmul 输入三值化、LN/残差保 f32，Pearson **0.219**，比全三值激活的 0.247 还低；
2. 瓶颈是 **matmul 输入的三值化本身**，不是 LN/残差；即使 context 保持 f32（去掉对
   context 的量化），也只回到 0.219/0.190；
3. 所以“LN/残差 f32 + matmul 输入三值 + 权重三值”没有把 0.432 推向 0.733；
4. 真正可用的混合精度是 **仅权重三值 + 激活 f32**（`f32` 模式，0.733）；要进一步接近
   PyTorch 的 0.923，需要权重侧精度（GPTQ/更大侧车）或 8-bit 激活，而不是三值输入。

## 3.10 int8 激活：权重三值 + 8-bit matmul 输入

`--activation-mode int8`：权重三值，LN/残差 f32，matmul 输入 per-token 对称 int8
（`scale = max|x|/127`，反量化后参与 matmul）。

| 模式 | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---|---:|---:|---:|---:|
| int8（权重三值） | 1.1660 | 0.4695 | **0.7331** | **0.7446** |
| 仅线性权重（f32 激活） | 1.1660 | 0.4695 | 0.7330 | 0.7437 |
| 全三值 + per-channel | 1.2373 | 0.0100 | 0.4322 | 0.3889 |
| mixed（matmul 输入三值） | 1.6053 | 0.0082 | 0.2191 | 0.1896 |

读法：

1. **int8 激活与 f32 激活几乎完全相同**（Pearson 0.7331 vs 0.7330，Spearman 0.7446 vs
   0.7437），说明 8-bit 足够保留激活信息，**激活带宽不是瓶颈**；
2. 权重三值化本身把 PyTorch 的 0.923 拉到 0.733（−0.19）；要接近 0.923，必须做
   **权重侧精度**（GPTQ 逐列补偿/更大 rank 侧车/混合权重），而不是继续压激活；
3. 三值激活这条线已经到顶：纯 sign 0.247 → per-token 0.358 → per-channel 0.432 →
   matmul 输入三值 0.219；int8 是唯一不损失激活精度的低比特方案。

## 3.11 权重侧 GPTQ 逐列补偿：STS 0.7330 → 0.8065

`--gptq`：用 f32 校准集的 `H = XᵀX`（6 层 × 6 段线性层，128 句、mask 后的 2048 个 token）
逐列量化三值权重并把量化误差补偿到尚未处理的列：

```text
q_j = clamp(round(w_j / s), -1, 1)
e_j = (w_j - s·q_j) / [H⁻¹]_jj
w_k -= e_j · [H⁻¹]_jk              (k 尚未量化)
```

列顺序按 `diag(H)` 降序（GPTQ 的 act-order），`H` 加 1% 对角阻尼保证可逆；`s` 仍是
逐行 absmean，所以两边的 `trits` 只差补偿/重排带来的少数列，可直接对照。

导出 Hessian 与复算：

```bash
TRIT_DATA=<数据根> HF_HUB_OFFLINE=1   python crates/trit-wspace/examples/export_bert6.py "$TRIT_DATA/miniLM-bert6-sts" 128
cargo run --release -p trit-wspace --example bert6 --   --file "$HF_HOME/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors"   --data-dir "$TRIT_DATA/miniLM-bert6-sts" --activation-mode f32 --gptq
```

| 权重三值化 | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---|---:|---:|---:|---:|
| absmean 逐行（上一节基线） | 1.1660 | 0.4695 | 0.7330 | 0.7437 |
| **GPTQ 逐列补偿** | **1.0491** | **0.5551** | **0.8065** | **0.8320** |
| PyTorch f32 对照 | — | — | 0.9230 | — |

读法：

1. **GPTQ 把权重侧 Pearson 从 0.7330 拉到 0.8065**（+0.0735，相对 +10.0%），
   Spearman 0.7437 → 0.8320，`rel_l2 1.1660 → 1.0491`、`cosine 0.4695 → 0.5551`；
   这是第一次在**不改激活**的前提下把 0.733 推过 0.8。
2. **补偿系数必须用 `H⁻¹` 而不是 `H`**。第一版实现按 `H[j,k]/H[j,j]` 补偿（漏了求逆），
   H 加权误差从 absmean 的 `0.266` **涨到** `0.860`（36 个矩阵全部变大），端到端
   Pearson 掉到 **0.4924**——比 absmean 的 0.733 还低。修正后 36 个矩阵的 H 加权误差
   全部下降（query 段 `0.266 → 0.167`，−37%）。这条负结果留在
   `crates/trit-wspace/src/gptq.rs` 的模块文档里，并有 `tests/gptq.rs` 的方向性断言守着。
3. 与 PyTorch f32 的 0.9230 还差 **0.1165**；剩下的缺口在权重侧（更大 rank 侧车、
   混合权重、per-group scale、QAT），激活侧已经证明不是瓶颈（§3.10）。

## 3.12 per-group scale + GPTQ：STS 0.8065 → 0.8290

`--gptq-group N`：把每行的 scale 从"整行一个 absmean"改成"每 N 列一个 absmean"，
与 §3.11 的 GPTQ 逐列补偿叠加（不用重新导出校准数据）。

```bash
cargo run --release -p trit-wspace --example bert6 --   --file "$HF_HOME/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors"   --data-dir "$TRIT_DATA/miniLM-bert6-sts" --activation-mode f32 --gptq-group 32
```

`scales` 布局是 `(row * groups + g)`，`groups = ceil(cols / group)`，末尾不满组按实际
长度算 absmean；`group >= cols` 退化为 §3.11 的 per-row。

| 权重三值化（激活 f32） | `rel_l2` | `cosine` | STS Pearson | STS Spearman |
|---|---:|---:|---:|---:|
| absmean per-row（§3.11 基线） | 1.1660 | 0.4695 | 0.7330 | 0.7437 |
| GPTQ per-row | 1.0491 | 0.5551 | 0.8065 | 0.8320 |
| GPTQ group=128 | 1.0468 | 0.5612 | 0.8144 | 0.8333 |
| GPTQ group=64 | 1.0437 | 0.5562 | 0.8159 | 0.8408 |
| GPTQ group=32 | 1.0479 | 0.5558 | 0.8272 | 0.8464 |
| **GPTQ group=16** | 1.0448 | 0.5599 | **0.8290** | 0.8460 |

读法：

1. **粒度越细 Pearson 越高**：0.8065 → 0.8144 → 0.8159 → 0.8272 → 0.8290；
   group=32 比 per-row 再 +0.0207（+2.6% 相对），group=16 只再 +0.0018 ⇒
   **32 附近已饱和**，再细的粒度不值得 4 倍 scale 存储。
2. `rel_l2` 与 `cosine` 并不随粒度单调（1.0491 → 1.0468 → 1.0437 → 1.0479 → 1.0448），
   说明 per-group 改的是**误差在行内的分配**，行级幅度几乎不变；STS 对这种分配更敏感。
3. 相对 absmean：group=32 的 Pearson +0.0942（**+12.9% 相对**）；相对 PyTorch f32 的
   0.9230 仍差 0.0940。
4. 只改权重侧：激活保持 f32（§3.10 已证 8-bit 不损失精度），校准集与 §3.11 同一份。

## 3.13 128 对复验：0.829 是 64 对的偏乐观读数，排序与相对增益稳定

把评测/校准数据从 128 句（64 对）扩到 256 句（128 对），导出脚本、口径、Hessian 提取
（mask 后前 2048 个 token）都不变：

```bash
TRIT_DATA=<数据根> HF_HUB_OFFLINE=1   python crates/trit-wspace/examples/export_bert6.py "$TRIT_DATA/miniLM-bert6-sts-128" 256
```

| 配置（权重三值 + 激活 f32） | 64 对 Pearson | 128 对 Pearson | 64→128 |
|---|---:|---:|---:|
| PyTorch f32 基线 | 0.9230 | 0.9235 | +0.0005 |
| absmean per-row | 0.7330 | 0.6966 | −0.0364 |
| GPTQ per-row | 0.8065 | 0.7457 | −0.0608 |
| GPTQ group=32 | 0.8272 | **0.7893** | −0.0379 |

| 配置 | 64 对 Spearman | 128 对 Spearman |
|---|---:|---:|
| absmean per-row | 0.7437 | 0.7058 |
| GPTQ per-row | 0.8320 | 0.7580 |
| GPTQ group=32 | 0.8464 | 0.8084 |

读法：

1. **f32 基线在扩样本后几乎不动**（0.9230 → 0.9235），说明 128 对的评测口径与 64 对一致，
   下面的下降不是口径变化。
2. **三值配置全部下降 0.034–0.061**：`0.829` 只能当"前 64 对"的读数，128 对上的 GPTQ
   group=32 是 **0.7893**。后 64 对（第 129–256 句）明显更难，小样本会系统性偏乐观。
3. **排序与相对增益稳定**：GPTQ 相对 absmean 在 64 对是 +0.0735、128 对 +0.0491；
   group=32 相对 per-row 在 64 对 +0.0207、128 对 **+0.0436**（更大）。两条改进都不是
   小样本噪声。
4. 权重三值化相对 f32 的代价：absmean −0.2269、GPTQ per-row −0.1778、GPTQ group=32
   **−0.1342**（128 对口径）。
5. 以后报读数必须写明子集大小（64 对 / 128 对），不能把不同子集上的数字混着比。

## 3.14 全量 STS（1379 对）：f32 基线 0.8274，GPTQ 增益收窄，per-group 几乎消失

数据是 STS test 全量 1379 对（2758 句）。全量下逐层快照要 6×271 MB，导出用 `--no-layers`
跳过；评测用 `TRIT_NO_LAYERS=1` 关掉逐层输出，三组串行跑（并行会 OOM）。

```bash
TRIT_DATA=<数据根> HF_HUB_OFFLINE=1   python crates/trit-wspace/examples/export_bert6.py "$TRIT_DATA/miniLM-bert6-sts-full" 2758 --no-layers
TRIT_NO_LAYERS=1 cargo run --release -p trit-wspace --example bert6 --   --file "$HF_HOME/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors"   --data-dir "$TRIT_DATA/miniLM-bert6-sts-full" --activation-mode f32 --gptq-group 32
```

| 配置（权重三值 + 激活 f32） | `rel_l2` | `cosine` | Pearson | Spearman |
|---|---:|---:|---:|---:|
| f32 基线 | — | — | **0.8274** | 0.8203 |
| absmean per-row | 1.1803 | 0.4977 | 0.6870 | 0.6950 |
| GPTQ per-row | 1.1432 | 0.5166 | 0.7123 | 0.7153 |
| GPTQ group=32 | 1.1365 | 0.5192 | **0.7143** | **0.7190** |

子集演进（Pearson）：

| 子集 | f32 基线 | absmean | GPTQ | group=32 | GPTQ−absmean | g32−GPTQ |
|---|---:|---:|---:|---:|---:|---:|
| 64 对（§3.11/3.12） | 0.9230 | 0.7330 | 0.8065 | 0.8272 | +0.0735 | +0.0207 |
| 128 对（§3.13） | 0.9235 | 0.6966 | 0.7457 | 0.7893 | +0.0491 | +0.0436 |
| **1379 对（全量）** | **0.8274** | **0.6870** | **0.7123** | **0.7143** | **+0.0253** | **+0.0020** |

读法：

1. **f32 基线自己随子集扩大而下降**（0.9230 / 0.9235 → 0.8274）：前 128 句是容易样本。
   之前几节「距 f32 差 0.19 / 0.09」的对比全部建立在乐观基线上，对外报数必须换成全量口径。
2. **GPTQ 相对 absmean 的净增益随样本扩大收窄**：+0.0735 → +0.0491 → **+0.0253**。
   方向仍然一致（三次都是正），但量级远小于 64 对的读数。
3. **per-group 在全量上几乎无额外收益**（+0.0020，Spearman +0.0037）：§3.12 的 +0.02~+0.04
   是子集特异现象，不能当普适结论；per-group 的存储/实现成本换不来全量收益。
4. 权重三值化相对 f32 的代价（全量）：absmean **−0.1404**、GPTQ **−0.1151**、
   group=32 **−0.1131**。
5. 全量口径下结论：**权重侧 GPTQ 是有效但收益有限的一刀（+0.025）**；per-group 无效；
   想再往上走需要更大 rank 侧车、QAT 或混合精度，而不是继续调 scale 粒度。

## 3.15 低秩残差侧车：全量 rank 16 回收 55% 的权重差距，代价是字节 ×2

三值主干 `W ≈ s·T` 把误差 `R = W − s·T` 整块丢掉；GPTQ 只是把 R 在 Hessian 方向上压小，
并没有把 R 留下来。本节把 R 的前 r 个奇异三元组留成 f32 侧车：

```text
y = s·(T x) + U_r Σ_r V_rᵀ x
```

`rank = min(rows, cols)` 时行为上等价于完整 f32 线性层（8 句数据上 `rel_l2 = 2.6e-7`，
sidecar 参数 15.9M，是 dense 的 1.5×）；更小的 rank 是同一组奇异向量的**前缀**，
`set_rank` 只收缩不重排，所以**权重误差**关于 rank 单调。STS 是句向量余弦的排序指标，
不保证随权重误差单调——全量上 rank 32/64 都停在 rank 16 水平甚至略低（见读法 3）。

侧车按 `rank·(rows + cols)` 个 f32 存储；分解走 `RᵀR` / `R Rᵀ` 的 Jacobi 全分解
（`crates/trit-wspace/src/residual.rs`），36 个矩阵一次性构建约 100 s
（全量进程实测 `build_ms = 100.8`；8 句空载对照：无侧车 0.17 s、有侧车 81.9 s）。

复算（全量 1379 对）：

```bash
TRIT_NO_LAYERS=1 cargo run --release -p trit-wspace --example bert6 -- \
  --file "$HF_HOME/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/1110a243fdf4706b3f48f1d95db1a4f5529b4d41/model.safetensors" \
  --data-dir "$TRIT_DATA/miniLM-bert6-sts-full" \
  --activation-mode f32 --gptq-group 32 --sidecar-svd 16,64,128 \
  --out "$TRIT_DATA/tmpfull-svd.json"
```

| rank | 侧车参数 / dense f32 | 侧车字节 / 三值主干字节 | `rel_l2` | `cosine` | Pearson | Spearman | 差距回收 |
|---|---:|---:|---:|---:|---:|---:|---:|
| 0（GPTQ g32，§3.14） | 0 | 0 | 1.1365 | 0.5192 | 0.7143 | 0.7190 | — |
| 16 | 6.25% | 1.0× | 0.8736 | 0.5908 | **0.7769** | **0.7776** | **55%** |
| 32 | 12.5% | 2.0× | 0.8318 | 0.6040 | 0.7752 | 0.7760 | 54% |
| 64 | 25% | 4.0× | 0.7912 | 0.6413 | 0.7677 | 0.7696 | 47% |
| 128 | 50% | 8.0× | 0.6580 | 0.7584 | **0.7995** | **0.7966** | **75%** |
| f32 权重基线（§3.14） | — | — | — | — | 0.8274 | 0.8203 | 100% |

rank16/64/128 来自同一趟（`tmpfull-svd.json`）；rank32 是单独一趟（同机、同数据、同参数，
把 `--sidecar-svd` 换成 `32`、`--out` 换成 `tmpfull-svd-r32.json`）。两趟的 f32 与 rank0
读数逐位一致（Pearson 0.827406… / 0.714308…），可交叉复核。

读法（全量口径）：

1. **rank 16 回收 55% 的权重差距**（`(0.7769−0.7143)/(0.8274−0.7143)`），代价是侧车字节
   恰好等于 2-bit 三值主干（权重存储 ×2，折合 dense f32 的 12.5%，不含 scale/bias）。
   这是目前性价比最好的一档：比 GPTQ 的 +0.0253 大一个量级，也比翻倍字节的 rank 32 略高。
2. **rank 128 回收 75%**（0.7995），但侧车是主干字节的 8×、dense 参数的 50%。
   16→128 的 Pearson 差 0.0226；按 Fisher z（n=1379，1 SE ≈ 0.027 z ≈ 0.011 r）约 **2.1 SE**，
   超出噪声。
3. **rank 16 / 32 / 64 在统计上不可区分，中间档不值**：rank 32 的 `rel_l2`（0.8318）和
   `cosine`（0.6040）都比 rank 16（0.8736 / 0.5908）好，Pearson 却低 0.0017
   （0.7752 vs 0.7769，约 0.2 SE）；rank 64 同样 `rel_l2` 更低，Pearson 低 0.0092（0.9 SE）。
   三档落在 0.7677–0.7769，跨不过噪声；只有 rank 128 跳到 0.7995（比 rank 16 高 0.0226，
   约 2.1 SE）。从 rank 16 加到 32 会让侧车字节翻倍（1×→2× 主干）却不涨点，要跨噪声得直接
   上 8× 的 rank 128。**STS 不随权重误差单调，报侧车读数必须同时给 `rel_l2` 与 STS，
   不能拿前者代理后者**。
4. **存储口径**：主干 1 trit = 2 bit；侧车 1 参数 = 4 B。「×N」只算权重，不含 per-group
   scale/bias，也不含容器开销。
5. **[B] 归类**：侧车是二进制硬件上的混合精度实现（f32 附加张量 + 约 100 s 离线构建），
   不是三值原生红利；能回收多少误差取决于 rank 与 GPTQ 残差结构，结论只在给定 rank 下成立。
   二进制基线是同字节预算的「三值主干 + f32 侧车」本身（`rank = 0` 行与 f32 行），
   没有拿三值去比未压缩的 f32 权重。

### 3.15.1 128 对先导（同一代码路径）

先导集是 STS test 前 128 对。绝对值与全量不可直接比（容易样本偏乐观），只用来选 rank：

| rank | `rel_l2` | `cosine` | Pearson | Spearman |
|---|---:|---:|---:|---:|
| 0（GPTQ g32） | 1.0393 | 0.5562 | 0.7893 | 0.8084 |
| 16 | 0.8627 | 0.6185 | 0.8378 | 0.8680 |
| 32 | 0.8236 | 0.6298 | 0.8508 | 0.8790 |
| 64 | 0.7952 | 0.6451 | 0.8636 | 0.8863 |
| 128 | 0.6555 | 0.7645 | 0.8950 | 0.9169 |
| f32 权重基线 | — | — | 0.9235 | 0.9430 |

128 对上 rank 16 增益 +0.0485、rank 32 +0.0615、rank 128 +0.1057；全量对应
+0.0626 / +0.0609 / +0.0852。先导集上 rank 32 看起来比 16 好 0.013，全量只剩 −0.0017：
**128 对样本太小，只能用来选 rank 档位，不能用来判中间档的优劣**；报数一律用全量。

## 3.16 均匀位宽反扫：5-bit 在 1379 对上首次越过 f32

§3.15 的侧车是「三值主干 + f32 修正」。反过来问：同样字节直接给每个权重更多级
（均匀 int3/4/5/6/8）是不是更好？2026-11-23 在 `feat/wsp-quant-compare` 上加了
`TRIT_WEIGHT_BITS`（2 = 三值，3..8 = 均匀对称 per-group）、`TRIT_UNIFORM_GPTQ`
（1 = 均匀位宽走 GPTQ）、`TRIT_F32_LAYERS`（逐层保 f32）和 `TRIT_LAYERS`（层数）。
前向、数据、GPTQ Hessian 都不变，只改权重表示。

存储口径：下表把线性权重、per-group scale 和 bias 都算进去；f32 行是线性权重
42,467,328 B + bias 82,944 B = 42,550,272 B，作为 100%。

| 格式 | 权重 B | scale B | bias B | 合计 B | 占 f32 |
|---|---:|---:|---:|---:|---:|
| f32 | 42,467,328 | 0 | 82,944 | 42,550,272 | 100% |
| 三值 g32 | 2,654,208 | 1,327,104 | 82,944 | 4,064,256 | 9.55% |
| rank16 侧车 g32 | 5,308,416 | 1,327,104 | 82,944 | 6,718,464 | 15.79% |
| int4 g32 | 5,308,416 | 1,327,104 | 82,944 | 6,718,464 | 15.79% |
| int3 g32 | 3,981,312 | 1,327,104 | 82,944 | 5,391,360 | 12.67% |
| int5 g64 | 6,635,520 | 663,552 | 82,944 | 7,382,016 | 17.35% |
| int5 g128 | 6,635,520 | 331,776 | 82,944 | 7,050,240 | 16.57% |

全量 1379 对（同一进程先算 f32 基线，再算量化）：

| 格式 | `rel_l2` | Pearson | Spearman | 合计 B |
|---|---:|---:|---:|---:|
| f32 | — | 0.827406 | 0.820325 | 42,550,272 |
| 三值 g32 | 1.1365 | 0.714308 | 0.719042 | 4,064,256 |
| rank16 侧车 | 0.8736 | 0.776944 | 0.777599 | 6,718,464 |
| int4 g32 | 0.2750 | 0.826061 | 0.819354 | 6,718,464 |
| int4 GPTQ g32 | 0.2271 | 0.826129 | 0.819532 | 6,718,464 |
| int5 g64 | 0.1437 | 0.828356 | 0.820752 | 7,382,016 |
| int5 g128 | 0.1614 | **0.829203** | **0.821784** | 7,050,240 |
| int5 g32 | 0.1290 | 0.826859 | 0.819054 | 8,045,568 |

**int5 g128 是目前最好的一档**：Pearson 0.829203、Spearman 0.821784，比 f32 基线高
+0.00180 / +0.00146，总字节 7,050,240 B（f32 的 16.57%，**6.03× 压缩**）。
int5 g64 也越过（+0.00095 / +0.00043），但总字节更大、增益更小；int5 g32 全量反而
低 0.00055。g128 在 128 对上比 g64 低（0.9248 vs 0.9259），全量却最高，再次说明
128 对不能用来判 0.001 量级的优劣，group 也不能靠小样本单调外推。

int4 g32 用 6,718,464 B（15.79%，6.33× 压缩）拿到 0.826061 / 0.819354，只比 f32 低
0.00135 / 0.00097；**同字节的 rank16 只有 0.776944**。如果目标是「不掉点」，int4 是
更划算的一档。补跑 4-bit GPTQ g32 后，`rel_l2` 从 0.2750 降到 0.2271（−17%）、
cosine 从 0.9617 升到 0.9739，但 Pearson 只涨 0.000068（0.826129），仍低 f32
0.00128：**误差补偿补不回这一档的 STS 差距**，和 5-bit GPTQ 的结论一致。
如果目标是「字面越过」，int5 g128 是当前答案。

但按单相关系数的 Fisher z（n=1379，1 SE ≈ 0.011 r）看，+0.00180 只有约 0.16 SE，
**不能当作统计显著**；它只能说明「同架构同数据、5 bit 均匀量化不会必然输给 f32」。
要下「稳定更强」的结论必须做配对检验或在 1251 对留出集复验。g64/g128 还是先由
128 对选档，严格说全量读数带一点选参泄漏。

### 3.16.1 128 对先导：位宽和 group 都不是单调的

| 格式 | `rel_l2` | Pearson | 说明 |
|---|---:|---:|---|
| f32 | — | 0.923497 | 基线 |
| 三值 g32 | 1.0393 | 0.7893 | 同 §3.15.1 |
| rank16 | 0.8627 | 0.8378 | 同字节对照 |
| int4 g32（RTN） | 0.2583 | 0.9191 | 5.31 MB 权重 |
| int4 g16 | 0.2223 | 0.9204 | scale 翻倍，涨 0.0013 |
| int4 g64 | 0.2913 | 0.9182 | scale 减半，掉 0.0009 |
| int4 g128 | 0.3136 | 0.9180 | — |
| int4 GPTQ g32 | 0.1650 | 0.9214 | 误差降 36%，只涨 0.0023 |
| int3 g32 | 0.5346 | 0.8906 | 5.39 MB 总字节 |
| int5 g16 | 0.1057 | 0.9213 | — |
| int5 g32 | 0.1209 | 0.9254 | — |
| int5 g64 | 0.1307 | **0.9259** | 128 对峰值 |
| int5 g128 | 0.1490 | 0.9248 | — |
| int5 g256 | 0.1604 | 0.9229 | — |
| int5 g512 | 0.1831 | 0.9225 | — |
| int5 GPTQ g32 | 0.0729 | 0.9223 | 误差最低，Pearson 反低于 RTN |
| int6 g32 | 0.0595 | 0.9232 | 比 5-bit 低 |
| int8 g32 | 0.0145 | 0.9233 | 逼近 f32，仍低 0.0002 |

三个反直觉点：

1. **位宽不单调**：5-bit 0.9254 > f32 0.9235，6-bit 0.9232、8-bit 0.9233 反而低。
   量化噪声在 5-bit 这档刚好起了正则化作用；不是「位数越多越好」。
2. **group 不单调**：5-bit 从 g16 到 g64 一路涨（0.9213→0.9254→0.9259），
   g128 又掉到 0.9248。g64 是 128 对上的峰值，不是理论最优；全量上 g128
   （0.8292）反而高于 g64（0.8284）、g32 低于 f32（0.8269），见 §3.16。
3. **GPTQ 的低 `rel_l2` 不等于高 STS**：int5 GPTQ 把 `rel_l2` 从 0.1209 压到 0.0729，
   Pearson 反从 0.9254 掉到 0.9223；int4 GPTQ 128 对只涨 0.0023，全量只涨 0.00007，
   仍低 f32 0.00128。和 §3.15 的侧车非单调是同一件事：权重误差与任务指标不同向。

### 3.16.2 逐层 f32 回退：三值吃末层，4-bit 不吃

三值主干（g32）逐层把一层换回 f32，128 对读数：

| 回退层 | 无 | 0 | 1 | 2 | 3 | 4 | 5（末层） |
|---|---:|---:|---:|---:|---:|---:|---:|
| Pearson | 0.7893 | 0.8232 | 0.8047 | 0.8178 | 0.8036 | 0.7938 | **0.9023** |

末层 f32 一把拉回 +0.113，远大于其它层；代价是总字节 9.29 MB（21.9% f32）。
但同样 9.29 MB 的 int4 全层是 0.9191，已经高于它，所以逐层 f32 只适合做敏感度诊断，
不值得作为存储方案。int4 叠加末层 f32（0.9203）、叠加 0+5（0.9192）、叠加 2+5（0.9218）
也都没有跑赢「全 4-bit + 5-bit 的 group 扫描」，更没跑赢 5-bit g64。

### 3.16.3 12 层容量先导：4-bit L12 ≈ 6 层 f32

既然 6 层 f32 是基线，另一个方向是加容量而不是加位数。用 `TRIT_LAYERS=12` 加载
`all-MiniLM-L12-v2`，128 对：

| 模型 / 格式 | 线性权重 B | 占 6 层 f32 | Pearson |
|---|---:|---:|---:|
| L6 f32 | 42,467,328 | 100% | 0.923497 |
| L12 f32 | 84,934,656 | 200% | 0.927599 |
| L12 int4 g32 | 10,616,832 | 25.0% | 0.923549 |
| L12 int5 g32 | 13,271,040 | 31.25% | 0.925178 |
| L6 int5 g64 | 6,635,520 | 15.6% | 0.925900 |

L12 int4 用 1/4 的线性字节，128 对上和 L6 f32 打平（0.923549 vs 0.923497）；
L12 int5 高 0.0017。**这是目前最像「真正突破」的方向**：容量换精度，不依赖 5-bit
的噪声运气。但它还只有 128 对先导，没有全量；L12 f32 自身也只比 L6 f32 高 0.0041，
全量很可能被量化误差抹平，必须跑 1379 对才能下结论。

### 3.16.4 归类与边界

- **`[B]` 二进制硬件**：int3/4/5/6/8 是标准二进制对称量化，per-group f32 scale；
  不是三值原生红利。二进制基线是同字节的 int4/rank16，不是未压缩 f32。
- **没有 QAT、AdaRound、学习式 scale**；int4 GPTQ 只做了 OBS 误差补偿，没有调 scale。
- 5-bit 的「越过」是同一全量、两个 group（g64/g128）；两档都由 128 对先导选出。
  要把它变成可发表结论，需要 1251 对留出集、配对显著性检验和多 group 复现。
- 12 层只有先导；GPTQ Hessian 只有 6 层数据，L12 的 int5 没走 GPTQ。
- L12 先导复用 L6 导出的第 0 层输入（两模型共享同一套 embedding/tokenizer），没有单独
  导出 L12 embedding；严格复验要重导 L12 的第 0 层输入。
- 所有字节仍是估算：没有打包、没有容器/加载路径，scale 按 f32 记。

## 4. 未证栏

- 基础对齐与逐层快照只有 8 句、`8×64` token；STS 读数另有 128 对与 1379 对两档，但都不是
  训练/检索任务。
- 只三值化线性权重；LN/GELU/softmax/残差保持 f32。全三值激活已做 `ternary-all`，
   cosine 只有 0.03（见 §3.5）；没有 QAT、学习式 scale；逐层混合精度只测了「整层保 f32」
  （§3.16.2），没有测层内混合位宽。
- absmean / GPTQ / per-group / 均匀 int3–8 / SVD 残差侧车都测过；侧车和均匀位宽都只在
  `examples/bert6` 的前向里验证，
  **没有进 WSP/TTX 容器，也没有导出/加载路径**，因此上面的字节数是估算不是落地格式。
- 没有任务头（检索/分类），`cosine` 与 Pearson 不能直接换算成任务掉点。
- GELU 用 erf 近似；f32 对齐 6e-7，但严格逐位复现仍需与 PyTorch 用同一近似。
- 只从第 0 层输入开始；embedding/tokenizer 不在 Rust 侧。

## 5. 接口

- `examples/export_bert6.py`：导出 input/mask/reference/layer0..5；
- `crates/trit-wspace/src/activations.rs`：`read_matrix`（`WSPACT1`）/
  `read_mask`（`WSPMSK1`）；
- `crates/trit-wspace/src/residual.rs`：`LowRankResidual`（`R = W − s·T` 的 top-r SVD，
  `build` / `set_rank` / `projections` / `correction`），`tests/residual.rs` 守满秩重构、
  截断单调与宽/高两种取向；
- `examples/bert6/`：`main.rs` 入口；`run/main_flow.rs` 主流程、`run/modes.rs` 激活模式、
  `run/sidecar_svd.rs` 侧车 rank 扫描、`run/weight_format.rs` 字节账、`model/` 前向与加载、
  `linear/` f32 / 三值 / 均匀位宽线性层；输出 `f32_vs_reference` /
  `real_ternary_vs_f32` / `random_ternary_vs_f32` / `output_stage` / `sidecar_svd` /
  `weight_format` 统计。`tests/gptq_bits.rs` 守均匀位宽 GPTQ 的码值域与 H 误差下降。
- CLI：`--activation-mode f32 --gptq-group 32 --sidecar-svd 0,16,32,64,128`。
- 实验环境变量：`TRIT_WEIGHT_BITS=2..8`（2 = 三值，3..8 = 均匀对称）、
  `TRIT_UNIFORM_GPTQ=1`（均匀位宽走 GPTQ，需数据目录有 `gptq/`）、
  `TRIT_F32_LAYERS=0,5`（整层保 f32）、`TRIT_LAYERS=6|12`（模型层数）。
