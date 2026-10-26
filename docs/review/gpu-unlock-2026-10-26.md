# gpu-unlock 与 trit 的兼容性评估（2026-10-26）

对应 Issue #167。本文只回答两件事：**解锁后 trit 的哪些路径真的会变快**，以及**要多少工作才叫"支持"**。
所有 trit 侧数字都来自本仓 README / 实测记录；gpu-unlock 侧标注了来源与未验证项。

## 0. 结论（先看这段）

1. gpu-unlock 解锁的是 **FP16/BF16 张量核 FMA 的发射率**（FP32 累加），**不是 FP32 SIMT 速率**。
   作者在 HN 的原话：*"It's the FP/BF16 FMA to FP32 instructions"*。
2. trit 现有四条 GPU 路径里，**三条都不受益**：
   - 三值双导轨 GEMM（整数 `countOneBits`）——与 FP 发射率无关；
   - f32 SIMT GEMM——不是张量核指令；
   - f16 存储 + f32 累加——仍是 SIMT，只是存储减半。
3. 能吃到红利的只有**张量核 GEMM**这条 trit 还没有的路径。所以 #167 的"兼容"不是开关，而是
   **新增一条张量核后端**。当前支持度：**0/4 条既有路径受益**。
4. 本机（Windows + RTX 4060 Ti）**连解锁都用不了**：4060 Ti 是 AD106（Ada），
   gpu-unlock 的型号表只到 GA102/103/104/106（Ampere）；且它是 Linux 内核模块，
   WSL2 走 Windows 驱动的 `dxgkrnl`，同样不适用。

## 1. gpu-unlock 到底改了什么（外部证据）

| 项 | 内容 | 来源 |
|---|---|---|
| 机制 | pre-driver kmod：initramfs 里早于 `nvidia` 绑定 → 写 speed selector（`SS0 = 0x88888888` 选满速 BF16，4 位 `SS1 = 0x8` IMLA4）→ FLR → 未改动的驱动正常绑定 | 仓库 README |
| 解锁对象 | FP16/BF16 张量核 FMA 发射率（专业卡档位），**不是** FP32 SIMT | 作者 HN 回复 |
| 实测（3090 Ti） | BF16 87.50 → **157.41** TFLOPS（+80%）；FP16 87.46 → **118.39**（+35%）；TF32 42.58 → **82.61**（+94%） | 项目 README / 媒体汇总 |
| 型号 | GA102 / GA103 / GA104 / GA106 消费级 30 系；移动版与专业卡已排除 | 仓库 README |
| 系统 | 仅 Linux（内核模块 + initramfs）；挂起到内存后由 notifier 重新解锁；**休眠不覆盖** | 仓库 README |
| 反例 | OEM RTX 3080 Ti 20GB（GA102-250）解锁"成功"但 BF16 仍是半速 ⇒ **"解锁完成"≠"有效"** | 媒体实测汇总 |

## 2. trit 侧基线（本仓实测，512×4096×512）

| 路径 | 读数 | 备注 |
|---|---|---|
| 三值双导轨 GEMM（`main_tiled2o3` / `main_splitk`） | **0.101–0.103 ps/MAC** | 整数 popc 主导；已到 popc 天花板 79–82% |
| f32 SIMT（`main_tiled4k32`） | **0.298–0.300 ps/MAC** | 换算是 6.71 TFLOP/s；4060 Ti FP32 峰值约 22.1 ⇒ **约 30% 峰值** |
| f16 存储 + f32 累加 | 0.635–0.644 ps/MAC | 同结构 −16%，收益来自带宽 |
| MOD 前向端到端 | 已通（`trit-texture::to_dual_rail` → `forward::matmul_packed`） | 走 naive 内核 |

> f32 只到 30% 峰值，且 README 已把剩余差距定位在**指令混合**（共享读 + 索引 + 累加），
> 不是 FMA 发射率。所以即使 FP32 速率翻倍，这个内核也不会翻倍——这也是"解锁帮不上"的第二重理由。

## 3. 逐路径判定

