# gpulib Vulkan 后端 × trit-gpu 三值 matmul 对照

同卡、同形状、同一份 **WGSL 源码**、同一份输入与 CPU 期望值，只换后端：

| 路径 | 驱动方式 |
|---|---|
| **wgpu** | `crates/trit-gpu` 现有路径（wgpu 30 / naga 30 → Vulkan） |
| **gpulib** | `/d/KF/gpulib` 的原生 Vulkan 后端（ash 0.38；WGSL → SPIR-V 后直接构造命令缓冲） |

这是一个**独立 workspace**，与 [`../cuda-oxide-compare`](../cuda-oxide-compare) 同模式：
依赖本机 gpulib 检出，不进主 workspace、不进 CI。目的是把「同一个三值内核换个后端能差多少」
变成可复算的读数，而不是凭印象换后端。

## 复现

1. 准备 gpulib 检出，默认路径 `../../../gpulib`（即与 trit 检出同级）；
   不在这个位置就改 `Cargo.toml` 里 `gpulib-core` / `gpulib-vulkan` 两条 `path`。
2. 跑对照（默认形状与 `crates/trit-gpu/README.md` 的读数对齐）：

```bash
cargo run --release -- --m 512 --k 4096 --n 512
```

可选参数：

```text
--skip-wgpu    只跑 gpulib 路径
--wgpu-first   先跑 wgpu（GPU 时钟/温度会随负载爬升，换顺序排除单侧占便宜）
```

计时口径与 `trit-gpu` 一致：warmup 20；5 组 × 20 次 dispatch，取中位 ns/MAC；
两侧都在 20 次 dispatch 后统一等待（wgpu 是 20 次 `queue.submit` + 一次 poll，
gpulib 是 20 次 `dispatch` 累积进一个 command buffer + 一次 `sync`）。

## 实测（RTX 4060 Ti，driver 616.92，2026-11-17）

`m=512, k=4096, n=512`（1.07 G MAC）。两轮独立运行，**顺序对调**：

| 内核 | gpulib ps/MAC | wgpu ps/MAC | gpulib/wgpu 时间比 |
|---|---:|---:|---:|
| `main`（naive） | 1.910 / 1.800 | — | — |
| `main_tiled`（16×16） | 0.152 / 0.138 | — | — |
| `main_tiled2`（2×2） | 0.137 / 0.124 | — | — |
| `main_tiled4`（4×4） | 0.118 / 0.116 | — | — |
| `main_tiled2o3`（3-popc） | **0.121 / 0.113** | **0.103 / 0.103** | **1.10–1.17×** |
| `main_splitk`（split-K×4） | **0.124 / 0.125** | **0.110 / 0.102** | **1.13–1.23×** |

（前一个数是 gpulib 先跑，后一个数是 wgpu 先跑。）

**正确性**：六个内核读回的输出与 `trit_gpu::matmul::Inputs` 的 CPU 位并行期望值
**逐元素相等**；两侧都通过。

## 结论

1. **接入本身成立**：trit-gpu 的 WGSL（双导轨 + `countOneBits`，含 `workgroup` 共享内存、
   `atomic<i32>` split-K）可以用 naga 30 编成 SPIR-V，由 gpulib 的 Vulkan 后端原样执行，
   结果与 CPU 镜像逐位一致。不需要为 Vulkan 改一版内核。
2. **这组负载上 gpulib 没有复现它自己 f32 conv 的 11.2% 优势**：3-popc 慢约 **10%**，
   split-K 慢约 **13–23%**。两次换序都成立，不是「后跑的一侧 GPU 更热」造成的。
   gpulib README 里那 11.2% 是 `conv_tiled`（f32、共享内存 tile、不同计时口径），
   不能外推到三值 popc 内核——这正是本对照存在的意义。
3. **差距来源未定位**（未证栏，见下）。两个可疑点：
   - gpulib 的 `dispatch` 每次都在 compute shader 后插一个完整 `SHADER_WRITE →
     SHADER_READ|WRITE` 屏障；wgpu 把 20 次 dispatch 分成 20 个独立提交，由驱动决定
     同步粒度。若屏障让相邻 dispatch 串行化，正好落在 10% 量级。
   - split-K 的清零方式不同：wgpu 用 `Encoder::clear_buffer`，本工程用 gpulib 的一次
     额外 dispatch（64 线程清零内核）。这是实现级差异，split-K 的比值不能直接读成
     「后端差距」。
4. **这不构成「gpulib 更慢」的通用结论**。只测了一张卡、一个形状、一组三值内核；
   换 f32 / 卷积 / 常驻权重场景，gpulib 的薄抽象仍可能赢。要下通用结论必须补那些负载。

## 未证栏

- 没有做 GPU 计数器 / SASS 级归因（本机 `ncu` 报 `ERR_NVGPUCTRPERM`），
  「barrier 串行化」只是可解释的假设，不是已证实的原因。
- 没有对照 gpulib 的 f32 `conv_tiled` 路径；那才是它 README 里 11.2% 的来源。
- 只测 `512×4096×512` 一个形状；小形状（如 `64×256×64`）dispatch 开销占主导，
  本工程能跑，但读数不适合跨后端比较。
- gpulib 路径目前只覆盖 matmul；WF32（三值权重 × f32 激活）与 `dp4a` 未接。
- 依赖本地 gpulib 检出的路径，没有版本钉住（gpulib 无 git remote）；
  读数对应 2026-11-17 的 `D:/KF/gpulib` 工作副本。
