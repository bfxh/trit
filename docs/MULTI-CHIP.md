# 多芯片后端设计：x86 / ARM / 移动 SoC（Android · iOS · Web）

> 状态：**设计稿（2026-10-29）**，尚未实现。本文是后续开发的依据；所有"预计"都标了判据，
> 凡是没实测的数字一律写"未证"。
> 关联：`crates/trit-parts/README.md`（现状与读数）、`docs/TERNARY-BOUNDARIES.md`（A/B/C/D 归类规则）。

---

## 0. 一句话

把「三值权重（−1/0/+1 × per-channel scale）+ per-channel affine」的推理栈，从只在 RTX 4060 Ti + wgpu
上跑，扩成**一条代码、多后端**：x86（AVX2/AVX512）、ARM64（NEON / i8mm / SVE）、
移动 GPU（Vulkan / Metal，经 wgpu）、移动 NPU（Qualcomm HTP / Apple ANE），
并且**每个后端都有与 f32 参考逐元素对拍的门**。

不做的事（明确划界）：
- **不在手机上训练**。训练仍在 x86 + 独立 GPU；手机只做推理（可选：小规模激活校准）。
- **不追大模型**。本仓的模型是部件平面头（13 层 UNet，384²，约 110 万三值权重），
  目标是"手机单帧推理 ≤ 数百毫秒"这一量级，不是 LLM 吞吐。
- **不引入 Python 推理路径**（死命令）。导模型给 NPU 时，导出器用 Rust 写；
  若某厂商只收 Python 工具链的产物，则记负结果并停在该平台。

---

## 1. 现状盘点（可复核）

| 项 | 现状 | 证据 |
|---|---|---|
| 模型 | 13 层 UNet：3→32→32 / pool / 32→64→64 / pool / 64→128→128 / ConvT / 64→32 / ConvT / 32→16 | `crates/trit-parts/src/net.rs` |
| 参数量 | 三值权重约 1.1 M 个 + per-channel scale（每层 cout 个）+ affine γ/β（每层 cout 个） | `Layer` |
| 推理后端 | wgpu 30（Vulkan/DX12/Metal/GL），WGSL 计算内核 | `src/gpu.rs`、`src/gpu_bwd.rs`、`src/shaders*.wgsl` |
| 训读 | forward 16 ms / backward 107 ms（384²，4060 Ti，release） | `parts-bench` |
| 训练吞吐 | 0.19 s/样本（独占 GPU）；共享 GPU 时 0.25–0.57 s | `parts_train` 日志 |
| 精度（留出 300 帧） | id 0.6913 / IoU 0.5399（清单约束 0.6924 / 0.5408） | LEDGER A67 |
| 对齐门 | `parts-parity`（f32 argmax 737280/737280，三值 vs PyTorch 三值 0.999688）、`parts-gradcheck` 17 项 | 仓库 README |

**关键观察**：现在的"三值"只体现在**权重**上；激活是 f32，计算是 f32 乘加。
所以当前实现吃到的是**带宽红利**（权重 1.58 bit/权重，等效压缩 20×），
没吃到**指令红利**（没有把乘加换成整数点积/查表）。多芯片设计的核心，就是把这两条都拿下。

---

## 2. 目标平台矩阵

| 平台 | CPU 后端 | GPU 后端 | NPU 后端 | 本次目标 |
|---|---|---|---|---|
| Windows / Linux x86_64 | AVX2 / AVX512-VNNI | wgpu（Vulkan / D3D12） | — | **必须**（开发与对拍基准） |
| macOS（Apple Silicon） | NEON / i8mm | wgpu（Metal） | Core ML（ANE） | 尽量（CI 可覆盖） |
| Android（arm64） | NEON / i8mm（可选 SVE） | wgpu（Vulkan） | QNN HTP / NNAPI | **必须**（"手机能用"的主体） |
| iOS / iPadOS | NEON / i8mm | wgpu（Metal） | Core ML（ANE） | 尽量 |
| 浏览器（wasm32） | wasm SIMD | WebGPU | — | 加分项 |

依据（2026-10 现状）：
- **WebGPU 已全浏览器铺开**：Chrome/Edge/Firefox/Safari 均默认支持；Safari 26 起覆盖 iOS 26 / iPadOS 26 / macOS 26；
  第三方统计口径（2026-10-17）约 **89.5% 覆盖率**（85.8% 完整 + 3.66% 部分）。
  ⇒ 移动端"最省事的跨芯片 GPU 路径"就是 wgpu → Vulkan（Android）/ Metal（iOS）。
  Firefox Android 仍在推进，不作为必选项。
