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
   之前几节「距 f32 差 0.19 / 0.09」的对比全部建立在乐观基线上，对外引用必须换成全量口径。
2. **GPTQ 相对 absmean 的净增益随样本扩大收窄**：+0.0735 → +0.0491 → **+0.0253**。
   方向仍然一致（三次都是正），但量级远小于 64 对的读数。
3. **per-group 在全量上几乎无额外收益**（+0.0020，Spearman +0.0037）：§3.12 的 +0.02~+0.04
   是子集特异现象，不能当普适结论；per-group 的存储/实现成本换不来全量收益。
4. 权重三值化相对 f32 的代价（全量）：absmean **−0.1404**、GPTQ **−0.1151**、
   group=32 **−0.1131**。
5. 全量口径下结论：**权重侧 GPTQ 是有效但收益有限的一刀（+0.025）**；per-group 无效；
   想再往上走需要更大 rank 侧车、QAT 或混合精度，而不是继续调 scale 粒度。

## 4. 未证栏

- 只有 8 句、`8×64` token；不是大规模语料。
- 只三值化线性权重；LN/GELU/softmax/残差保持 f32。全三值激活已做 `ternary-all`，
  cosine 只有 0.03（见 §3.5）；没有 QAT / 学习式 scale / 混合精度。
- 只测 absmean 逐行三值化；没有 GPTQ / per-group / 残差侧车。
- 没有任务头（检索/分类），`cosine 0.47` 不能直接换算成任务掉点。
- GELU 用 erf 近似；f32 对齐 6e-7，但严格逐位复现仍需与 PyTorch 用同一近似。
- 只从第 0 层输入开始；embedding/tokenizer 不在 Rust 侧。

## 5. 接口

- `examples/export_bert6.py`：导出 input/mask/reference/layer0..5；
- `crates/trit-wspace/src/activations.rs`：`read_matrix`（`WSPACT1`）/
  `read_mask`（`WSPMSK1`）；
- `examples/bert6/main.rs` + `model.rs`：6 层 Rust 前向，输出
  `f32_vs_reference` / `real_ternary_vs_f32` / `random_ternary_vs_f32` /
  `output_stage` 统计。
