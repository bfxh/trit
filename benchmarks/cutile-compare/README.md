# NVIDIA cuTile（cutile-rs）对照（f32 矩阵乘）

同卡（RTX 4060 Ti）、同形状（512×4096×512）、同一计时口径（warmup 5；5 组 × 20 次取中位数；
读数统一 **ps/MAC**），把 NVIDIA 的 **cuTile（Tile 模型，Rust → CUDA Tile IR）** 与
本仓 `crates/trit-gpu` 的 WGSL 内核放在一起比。

## 实测（2026-10-25）

| 实现 | tile | ps/MAC | 相对本仓 f32 最优 |
|---|---|---|---|
| cuTile `gemm`（示例内核） | 16×16×8 | **2.613** | 慢 8.7× |
| cuTile `gemm`（示例内核） | 64×64×16 | 2.707 | 慢 9.0× |
| 本仓 f32 `main_tiled4k32` | 4×4 + K32 | **0.301** | — |
| 本仓 f32 `main_tiled2k8_f16`（fp16 存储） | 2×2 + K8 | 0.635 | — |
| cuda-oxide 最优（reg2-K8） | 2×2 + K8 | 1.348 | — |

## 口径（不回避）

- cuTile 是 **Tile 模型**（`partition` + `mma`，由 CUDA Tile IR JIT 编译），与本仓的 SIMT
  逐线程内核**不是同结构**；这里比的是「同一问题的两种实现模型」，不是同结构实现赛跑。
- 用的是 cuTile 官方 `gemm` 示例内核，只把形状改成 512×4096×512 并加计时；**没有**做
  tile 形状搜索、pipeline/多级流水、split-K 等调优。16×16×8 与 64×64×16 两档都试过，
  后者没有更快。
- cuTile 的 `mma` 在 f32 上走 Tensor Core 路径（TF32 或等价），本仓 f32 是标量 FMA；
  精度口径不同，这里只比吞吐。
- 单卡单形状；换卡或换形状需要重测。

## 复现步骤（WSL2）

```bash
# 1) CUDA 13.2+（cuTile 运行时需要 tileiras，13.0 的 tileiras 拒绝 13.2 的 IR）
sudo apt install -y cuda-toolkit-13-2

# 2) cuda-rust 仓库（含 cuda-oxide 与 cutile-rs）
git clone https://github.com/NVIDIA/cuda-rust ~/cuda-rust

# 3) 用 13.2 全套环境（CUDA_HOME/PATH 都指向 13.2）
export CUDA_HOME=/usr/local/cuda-13.2 CUDA_TOOLKIT_PATH=/usr/local/cuda-13.2
export PATH=/usr/local/cuda-13.2/bin:$PATH

# 4) 在 cutile-rs workspace 内加一个带计时的 gemm example（形状 512×512×4096），
#    用已安装的 1.99.1 工具链（rust-toolchain.toml 钉的 1.98.0 会触发下载）：
cd ~/cuda-rust/cutile-rs && cargo +1.99.1 run --release --example gemm_bench
```

## 环境备注

- 本机 `rust-toolchain.toml` 钉 1.98.0，未预装；用 `cargo +1.99.1` 绕过（未改仓库配置）。
- `cuda-toolkit-13-2` 装完后 `update-alternatives` 会指向 13.2；若还要跑 cuda-oxide 对照，
  用 `CUDA_HOME=/usr/local/cuda-13.0` 显式指定。
- 本目录不包含可执行工程：cutile-rs 依赖外部 checkout 与 CUDA Tile IR 工具链，
  入库会引入不可复现的重依赖；这里只记录读数与复现步骤。