- **移动 NPU 只吃低比特**：Qualcomm QAIRT 的 HTP 支持 **INT4 权重编码**（部分算子）；
  官方建议权重对称量化、激活优先 8 bit。Core ML 支持 **INT4/INT8 权重 + INT8 激活**。
  ⇒ 三值权重在 NPU 上通常要先"还原成 INT4"（−1/0/+1 ⊂ INT4），这是可接受的映射。

---

## 3. 技术盘点（论文与工程，带结论）

### 3.1 三值训练（我们已经吃到的部分）

- **TWN / TTQ**（2016）：三值权重量化的开山工作，阈值 + 尺度（scale）两参数。
- **LSQ**（2020）：把 **scale 变成可学习参数**并用梯度缩放稳定训练 —— 我们的 `scale` 就是 LSQ 风格。
- **BitNet b1.58**（2024）：权重严格取 {−1,0,+1}（≈1.58 bit），证明同规模下可与 fp16 持平；
  2025 年的 BitNet b1.58 2B4T 是首个大规模原生 1-bit 开源模型。
- **结论（对我们）**：三值权重 + 每通道 scale + affine 是正确路线，已在本仓复现（见 A65：补 affine 后 IoU 超 f32 基线）。

### 3.2 三值推理内核（我们还没吃到的部分）

两条主流路线，**都不是逐元素乘加**：

**(a) 整数点积（sub-byte packed + int8 激活）**
- `bitnet.cpp` 的 **I2_S** 内核：权重 2-bit 打包，激活 int8，用整数点积指令（ARM `sdot`/i8mm、x86 AVX2/AVX512）。
- 官方读数：CPU 上相对全精度 **6.17×（x86）/ 5.07×（ARM）**（BitNet 官方仓 README 口径）。

**(b) 查表（LUT / mpGEMM）**
- **T-MAC**（arXiv 2407.00088）：把低比特权重当"索引"，预先把激活组合算成表，
  推理时只做**查表 + 加法**，直接支持 mpGEMM 且不需要反量化；
  相对 llama.cpp 的 SOTA CPU 实现，**最高 6.6×、平均 3.6×**。
- `bitnet.cpp` 的 **TL1 / TL2**：TL1 把**2 个三值权重**打包成 **4-bit 索引**（3×3=9 种组合），
  TL2 用 **1-bit 符号 + 4-bit 索引**表示 **3 个**权重；面向 AVX2 / NEON，激活 int8，宣称**无损**。
- **结论（对我们）**：卷积核是 3×3×cin 的短点积（cin=32/64/128），非常适合 **LUT（每次查 9 种组合）**
  或 **int8 点积**；手机 ARM 上优先 LUT（TL1 式），x86 上优先 AVX512-VNNI 的 int8 点积 + 可选 LUT。

### 3.3 ARM 生态的现成加速库

- **KleidiAI**：Arm 官方的 int4/int8 matmul 微内核库（NEON MLA、`sdot`、i8mm、SME2），
  已被 ONNX Runtime 等框架接入。
- **结论**：不做"从零写 NEON 内核"的宗教选择；先写**自研最小内核**保证可控与可对拍，
  若被 KleidiAI 明显超过，再评估 FFI 接入（代价：多一个 C 依赖，需许可与构建链评估）。

### 3.4 移动端执行环境

| 环境 | 能跑什么 | 关键约束 |
|---|---|---|
| Android（arm64） | wgpu→Vulkan；CPU NEON；QNN/NNAPI | 机型碎片化；Vulkan 驱动差异；NPU 需厂商 SDK（QNN 仅高通） |
| iOS | wgpu→Metal；CPU NEON；Core ML | 只能静态链接 + XCFramework；Core ML 需 `.mlpackage` |
| Web | WebGPU；wasm SIMD | 显存/工作组上限比桌面小；首帧编译着色器有延迟 |

**统一的软约束**（三端都要遵守）：
- 单次 dispatch 的工作组数上限保守取 **65535/维**（现在已按 `256 × ceil(n/16384)` 分派，天然满足）；
- 存储缓冲绑定上限：手机常见 **128 MB**（我们最大的缓冲 ~40 MB，安全）；
- 不用 `float16` 作为默认（移动端 f16 支持参差），先 f32 + int8 两条腿。

---

## 4. 架构设计

### 4.1 三层

```
┌───────────────────────────────────────────────┐
│ L2 调度层  DeviceCaps → 选后端、选 batch、选精度  │
├───────────────────────────────────────────────┤
│ L1 内核层  Backend trait 的若干实现              │
│   RefCpu(f32) · Wgpu · NeonInt8 · LutArm/X86 · Npu│
├───────────────────────────────────────────────┤
│ L0 表示层  TriPack(2bit 双导轨) + scale + affine  │
└───────────────────────────────────────────────┘
```

