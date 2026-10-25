# NVIDIA cuda-oxide 对照（f32 矩阵乘）

同卡、同形状、同布局、同一计时口径，把 **NVIDIA cuda-rust（cuda-oxide，Rust → PTX）**
和本仓 `crates/trit-gpu` 的 WGSL 内核放在一起比。

- 形状：`C[m][n] = Σ_k A[m][k]·Bᵀ[n][k]`，`m=512, k=4096, n=512`（B 转置存放）
- 计时：warmup 10；5 组 × 20 次，取中位数；读数统一 **ps/MAC**

## 实测（RTX 4060 Ti，driver 616.92，WSL2 Ubuntu 24.04，sm_89；2026-10-25 配对复跑）

| 内核 | cuda-oxide | 本仓 WGSL（最新） | 谁快 |
|---|---|---|---|
| naive（2D 16×16 块） | 6.205 | **3.141** | 本仓 **2.0×** |
| tiled 16×16 | 2.225 | **0.829** | 本仓 **2.7×** |
| reg2×2 + K 段 8 | 1.348 | **0.759** | 本仓 **1.8×** |
| reg4×4 + K 段 8 | 1.557 | **0.393** | 本仓 **4.0×** |
| **最优对最优** | 1.348（reg2） | **0.301**（reg4 + K 段 32） | 本仓 **4.5×** |

（2026-10-25 `2c4e77d` 复测；三值最优 `main_splitk` 0.1002 ps/MAC，见 `crates/trit-gpu/README.md`。）

**两个根因**：既不是线程映射，也不是 FMA 收缩，而是**内存层的两笔账**。

1. **访问宽度**：cuda-oxide 的参考内核都用标量 `f32`（全局 32 位、共享读也是 32 位）。
   本仓换成 128 位——全局 `array<vec4<f32>>` 只读视图（同一个 buffer），共享内存按
   `vec4<f32>` 组织、内层一次读喂 4 个乘加。
2. **共享内存 bank 冲突**：行距 8 个 vec4 = 128 B，等于全部 32 个 bank 的宽度，
   同一 warp 的 16 行全落在同一组 bank。先做「+1 vec4 行填充」，再改成
   `part ^ (row >> k)` 的 XOR swizzle。

| 内核 | 原始 | 128 位装载 | + swizzle | 最终 |
|---|---|---|---|---|
| naive 2D | 6.24 | **3.12** | — | **3.12** |
| tiled 16×16 | 2.37 | 1.77 | 0.84 | **0.84** |
| reg2 + K8 | 1.45 | 1.16 | 0.76 | **0.76** |
| reg4 + K8 | 1.34 | 1.00 | 0.40 | **0.40** |
| reg4 + K32 | — | 0.91 | 0.30 | **0.298** |

**口径说明（不回避）**：四对仍是同一算法、同一线程/分块结构，但向量化装载是**实现级**差异——
NVIDIA 的参考内核没有做这件事。如果他们也上 128 位装载，差距会缩小（幅度未知）。这是
「同结构实现赛跑」，不是「算法代差」。三值侧（本仓自己的后端）也试过把共享读改成 `vec4<u32>`，
反而更慢（o3 0.110 → 0.126）：三值每 MAC 已由 popc 主导，砍共享读没有回报。

**另外**：本仓三值内核 `main_splitk` 0.101 ps/MAC 对 cuda-oxide 最优 f32 0.298 约 **3×**
（对它的最优 1.348 约 13×）。三值/二值属不同表示与不同任务，不能当同尺结论。

## 环境（本机 2026-10-25 实测可跑）

cuda-oxide 只支持 Linux（CUDA 13 + LLVM 21 + Rust 1.99.1 钉死）。Windows 宿主用 WSL2：

1. WSL2 Ubuntu 24.04（GPU 直通已可用：`wsl -e nvidia-smi -L` 能看到卡）
2. CUDA Toolkit 13.0（WSL 源）：
   ```bash
   wget https://developer.download.nvidia.com/compute/cuda/repos/wsl-ubuntu/x86_64/cuda-keyring_1.1-1_all.deb
   sudo dpkg -i cuda-keyring_1.1-1_all.deb && sudo apt update && sudo apt install -y cuda-toolkit-13-0
   ```
3. LLVM 21 + clang 21：`llvm.sh 21` + `libclang-21-dev libclang-cpp21-dev libclang-common-21-dev`
4. Rust：`rustup toolchain install 1.99.1` + `rust-src rustc-dev llvm-tools`
   （`static.rust-lang.org` 在 WSL 里常超时，用 `RUSTUP_DIST_SERVER=https://rsproxy.cn`）
5. 克隆 <https://github.com/NVIDIA/cuda-rust>（WSL 连不上 GitHub 时在宿主克隆再拷进去），
   `cd cuda-oxide && cargo +1.99.1 install --path crates/cargo-oxide --locked`，
   然后 `cargo oxide setup`（编 codegen backend）、`cargo oxide doctor`（应全绿）
6. 本目录：把 `Cargo.toml` 里的 `__CUDA_RUST__` 换成本地 checkout 路径，
   ```bash
   export PATH=/usr/local/cuda/bin:$PATH
   cargo oxide run --arch sm_89
   ```

## 未做

- 只测一张卡、一个规模档；没做 cutile-rs（Tile 模型）对照。
- 没给 cuda-oxide 侧的参考内核补向量化装载或 bank swizzle（本仓的领先很大一部分来自这里，见上文口径说明）。
- 没测混合精度或分块形状搜索（BLOCK 16/32/64 的取舍只做了回归不做的粗扫）。
- 三值侧「3-popc + split-K」**已做**（`main_splitk`，0.1002 ps/MAC；POPC 利用率 88%，
  理论余量约 1.1×）。新增三值内核仍受 `matmul.rs` 792/800 行预算约束。
