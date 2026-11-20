# bitnet.cpp 三元格式互操作：规格、设计与判据（2026-11-20）

> 计划与规格，**不是实现**。本文件只落 M0：外部格式规格、落点设计、可复算判据。
> 实现后按 [STATUS.md](../positioning/STATUS.md) 的四个状态词改写进度；负结果进
> [LESSONS.md](../results/LESSONS.md)。
>
> 定位：让 `trit-export` 能**无损**读写 bitnet.cpp 的 I2_S / TL1 / TL2，
> 与外部三元生态对齐；不跑 llama.cpp，不做推理内核。

## 0. 结论（先给判断）

三进制权重的外部事实标准已经收敛到两条：**I2_S**（2 bit/trit + per-tensor scale）与
**TL1/TL2**（查表索引；表不入盘，运行时由权重生成）。仓库有 GGUF 三值容器与
5 trits/byte 线格式，但**没有这两个格式的读写**，也没有与外部实现的逐字节对拍。

先做互操作的理由不是"多支持一个格式"，而是：

1. **可被外部验证**：格式是公开规格，读写对不对可以逐字节判，不依赖任何外部运行时；
2. **成本最低**：只动 `trit-export` 的编解码层，不碰训练与 GPU 内核；
3. **为后续铺路**：TL 的表构造与 `trit-gpu` 的 LUT / popc 路线共用同一套数学；
   I2_S 的 scale 语义也会暴露"per-tensor vs per-row"的口径差。

判据一句话：**往返逐位 + 规格向量逐字节 + TL 表 27/4 组合穷举**；三条都过才算 M1/M2 完成。

## 1. 外部规格（2026-11-20 核验）

### 1.1 论文与实现

| 对象 | 标识 | 与本计划相关的事实 |
|---|---|---|
| BitNet b1.58 | arXiv:2402.17764（2024-02） | 权重 `{-1,0,+1}`；absmean 量化；"无乘法"只覆盖 linear |
| bitnet.cpp | arXiv:2502.11880（2025-02） | 两种格式：**TL**（Ternary LUT）与 **I2_S**（Int2 + Scale） |
| 官方实现 | `microsoft/BitNet`，MIT | 格式细节以仓库内文件为准（下两节） |

### 1.2 I2_S 格式

来源：`microsoft/BitNet` 的 `utils/convert-hf-to-gguf-bitnet.py` 的 `quantize_to_i2_s`（2026-11-20 核验）。

- 量化：`q = round(w / scale).clamp(-1, 1)`；**scale 取第一个非零 `|w|`**（阈值 `1e-6`，
  对应权重已经量化成 `±scale/0` 的情形）；离线量化模型直接用其 `weight_scale` 作为
  `override_scale`；全零时取 `1e-5`。
  > 注意口径：论文 BitNet b1.58 的 `1/mean(|w|)` 是**训练期 absmean**，不是 I2_S
  > 文件格式的 scale 计算；两者不能混。
- 三值到 2-bit 的映射：`-1 → 0`、`0 → 1`、`+1 → 2`。
- 分块：**每 128 个值一个 block，打包成 32 字节**（要求元素数是 128 的倍数）。
- 字节内位序：block 内先 `reshape(4, 32)`，第 `j` 组第 `k` 个值放在第 `k` 字节的
  `(3-j)` 个 2-bit 槽：`byte = (c0 << 6) | (c1 << 4) | (c2 << 2) | c3`
  —— `c0..c3` 是**跨步 32** 的四个值（索引 `k`、`32+k`、`64+k`、`96+k`），不是连续 4 个。
- 尾部：`packed_size = n / 4` 字节之后追加 **32 字节尾部**，前 4 字节是 per-tensor
  `f32`（小端），其余 28 字节补 0。
- 非三值权重（norm / embedding 等）保持 float16；I2_S 只用于 linear 投影。

### 1.3 TL1 / TL2 格式

来源：`microsoft/BitNet` 的 `include/bitnet-lut-kernels.h`（`three_lut_ctor` / `two_lut_ctor`）。