**L0 表示层（唯一真源）**
- `TriPack`：每个 trit 2 bit，双导轨 `pos/neg`（`11` 非法，视为 0）；32 trit/`u32`。
  现有 `q: Vec<i32>` 是「开发期方便」的形态，多芯片阶段要收敛到 `TriPack`，
  否则带宽红利会在打包/解包上被吃掉。
- 每层附带：`scale: Vec<f32>`（per-out-channel）、`gamma/beta: Vec<f32>`（affine）、`bias: Vec<f32>`。
- 序列化：`TPNT3`（训练存档）之外新增 **`TPM1` 部署包**（只读、无 Adam），供手机端加载。

**L1 内核层（`Backend` trait，草案）**

```rust
pub struct Caps {
    pub name: &'static str,
    pub max_batch: u32,
    pub max_buffer: u64,
    pub supports_int8: bool,
    pub supports_lut: bool,
    pub supports_f16: bool,
}

pub trait Backend {
    fn caps(&self) -> Caps;
    fn upload(&mut self, m: &TriModel) -> Result<(), Error>;
    /// 卷积（NHWC，pad=kk/2，可选 ReLU 由 affine 处理）
    fn conv(&mut self, x: &Tensor, w: LayerId, h: u32, w_: u32) -> Result<Tensor, Error>;
    fn convt(&mut self, x: &Tensor, w: LayerId, h: u32, w_: u32) -> Result<Tensor, Error>;
    fn affine(&mut self, v: &Tensor, l: LayerId, relu: bool) -> Result<Tensor, Error>;
    fn pool(&mut self, x: &Tensor, h: u32, w: u32, c: u32) -> Result<Tensor, Error>;
    fn cat(&mut self, a: &Tensor, b: &Tensor, hw: u32, ca: u32, cb: u32) -> Result<Tensor, Error>;
    fn read(&mut self, t: &Tensor) -> Result<Vec<f32>, Error>;
}
```
> 现有 `Net`（训练）与 `forward.rs`（对拍）都收敛到这套接口之上；训练专用的反向接口暂时不进 trait
> （手机端不做训练），避免把 trait 撑成上帝对象。

**L2 调度层**
```rust
fn pick(caps_list: &[Caps], need: Need) -> BackendKind
```
规则（先写死，后续再谈自适应）：
1. 有 NPU 且模型能过 NPU 的算子集 → NPU（最快、最省电）；
2. 否则有 GPU（wgpu 可用）→ wgpu；
3. 否则有 int8/LUT CPU → 对应 CPU 后端；
4. 兜底 → `RefCpu`（f32，永远正确，作为对拍基准）。

### 4.2 各后端实现计划

| 后端 | 语言/宿主 | 内核要点 | 预计相对 f32 加速 | 判据 |
|---|---|---|---|---|
| `RefCpu` | Rust 纯标量 | 逐元素 f32（现有 `backward.rs` 前向即可复用） | 1× | 与 f32 逐元素 bit 级一致 |
| `Wgpu` | 现有 | WGSL vec4（已做：forward 16 ms） | 桌面 ~30× | 现有 parity 门 |
| `NeonInt8` | `core::arch::aarch64` | 激活 int8、权重解包到 int8，`vdotq_s32`（无 i8mm 时）/ `smmla`（i8mm） | 未证，目标 ≥2× vs RefCpu | 与 f32 argmax ≥0.995 |
| `LutArm` | 同上 + `vtbl` | 每 2 个三值权重 → 4-bit 索引，9 项 LUT（3 项激活组合 ×3 权重组合） | 未证，目标 ≥1.5× vs NeonInt8 | 同上 |
| `LutX86` | `core::arch::x86_64` | `pshufb` 做 16 路查表 + AVX2/VNNI | 未证 | 同上 |
| `NpuQnn` | FFI（QNN C API） | 导出 INT4 权重（三值映射到 −1/0/+1）+ INT8 激活 | 未证（取决于机型） | 精度门 + 真机耗时 |
| `NpuCoreMl` | `objc2-core-ml` | `.mlpackage`（INT4/8 量化） | 未证 | 同上 |

**为什么不是先做 NPU**：NPU 的收益最大但可移植性最差（QNN 只覆盖高通、Core ML 只覆盖苹果），
且需要"算子集裁剪 + 导出器"。先把 **CPU int8/LUT** 打通（一条代码覆盖所有手机），
NPU 作为 M4 的增益项。

