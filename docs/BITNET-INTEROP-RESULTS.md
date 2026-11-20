# BitNet 互操作结果（J6 体积账 / J7 真实权重）

> 计划与判据：`docs/BITNET-INTEROP-PLAN.md`。日期：2026-11-20。
> 复跑：`cargo test -p trit-export --test volume_account --test i2s_real_weights`；
> 全量 J7 对拍：`python scripts/check_bitnet_i2s_parity.py`（联网，约 9 MB 流量）。

## J6 体积账（1.57 Mi 三值，1024×1536）

由 `crates/trit-export/tests/volume_account.rs` 用真实编码器实测，不是手算表：

| 格式 | 每权重 | 1,572,864 三值实测 | 备注 |
|---|---|---|---|
| I2_S | 0.25 B | 393,248 B | `n/4 + 32`（每张量一份 32 B 尾） |
| TL1 索引 | 0.25 B | 393,216 B | 每字节 4 个三值 |
| TL2 three 段 | 5/24 ≈ 0.208 B | 327,680 B | 每 3 值 5 bit = 4-bit 幅度 + 1-bit 符号 |
| 仓库自用 5 trits/byte | 0.2 B | 314,572 B | `n/5` |

**表单独列（不入盘，运行时生成）**：`three_lut_ctor` 每 24 个三值权重 256 B →
1,572,864 三值对应 **16,777,216 B**（是索引本体的 42.7 倍）；`two_lut_ctor` 每 16 个
二值权重 256 B → 25,165,824 B。这正是「表不入盘」必须成立、且体积账必须把表排除在
权重文件之外的理由。

## J7 真实权重对拍（已通过）

数据源（均为 MIT）：`microsoft/bitnet-b1.58-2B-4T`（safetensors）与
`microsoft/bitnet-b1.58-2B-4T-gguf`（`ggml-model-i2_s.gguf`，1.19 GB）。
只拉需要的两段，不下载全文件。

| 项 | 值 |
|---|---|
| 张量 | GGUF `blk.0.ffn_down.weight` ↔ safetensors `model.layers.0.mlp.down_proj.weight` |
| 形状 | GGUF dims `[6912, 2560]`（ne0 最快变化）↔ safetensors `U8 [640, 6912]` |
| 权重数 | 17,694,720 |
| 结果 | **4,423,680 字节逐字节一致**（I2_S 主体） |
| GGUF 张量占用 | 4,423,712 B = `n/4 + 32` |
| 尾部 f32 scale | `2.163161277770996` |
| safetensors `weight_scale`（BF16） | `2.15625` = bf16(2.163161277770996)，两者自洽 |

### 复算要点（脚本里的三条事实）

1. **safetensors 的 U8 打包**：每字节 4 个三值，**低位先**（`sh = 0,2,4,6`），4 个槽分给
   相距 `rows/4` 的 4 个行块（`W[j*640 + k] = (b[k] >> 2j) & 3`）。按「连续 4 行交错」
   （`W[4k + j]`）重建会在第 54 个 128 值块（一行 54 块）起全部对不上 —— 曾据此误判为
   「块顺序不同」。
2. **I2_S 打包**：矩阵 (2560, 6912) 行主序 → 每 128 值一块 → 块内 `reshape(4, 32)` 交错，
   与 `i2s::pack` 完全一致；块内布局正确但块间不同时，先怀疑行的分组而不是块的排列。
3. **尾部 28 字节在真实 GGUF 里非零**：Python 转换脚本写零，真实文件不是。读侧不能依赖
   「补零」，只有前 4 字节 f32 scale 有意义。

## J7 复跑：两个张量（含 `attn_k`）

`scripts/check_bitnet_i2s_parity.py` 现在同时验 `blk.0.ffn_down.weight` 与
`blk.0.attn_k.weight`：

| 张量 | 形状（GGUF / safetensors） | 结果 |
|---|---|---|
| `blk.0.ffn_down.weight` | `[6912, 2560]` / `U8 [640, 6912]` | 4,423,680 B 全等 |
| `blk.0.attn_k.weight` | `[2560, 640]` / `U8 [160, 2560]` | 409,600 B 全等 |

**更正**：`attn_k` 一开始“首块一致、其后不一致”，我把它归因为 llama.cpp 对
`q_proj`/`k_proj` 的 RoPE 头重排。实测**不需要任何重排**——用与 `ffn_down` 相同的
「4 槽分给相距 `rows/4` 的行块」规则重建后逐字节全等；当初的差异只是因为错用了
「连续 4 行交错」。头重排假设已证伪（对 `n_head ∈ {5,10,20}` 三种 permute 都不如
无重排）。

### 未覆盖

- `token_embd.weight` 是 f16、norm 是 f32，这两类张量不在 I2_S 对拍范围（M4 的
  `read` 也仍以 `TensorType` 拒绝它们）。
- TL2 的 two 段尾（`k % bk != 0`）与真实模型的 `kernel_config.ini` 尚未对齐。

## M4：GGUF 的 I2_S 读写分支

| 项 | 内容 |
|---|---|
| 类型码 | 本仓 `TYPE_TRIT = 100`；上游 `TYPE_I2_S = 36`（实测 `bitnet-b1.58-2B-4T-gguf`） |
| per-tensor 语义 | `scales.len() == 1 && rows > 1` → 写 kind 36、体 = `n/4 + 32`；否则 per-row（kind 100） |
| 读 I2_S | 2-bit → trits；`scales = [尾部 f32]`；`bias` 全 `Unknown` |
| 负例 | 元素数不是 128 的倍数 → `Error::Tensor`；多行张量的 scale 既非 1 也非 `rows` → `Error::Field` |
| 测试 | `tests/gguf_i2s.rs` 4 条（往返、混合类型、体积差 32、负例） |
| 未做 | 真实 GGUF 还有 f32（norm）与 f16（embedding）张量，`read` 仍以 `TensorType` 拒绝整份文件；只支持 2D |
