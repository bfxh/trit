# NVIDIA cuTile（cutile-rs）对照（f32 矩阵乘）

同卡（RTX 4060 Ti）、同形状（512×4096×512）、同一计时口径（warmup 5；5 组 × 20 次取中位数；
读数统一 **ps/MAC**），把 NVIDIA 的 **cuTile（Tile 模型，Rust → CUDA Tile IR）** 与本仓
`crates/trit-gpu` 的 WGSL 内核放在一起比。

## 调优后的读数（2026-10-25）

| 实现 | 配置 | ps/MAC |
|---|---|---|
| cuTile `gemm`（**tile 搜索最优**） | 32×256×8 | **0.492** |
| cuTile `gemm`（官方示例默认） | 16×16×8 | 2.711 |
| cuTile `gemm` + split-K×2/×4/×8 | 32×256×8 | 0.693 / 0.684 / 0.727（**负收益**） |
| 本仓 f32 `main_tiled4k32` | 4×4 + K32 | **0.301** |
| 本仓 f32 `main_tiled2k8_f16`（fp16 存储） | 2×2 + K8 | 0.635 |
| cuda-oxide 最优（reg2-K8） | 2×2 + K8 | 1.348 |

**结论**：把 tile 搜索做完后，cuTile 从 2.71 降到 **0.492**（5.5×）；本仓 f32 最优仍是 0.301，
即本仓约快 **1.63×**（此前按默认 tile 算是 8.7×——那个倍数不再成立）。

## tile 搜索（全部实测）

| BM×BN×BK | ps/MAC | | BM×BN×BK | ps/MAC |
|---|---|---|---|---|
| 16×16×8 | 2.711 | | 64×64×4 | 1.279 |
| 16×16×16 | 4.128 | | 64×64×8 | 1.076 |
| 32×32×8 | 2.184 | | 128×64×8 | 1.120 |
| 32×32×16 | 2.045 | | 64×128×8 | 1.031 |
| 64×32×16 | 1.178 | | 128×128×8 | 2.523 |
| 32×64×16 | 1.155 | | 128×128×16 | 3.392 |
| 64×64×8 | 1.152 | | 128×64×16 | 1.598 |
| 64×64×16 | 2.842 | | 64×128×16 | 1.370 |
| 64×64×32 | 2.134 | | 32×128×8 | 0.833 |
| 128×32×8 | 1.156 | | 64×128×4 | 0.725 |
| 64×256×8 | 1.958 | | **32×256×8** | **0.492** |
| 32×256×4 | 0.615 | | 16×256×8 | 0.519 |
| 32×512×8 | 1.275 | | 16×512×8 | 1.077 |
| 64×256×4 | 2.491 | | | |

规律：`BN` 大、`BM` 小（32×256）明显更好；`BK=8` 稳定优于 16/32/4；
128×128 反而退化（寄存器/共享内存压力）。

## split-K（负结果）

两阶段实现：`gemm_split` 按 K 段写 `[S*m, n]`，`reduce_splits` 对 S 段求和（32×256×8）。

| S | ps/MAC |
|---|---|
| 1（等价单段） | 0.664 |
| 2 | 0.693 |
| 4 | 0.684 |
| 8 | 0.727 |

S=1 反而最快 ⇒ 这个形状（32 tiles，22 SM）并行度已经够，split-K 只增加 reduce 与访存开销。
注意 S=1 的 0.664 比单 kernel 的 0.492 慢，说明两阶段本身有固定开销。

## 流水

cuTile 没有显式的软件流水 API；`mma` 的 load/compute 重叠由 **CUDA Tile IR 编译器**自动调度
（`#[cutile::entry]` 的 `optimization_hints` 只暴露 occupancy / CGA 等提示）。
因此本对照里「流水」这一项不是手工调参，而是编译器默认行为；没有可对照的手工流水版本。

## 口径（不回避）

- cuTile 是 **Tile 模型**（`partition` + `mma`，CUDA Tile IR JIT），与本仓的 SIMT 逐线程内核
  不是同结构；比的是「同一问题的两种实现模型」。
- 用的是 cuTile 官方 `gemm` 内核，只改 tile 形状与加计时；**没有**改 kernel 体、没有手写流水。
- cuTile 的 f32 `mma` 走 Tensor Core（TF32 或等价）路径，本仓 f32 是标量 FMA；只比吞吐不比精度。
- 单卡单形状；换卡或换形状需要重测。
- 未做逐元素正确性校验：用的是官方 gemm 内核，只改 tile 常量；官方 `gemm.rs` 自带校验路径。

## 复现步骤（WSL2）

```bash
sudo apt install -y cuda-toolkit-13-2          # 含 tileiras 13.2
git clone https://github.com/NVIDIA/cuda-rust ~/cuda-rust
export CUDA_HOME=/usr/local/cuda-13.2 CUDA_TOOLKIT_PATH=/usr/local/cuda-13.2
export PATH=/usr/local/cuda-13.2/bin:$PATH
# 在 cutile-rs workspace 内加带计时的 gemm example（形状 512×512×4096，tile 可参数化）
cd ~/cuda-rust/cutile-rs && cargo +1.99.1 run --release --example gemm_bench
```

## 环境备注

- 本机 `rust-toolchain.toml` 钉 1.98.0，未预装；用 `cargo +1.99.1` 绕过（未改仓库配置）。
- `cuda-toolkit-13-2` 装完后 `update-alternatives` 会指向 13.2；若还要跑 cuda-oxide 对照，
  用 `CUDA_HOME=/usr/local/cuda-13.0` 显式指定。
- 本目录不包含可执行工程：cutile-rs 依赖外部 checkout 与 CUDA Tile IR 工具链，
  入库会引入不可复现的重依赖；这里只记录读数与复现步骤。