### 4.3 量化细节（int8 激活路径）

当前激活是 f32。int8 后端需要 **per-tensor 激活量化**：
- 校准：用 32–64 帧训练集样本，统计每层激活的 max-abs（对称量化，zero-point=0）；
- 量化点放在 **affine 之后**（`y = relu(γv+β)` 的输出），避免 ReLU 前分布偏斜；
- 误差预算：每层 int8 对称量化理论 SNR ≈ 6·b − 4.8 ≈ 43 dB；14 层串起来后按卷积网络经验
  **留出 argmax 一致率 ≥0.995 的门**（比逐元素误差更贴近任务）。
- **风险**：valid 通道（sigmoid）对量化更敏感；若一致率不达标，valid 分支单独保留 f32。

### 4.4 内存与布局

- 激活统一 **NHWC**（现有约定，不改）。
- `TriPack` 权重按 **输出通道连续** 打包（`[k][ic][oc]` 的 oc 方向每 16 个 trit 一个 `u32`），
  这样 int8/LUT 内核沿 `oc` 取数时是顺序读。
- 每层附 `scale/gamma/beta/bias` 四个 `f32` 数组；int8 路径把它们折叠成 (int32 乘数, 右移位数)，
  用整数乘加完成反量化（避免每元素浮点）。

---

## 5. 一致性与质量门（最重要）

**每个后端都必须过同一组门**，否则不给"可用"标签：

| 门 | 判据 | 复算方式 |
|---|---|---|
| G1 逐元素 | 与 `RefCpu`（f32，同权重）比 `max|Δlogit| ≤ 5e-2`（int8 后）；LUT 与 int8 之间 ≤ 1e-3 | `parts-parity --backend <b>` |
| G2 任务 | 平面 argmax 一致率 **≥0.995**；`valid` 一致率 ≥0.99 | 同上 |
| G3 端到端 | 留出 300 帧 id / IoU 与 wgpu 版差 ≤0.02 | `parts-train --eval --backend <b>` |
| G4 性能 | 记录单帧耗时（P50/P95），回归 >15% 判红 | `parts-bench --backend <b>` |
| G5 内存 | 峰值驻留 ≤ 设备上限的 60%（手机 8 GB 机型） | 真机采样 |

G1/G2 是硬门；G3 用来防"逐元素过了但语义崩"；G4 只做趋势记录（手机机型差异太大，不设绝对阈值）。

---

## 6. 构建、打包与 CI

| 目标 | 产物 | 工具链 |
|---|---|---|
| Windows/Linux x86_64 | `parts-cli`（单文件） | `cargo build --release` |
| Android arm64 | `.so` + JNI 壳 | `cargo ndk -t arm64-v8a` |
| iOS | `.xcframework`（device + simulator） | `cargo build --target aarch64-apple-ios` + `xcodebuild -create-xcframework` |
| WASM | `.wasm` + 最小 JS 胶水 | `wasm-bindgen`（唯一可能引入的 JS 依赖） |

Cargo feature 划分：`backend-wgpu`（默认开）、`backend-neon`、`backend-lut`、`backend-qnn`、`backend-coreml`。
**默认构建不拉任何移动 SDK**，保证 CI 的 x86 job 不变慢。

CI 矩阵（GitHub Actions）：
- `ubuntu-latest`（x86_64）：fmt/clippy/test + `RefCpu` 对拍；
- `macos-latest`（arm64）：NEON/LUT 编译 + `RefCpu↔NEON` 对拍（CPU 后端可在 CI 真跑）；
- `windows-latest`：wgpu 构建（无 GPU 时跳过真机门，只编译）；
- Android/iOS：**只做交叉编译**（CI 无真机）；真机读数由本地脚本记录进 `docs/BENCH.md`。

---

## 7. 分阶段计划（每阶段带判据）

**M0 · 抽象层落地（1–2 天）**
- 抽出 `Backend` trait；把现有 wgpu 实现包成 `Wgpu` 后端；新增 `RefCpu`（f32 标量）。
- 新二进制 `parts-cli infer --backend refcpu|wgpu --dir <夹具>`。
- **判据**：`refcpu` 与 `wgpu` 在同一夹具上 G1 `≤2e-4`、G2 `argmax=1.000000`（现有 parity 已证明）。

**M1 · ARM int8 点积（3–5 天）**
- `TriPack` 打包/解包 + `NeonInt8`（`vdotq_s32`；有 i8mm 走 `smmla`）。
- **判据**：G2 ≥0.995；GPU 侧同机对比 G4 有明显收益（目标 ≥2× vs RefCpu，实测为准）。