- **权重文件里存的是索引，不是表**：TL1 是 1-bit 索引（ARM 路径），TL2 是 2-bit 索引（x86 路径）。
- **表由运行时按权重现场生成**，不落盘：
  - `three_lut_ctor`：每 **24 个三值权重**生成 **256 字节表**（8 个 32 字节块）；
    每 3 个权重占一个字节索引（0..26），表项 = `b0*q0 + b1*q1 + b2*q2`，`q ∈ {-1,0,1}`；
  - `two_lut_ctor`：每 **16 个二值权重**生成 256 字节表；每 2 个权重一个字节索引（0..3），
    表项 = `b0*q0 + b1*q1`。
- `lut_scales` 是 per-tensor float32。

> 索引的 byte 布局由 Python 参考实现写死：TL1 见 `preprocess_weights_tl1` / `process_tl1`
> （M2 逐字节复刻并对拍），TL2 见 `preprocess_weights_tl2`（M2b）。

## 2. 仓库现状与差距

| 现有件 | 现状 | 与互操作的差距 |
|---|---|---|
| `trit-export` | GGUF v3 最小子集 + 自定义 `TYPE_TRIT = 100`；5 trits/byte | 没有 I2_S / TL1 / TL2 的读写与 type id 映射 |
| `trit-codec` | 5 trits/byte（canonical padding） | 与 I2_S 的 2 bit/trit 不是同一种线格式（0.2 B vs 0.25 B / 权重） |
| `trit-parts` 的 TPM / GGUF 张量 | per-row `scales` | I2_S 是 **per-tensor** scale；两者不能混用 |
| `trit-gpu` 的 LUT/popc 路线 | 3-popc、DP4A | 没有 TL 的表构造；互操作格式可先独立于内核 |

**差距归纳**：① 没有格式读写；② GGUF type id 没有映射；③ scale 语义未对齐；
④ "表不入盘"在数据模型里没有表达。

## 3. 设计

### 3.1 模块与 API（`trit-export::bitnet`）

```text
trit-export::bitnet::i2s
    BLOCK_VALUES = 128, BLOCK_BYTES = 32, TAIL_BYTES = 32
    pack(trits: &[Ternary]) -> Result<Vec<u8>, I2sError>          // 仅 2-bit 块
    unpack(packed: &[u8], count: usize) -> Result<Vec<Ternary>, I2sError>
    quantize_scale(values: &[f32]) -> f32                         // 第一个非零 |w|
    encode_f32(values: &[f32], override_scale: Option<f32>) -> Result<(Vec<u8>, f32), I2sError>
    encode_ternary(trits: &[Ternary], scale: f32) -> Result<Vec<u8>, I2sError> // 块 + 32 B 尾
    decode(bytes: &[u8], count: usize) -> Result<(Vec<Ternary>, f32), I2sError>

trit-export::bitnet::tl
    three_index(trits: [Ternary; 3]) -> usize                    // 9*a + 3*b + c
    two_index(bits: [bool; 2]) -> usize                          // 2*a + b
    build_three_table(weights: [f32; 3], scale: f32) -> [i8; 27]
    build_two_table(weights: [f32; 2], scale: f32) -> [i8; 4]

trit-export::bitnet::tl1
    Tile { m, k, bm, bk, bmm }                                   // kernel_config.ini 的一节
    tl1_encode(trits: &[Ternary], tile: Tile) -> Result<Vec<u8>, TlError>   // M*K/4 字节
    tl1_decode(bytes: &[u8], tile: Tile) -> Result<Vec<Ternary>, TlError>
```

- 表：`build_*_table` 只算“组合 → 线性组合值”，不对 kernel 的寄存器布局负责；
  索引的 byte 布局由 `tl1` 按 `preprocess_weights_tl1` 的 tile 重排逐字节复刻。

- 输入/输出沿用 `Tensor` 的 `trits`；`scale` 作为**格式级**字段显式传递，
  不写进现有 `Tensor.scales`（那是 per-row）。
- TL 的 `encode` 只写索引；`decode` 只重建三值，**不重建表**；
  表由 `build_*_table` 单独提供，供 J4/J5 对拍。

### 3.2 数据流

```text
Ternary 张量 ──I2_S encode──▶ 32 B / 128 值 + f32 scale ──GGUF──▶ 外部
Ternary 张量 ──TL encode───▶ 1/2-bit 索引 ──GGUF──▶ 外部（表运行时生成）
```

