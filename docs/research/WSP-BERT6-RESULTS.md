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
3. **当前实现（absmean LN weight + sign 激活）不支持全三值激活**。要保留方向，
   需要 QAT/学习式 scale、混合精度（LN 输出保 f32）或残差侧车；
4. 随机输入在三种模式下都比真实输入更差（rel_l2 高 25%–34%），方向与 §3 一致。

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