**M2 · LUT 三值内核（3–5 天）**
- ARM `vtbl` / x86 `pshufb`：每 2 权重 4-bit 索引，9 项 LUT。
- **判据**：相对 M1 ≥1.3× 且 G2 不变；否则记负结果（很可能在 cin 大时 int8 点积更优）。

**M3 · 手机壳（3–5 天）**
- Android：`cargo ndk` + 最小 JNI 入口，读 `.tpm` 模型 + 一张 PNG，输出 16 通道 logits 的 argmax 图。
- iOS：静态库 + 一个最小 Swift 调用样例。
- **判据**：单帧端到端耗时 P50 记录在案（真机型号写清楚），温度/功耗不作为门。

**M4 · NPU（可选，按机型收益决定）**
- 导出器（Rust 写）→ ONNX QDQ / Core ML mlpackage；三值映射到 INT4。
- **判据**：G3 达标且比 M2 快 ≥1.5×；不达标就停在 M2，并把机型/原因写进文档。

**贯穿**：每个后端进 `parts-parity`/`parts-gradcheck` 的同构门；台账（LEDGER）一项一判据。

---

## 8. 风险与未证清单

| 风险 | 影响 | 缓解 |
|---|---|---|
| Vulkan 驱动碎片化（Android 低端机型） | wgpu 后端在部分机型崩/慢 | 加 `RefCpu`/`NeonInt8` 兜底；记录机型黑名单 |
| int8 激活量化损伤 valid 通道 | 任务指标掉点 | valid 分支单独 f32（已验证可分离） |
| LUT 收益被高估 | M2 可能不如 M1 | 先做 M1 再判 M2，负结果照实记 |
| QNN/Core ML 只收特定工具链产物 | "全 Rust"被打破 | 导出器自写；若必须 Python 工具链则记负结果并止步 |
| 手机热降频 | 长时推理性能漂移 | 只报 P50/P95 短序列，不报持续吞吐 |
| 我们的模型是卷积不是 GEMM | 论文的 LUT 收益基于矩阵乘 | M2 先做 3×3 小核 LUT 实测，不照搬论文结论 |

**未证（必须实测才算数）**：
- int8 激活量化对本任务的实际掉点（估计 1–3% id，未证）；
- LUT 在 cin=32/64/128 的 3×3 卷积上是否真比 int8 点积快（论文是 1×N 的 GEMV）；
- 手机 Vulkan 上 384² 单帧时延（估计 100–400 ms，未证）；
- QNN/Core ML 对 ConvTranspose/小通道卷积的支持度（需查算子表）。

---

## 9. 参考文献

1. Li, Liu, **Ternary Weight Networks**, 2016. arXiv:1605.04711
2. Zhu et al., **Trained Ternary Quantization**, 2016. arXiv:1612.01064
3. Esser et al., **Learned Step Size Quantization (LSQ)**, 2020. arXiv:1902.08153
4. Ma et al., **The Era of 1-bit LLMs: All LLMs are in 1.58 Bits (BitNet b1.58)**, 2024. arXiv:2402.17764
5. Wang et al., **BitNet: Scaling 1-bit Transformers**, 2023. arXiv:2310.11453
6. Wang et al., **bitnet.cpp: Efficient Edge Inference for Ternary LLMs**, 2025. arXiv:2502.11880
   （TL1：2 权重→4-bit 索引；TL2：1-bit 符号 + 4-bit 索引表示 3 权重；AVX2/NEON，int8 激活）
7. Wei et al., **T-MAC: CPU Renaissance via Table Lookup for Low-Bit LLM Deployment on Edge**, 2024. arXiv:2407.00088
   （LUT mpGEMM；相对 llama.cpp 最高 6.6×、平均 3.6×）
8. Microsoft, **bitnet.cpp / BitNet 官方仓**：CPU 6.17×(x86) / 5.07×(ARM) vs 全精度
9. Arm, **KleidiAI**（int4/int8 微内核：NEON MLA、`sdot`、i8mm、SME2）
10. Qualcomm, **QAIRT/HTP 文档**：支持 INT4 权重编码（部分算子），建议权重对称量化、激活优先 int8
11. Apple, **Core ML Tools 优化概览**：INT4/INT8 权重 + INT8 激活
12. gpuweb **Implementation Status / web.dev 博文**：WebGPU 在 Chrome/Edge/Firefox/Safari 均已发布，
    Safari 26 覆盖 iOS 26 / iPadOS 26 / macOS 26；第三方统计 2026-10 约 89.5% 覆盖率

> 引用只代表"该文/该文档说了什么"；能不能在我们的 13 层卷积上复现，一律以第 5 节的门为准。