| 路径 | 解锁后变快？ | 原因 |
|---|---|---|
| 三值 popc GEMM | **否** | 整数指令；项目方口径也明确 INT8 与 decode 不变 |
| f32 SIMT GEMM | **否（≈0）** | 解锁的是张量核 FP16/BF16 发射率，不是 FP32 SIMT |
| f16 存储变体 | **否** | 已吃到的是带宽账；FMA 仍是 SIMT |
| f16 张量核（cooperative matrix） | **可能（未证）** | 消费卡 FP16 也被限速（+35%）；但 trit 没有这条路径 |
| bf16 张量核 | **是，但够不到** | 收益最大（+80%），被 naga 的 bf16 缺失挡住（见 §4） |
| 多卡 / vfio 直通 | 不适用 | trit 没有多卡调度 |

## 4. 技术卡点：为什么 wgpu 现在吃不到

1. **WGSL 没有 bf16**。naga 30.0.1 全仓 `grep -ri bf16` **0 命中**——IR 里根本没有 bf16 标量类型。
   因此带 `BFloat16Type` 的 SPIR-V 模块进不了 naga，bf16 张量核这条路当前是**硬阻塞**。
2. **f16 张量核有理论通道，但未验证**。naga 有 `TypeInner::CooperativeMatrix`，SPIR-V 后端也实现了
   它；但 **WGSL 前端没有入口**。本仓已证明同类绕行可行：GLSL(subgroup) → glslang → SPIR-V →
   `wgpu::ShaderSource::SpirV`（需开 wgpu 的 `spirv` feature），64 个值全部正确。
   把 subgroup 换成 cooperative matrix 是**下一步可试的最小探针**，但目前没有实测证据。
3. **WSL2 不适用**。gpu-unlock 改的是 Linux `nvidia.ko`；WSL2 的 GPU 走 Windows 驱动的
   `dxgkrnl`，模块加载不到。
4. **本机型号不匹配**。RTX 4060 Ti = AD106（Ada），不在 GA10x 列表；即使装原生 Linux 也不支持。

## 5. 支持度评分（回答"能不能有更好的支持度"）

| 能力 | 现状 | 支持度 |
|---|---|---|
| 三值 GEMM | 0.101 ps/MAC，popc 天花板 79–82% | **可用**（且不受 unlock 影响） |
| f32 SIMT GEMM | 30% 峰值，瓶颈在指令混合 | **可用**；提升靠内核，不靠解锁 |
| f16 存储变体 | 0.635 ps/MAC | **可用**；不是张量核 |
| f16 张量核（cooperative matrix） | 无 | **0**（SPIR-V 路线未证） |
| bf16 张量核 | 无 | **0**（naga 无 bf16 类型） |
| 多卡 / 调度到解锁卡 | 无 | **0** |

## 6. 建议分档（按"能不能当场验"排）

- **T0（无需硬件，今天可做）**：把这张矩阵写进 `trit-gpu` README；`bench` 输出补
  `f32_peak_ratio`，让"30% 峰值"成为常驻可见指标。目的：以后不再有人把 unlock 当成 f32 提速。
- **T1（真兼容第一步，需一张 Ampere GA10x 的 Linux 机器）**：GLSL cooperative matrix 探针 →
  SPIR-V → wgpu，先证"能跑且数值对"，再谈 matmul。这是**唯一不引入 CUDA 依赖**的通道。
- **T2（收益最大，但要新依赖 + 真机）**：可选 `cuda` feature 的 BF16 GEMM（cuBLASLt），
  Linux + sm_86 构建；CI 只跑编译与 CPU 对拍，真机跑分。此时 gpu-unlock 才真正有意义。
- **T3（不做）**：解锁状态检测/自动开关——没有可读接口，只能跑基准反推，收益低。

## 7. 未验证清单

- gpu-unlock 的 3090 Ti 数字来自项目自测与媒体转述，**未在本机复现**（本机没有 GA10x）。
- cooperative matrix 走 naga/SPIR-V 的可行性：**未实测**，只有 subgroup 的同类路径证据。
- "f16 张量核在消费卡上也被限速"依赖官方 FP16 +35% 的读数；不同型号/功耗档差异未测。
- 多卡 + vfio 的调度收益未评估（trit 无多卡代码）。

## 8. 对 #167 的建议改写

把"让 trit 兼容解锁后的 Ampere GPU"改写为：

> **补一条张量核后端（优先 f16 cooperative matrix，其次 CUDA BF16），unlock 只是这条后端的上游加速条件。**

验收判据也随之变成可复算的：在任一 GA10x Linux 机器上，张量核 GEMM 的 ps/MAC 相对
SIMT f32（0.298）的倍数；无卡环境下用数值对拍 + 编译门兜底，不报"已兼容"。