### 3.3 与现有格式的关系

- **不改** `trit-codec` 的主线格式（5 trits/byte 仍是仓库自用格式）。
- `trit-export` 新增 I2_S / TL1 / TL2 的读写分支；`TYPE_TRIT = 100` 不动。
- 新增的 GGUF type id 需要先查上游 enum，避免与 `100` 冲突（M2 时定）。

## 4. 判据（可复算）

| # | 判据 | 复算方式 | 通过条件 |
|---|---|---|---|
| J1 | I2_S 往返无损 | `encode_ternary` → `decode`，128 / 256 值；129 值必须 `Err` | 逐元素相等；129 报对齐错误 |
| J2 | I2_S 位序 | 手工向量：块内 `k=0` 的四个跨步值 `-1/0/+1/-1` | 第 0 字节 == `0x18`（`0<<6|1<<4|2<<2|0`） |
| J3 | scale 口径 | `quantize_scale(&[0.0, 0.5, -0.25])`；全零；override | 分别 == `0.5`、`1e-5`、override 值 |
| J4 | TL 三值表 | 穷举 `q0,q1,q2 ∈ {-1,0,1}`（27 组） | 表项 == `b0 q0 + b1 q1 + b2 q2` |
| J5 | TL 二值表 | 穷举 `q0,q1 ∈ {-1,1}`（4 组） | 表项 == `b0 q0 + b1 q1` |
| J6 | 体积账 | 1M 三值 | I2_S = 0.25 B/权重；TL2 = 0.25 B/权重 + 表；5 trits/byte = 0.2 B/权重 |
| J7 | 真实 GGUF（可选） | 读 BitNet 2B4T 的 I2_S 张量 | 与 safetensors 同张量逐元素一致 |
| J8 | TL1 索引布局 | `tl1_encode` 与 Python `preprocess_weights_tl1` 同参数输出比 3 组夹具，再 `tl1_decode` 回三值 | 3 组逐字节全等且往返无损 |

J1–J6、J8 全部只依赖本仓代码，不需要外部运行时；J7 需要下载公开权重（MIT），
M3 再定是否纳入常规门。

## 5. 里程碑

- **M0（本轮）**：本文档 + 规格测试向量（文本夹具；不写实现）。
- **M1**：I2_S 读写 + J1–J3；新增 `tests/i2s_contract.rs`。
- **M2（本轮）**：TL1 索引读写 + `build_*_table` + J4–J5、J8；新增 `tests/tl_contract.rs`、
  `tests/tl1_contract.rs` 与 `tests/fixtures/tl1_golden.txt`。
- **M2b**：TL2 索引读写（three/two 双段 + sign 平面 + 特定 `(M,K)` 的 padding）。
- **M3**：J6–J7；读数与负结果写进 `docs/results/` 与 `LESSONS.md`。

## 6. 风险与未决问题

1. **scale 语义冲突**：I2_S / TL 是 per-tensor scale，仓库 GGUF 张量是 per-row `scales`。
   M1 必须显式区分，不能把两者相加或替换。
2. **TL 索引位序**：TL1 的布局由 Python 参考实现的 `preprocess_weights_tl1` / `process_tl1`
   写死；M2 用同参数夹具逐字节对拍（J8），不从 kernel 反推。TL2 的 three/two 双段与
   padding 是对特定 `(M,K)` 的常量，M2b 需参数化后再对拍。
3. **GGUF type id**：需要查上游 `ggml_type` 枚举；冲突时另起互操作 ID 空间，不复用 `100`。
4. **"表不入盘"**：TL 的文件体积不含表；J6 的体积账必须把表单独列，不能混进权重。
5. **许可**：BitNet 代码与权重都是 MIT；M3 若下载权重，只做读取对拍，不再分发。

## 7. 不做（本计划的边界）

- 不跑 llama.cpp / bitnet.cpp，不做推理性能对比（那是 P3 的 GPU LUT 路线）。
- 不改 `trit-codec` 的主格式，不发布新的格式标准。
- 不把 TL 表写进权重文件；不把 per-tensor scale 混进 per-row 通道。
