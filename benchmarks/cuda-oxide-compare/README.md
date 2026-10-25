# NVIDIA cuda-oxide 对照（f32 矩阵乘）

同卡、同形状、同布局、同一计时口径，把 **NVIDIA cuda-rust（cuda-oxide，Rust → PTX）**
和本仓 `crates/trit-gpu` 的 WGSL 内核放在一起比。

- 形状：`C[m][n] = Σ_k A[m][k]·Bᵀ[n][k]`，`m=512, k=4096, n=512`（B 转置存放）
- 计时：warmup 10；5 组 × 20 次，取中位数；读数统一 **ps/MAC**

## 实测（RTX 4060 Ti，driver 616.92，WSL2 Ubuntu 24.04，sm_89）

| 内核（同结构） | cuda-oxide | 本仓 WGSL | 谁快 |
|---|---|---|---|
| naive（一线程一输出） | **6.233** | 23.80 | NVIDIA 3.8× |
| tiled 16×16 共享内存 | **2.243** | 2.378 | NVIDIA ~6% |
| reg2×2（8×8 线程、16×16 tile） | **1.328** | 2.389 | NVIDIA 1.8× |
| reg4×4（8×8 线程、32×32 tile） | 1.567 | **1.349** | 本仓 14% |

**结论（如实）**：四对里只赢 reg4×4。三项落后的原因还没定位；目前唯一能站住的
"领先"是**三值内核 vs 二进制内核**（本仓 `main_tiled2o3` 0.110 ps/MAC 对 cuda-oxide
最优 f32 1.328 ps/MAC ≈ **12×**），但那是不同表示、不同任务，不能当同尺结论。

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
- 没定位三项落后的根因（怀疑与我们 WGSL 的线程/网格映射和 f32 内层展开有关）。
