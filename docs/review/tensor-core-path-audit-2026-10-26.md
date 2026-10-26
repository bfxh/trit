# trit 有没有张量核 GEMM 路径？（2026-10-26 核查）

对 #167 评估里「张量核 GEMM 会受益，但 trit 还没有这条路径」的追问。本文只回答**存在性**：
trit 现在到底有没有一条指令走 tensor core；有就给 `file:line`，没有就给反证。

## 0. 结论

**没有。** trit 当前所有矩阵乘都是 **SIMT / 整数 popc**：

- 三值 GEMM 走 `countOneBits`（整数）；
- f32 GEMM 走标量 FMA（SIMT）；
- f16 变体只把**存储与共享内存**换成 `vec4<f16>`，累加仍是 f32，指令仍是 SIMT；
- 全仓没有任何 CUDA / cuBLAS / cooperative-matrix 依赖或代码。

`benchmarks/cuda-oxide-compare/` 是**独立 workspace 的对照基准**（不在根 `Cargo.toml` 的 `members` 里），
用来量「别人家的 CUDA 实现有多快」，不是 trit 的后端。

## 1. 四条计算路径的实际指令类型

| 路径 | 位置 | 实际指令 | 张量核？ |
|---|---|---|---|
| 三值 GEMM（naive） | `crates/trit-gpu/src/matmul.wgsl:21,38-41` | `countOneBits(ap&bp) + ...` | 否 |
| 三值 GEMM（双导轨） | `crates/trit-gpu/src/ternary.wgsl:17,31-32` | `countOneBits`（整数） | 否 |
| f32 GEMM（4×4 阻塞） | `crates/trit-gpu/src/matmul_f32.wgsl:408-409` | `@compute @workgroup_size(8,8)` + 标量 FMA | 否 |
| f16 存储变体 | `crates/trit-gpu/src/matmul_f16.wgsl:1,15-16,25` | `enable f16`、`vec4<f16>` 存储；累加 f32 | 否（SIMT FMA） |
| MOD 前向端到端 | `crates/trit-gpu/src/forward.rs:73,95` | 复用 `matmul.wgsl` 的 `main` | 否 |

复算命令：

```bash
grep -rn "countOneBits" crates/trit-gpu/src/*.wgsl
grep -n "workgroup_size\|fn main_tiled4k32" crates/trit-gpu/src/matmul_f32.wgsl
grep -rn "cooperative_matrix\|CooperativeMatrix\|tensor" crates/trit-gpu/src/
```

最后一条在当前树**零命中**（`trit-texture` 里的 `BF16` 是文件格式，见 §3）。

## 2. 依赖面证据

- `crates/trit-gpu/Cargo.toml:17`：只有 `wgpu = { version = "30", optional = true }`。
- 全仓 `grep -rn "cublas|cuda|cutlass|cooperative" --include=Cargo.toml` 只命中
  `benchmarks/cuda-oxide-compare/Cargo.toml:12-14`——它 `path` 指向本地 cuda-rust checkout，
  需要私有编译器 `cargo oxide`，且注释写明「独立 workspace」。
- 根 `Cargo.toml` 的 `members` 不含 `benchmarks/`，所以它不进任何 CI 门。

## 3. bf16 只出现在「读文件」，不在「算」

`crates/trit-texture/src/gguf.rs:355-358`：

```rust
TYPE_BF16 => {
    let bits = u16::from_le_bytes(...);
    f32::from_bits((bits as u32) << 16)
}
```

GGUF 里的 BF16 张量在这里被**解码成 f32** 再进模型；导入器只支持 F32/F16/BF16 的格式解码
（`gguf.rs:5,119`），不保留 bf16 计算类型。

## 4. 为什么「没有」是取舍，不是漏项

1. **三值 GEMM 的收益来源就是避开浮点乘**：双导轨把每个 trit 变成 2 bit，点积变成 4 组 popc；
   已经到 popc 天花板 79–82%。张量核做的是 FP16/BF16 的 FMA——要用它必须先把三值还原成浮点，
   等于把已经省掉的乘法再加回来。
2. **f32 路径只是同结构对照**：0.298 ps/MAC ≈ 30% FP32 峰值，README 已把剩余差距定位在
   指令混合（共享读 + 索引 + 累加），不是 FMA 发射率。
3. **张量核真正有意义的是 trit-net 的训练/浮点前向**：STE 反传、浮点权重更新仍是 f32 矩阵乘，
   那里才吃 FP16/BF16 张量核；推理侧的三值路径不需要。

## 5. 要做的话，三条候选（按依赖成本）

| 路线 | 前置条件 | 现状 |
|---|---|---|
| GLSL cooperative matrix → SPIR-V → wgpu | glslang（本机 16.6.0 已有）、wgpu 的 `spirv` feature | **同类路径已证**（subgroup 探针 64/64 正确）；cooperative matrix 本身**未证** |
| CUDA + cuBLASLt（bf16/f16） | 新依赖、Linux + sm_86、真机 | 无 |
| 直接写 Vulkan GLSL 走 wgpu | 同第一条 | 无 |

第一条的最小验证：一个 `16×16×16` 的 f16 cooperative-matrix GEMM 探针，与 CPU 镜像逐元素对拍，
再在 512×4096×512 上量 ps/MAC。**先证存在性，再谈收益**。

## 6. 验收判据

- 存在性：`grep -rn "OpCooperativeMatrix\|cooperative_matrix" crates/` 非空，且内核里出现
  cooperative-matrix 类型/内建；
- 正确性：与 CPU 镜像逐元素对拍（相对误差 0 或 < 1e-3，取决于 f16）；
- 收益：同形状 ps/MAC 相对 f32 SIMT（0.298）的倍数，且注明卡型号与功耗档。

## 7. 未验证

- cooperative matrix 在 naga 30.0.1 的 SPIR-V 前端能否被接受：只有 `TypeInner::CooperativeMatrix`
  与 SPIR-V 后端的代码证据，**没有跑通**；
- 即使跑通，消费卡上 f16 张量核的实际发射率也未在本机测过。
