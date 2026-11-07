# K3 位切片汇编验证（第一片）

状态：实验记录，非默认构建，不在 Cargo 工作区。

目标：验证“汇编做三进制/多值”是否值得，以及和 Rust intrinsics 的差距有多大。

## 范围

- 运算：K3 的 `AND` / `OR` / `NOT`，双导轨 `(ones, unknowns)`。
- 实现：
  - `scalar`：u64 逐字基线（编译器可能用 SSE2 自动向量化）。
  - `avx2_int`：Rust `core::arch::x86_64` AVX2 intrinsics。
  - `asm`：手写 AVX2 汇编（Windows x64；其他平台回退到 intrinsics）。
- 本机：Intel Core Ultra 7 270K Plus，Windows x64；`avx2=true`、`bmi2=true`、`avx512f=false`。
- 汇编是二进制 ISA 上的 **B 类模拟**，不是原生三值硬件；SAT/BDD/ATPG 等 D 类算法瓶颈不在本片范围。

## 正确性

- 9 种 trit 组合 × 3 个运算的真值表，逐位核对。
- 23 种长度（0、1、3、4、5、7、8、9、15、16、17、31、32、33、63、64、65、127、128、129、1000、4096）随机双导轨输入，scalar / AVX2 intrinsics / 汇编三方逐位一致。
- 非 4 字倍数长度的尾部由标量处理，专门覆盖跨字边界。
- 输入保证合法 `(ones & unknowns) == 0`，不把 `(1,1)` 当合法 trit。

## 基准方法

- 两组工作集：
  - 16,384 words = 128 KiB/plane，计算/缓存为主。
  - 1,048,576 words = 8 MiB/plane，内存带宽为主。
- 绑定 CPU 0（Windows `SetThreadAffinityMask`，尽力而为）。
- scalar / intrinsics / asm 三个内核**交错采样**，避免频率漂移把某一内核“测快”。
- 每组取 100 / 50 次采样的中位数；`cyc/word` 为 TSC 参考周期/字。
- 输出字节数按 4 个输入平面 + 2 个输出平面计（`NOT` 为 2+2）。
- 未做核心拓扑级 pinning（P/E 核区分）、未固定倍频；读数是本机单次实验，不是跨机承诺。

## 读数（2026-11-08，本机）

### 16,384 words（128 KiB/plane）

| kernel | cyc/word | vs scalar |
|---|---:|---:|
| scalar_and | 2.589 | 1.00x |
| avx2_int_and | 2.091 | **1.24x** |
| asm_and | 2.414 | 1.07x |
| scalar_or | 2.478 | 1.00x |
| avx2_int_or | 2.181 | **1.14x** |
| asm_or | 2.369 | 1.05x |
| scalar_not | 2.047 | 1.00x |
| avx2_int_not | 1.811 | 1.13x |
| asm_not | 1.794 | 1.14x |

### 1,048,576 words（8 MiB/plane）

| kernel | cyc/word | vs scalar |
|---|---:|---:|
| scalar_and | 8.120 | 1.00x |
| avx2_int_and | 7.803 | 1.04x |
| asm_and | 7.840 | 1.04x |
| scalar_or | 7.552 | 1.00x |
| avx2_int_or | 6.975 | 1.08x |
| asm_or | 7.866 | 0.96x |
| scalar_not | 4.181 | 1.00x |
| avx2_int_not | 4.376 | 0.96x |
| asm_not | 4.097 | 1.02x |

> 读数波动：本机单轮中 `NOT` 的汇编读数波动明显；`AND/OR` 的 intrinsics 优势较稳定。数字只作本机证据，不作跨机承诺；结论以多次复跑和同一轮相对比较为准。

## 结论

1. **正确性通过**：手写 AVX2 在三值真值表、23 种长度、跨字尾部上都与 scalar/intrinsics 逐位一致。
2. **简单 K3 位运算不是汇编的优势区**：在小工作集上，AVX2 intrinsics 对 `AND/OR` 有 1.13–1.24× 提升；手写汇编只有 1.03–1.07×，没有打赢 intrinsics。`NOT` 的汇编读数波动较大（见过 0.80–1.14×），至少没有稳定超过 intrinsics。
3. **大工作集是内存瓶颈**：8 MiB/plane 时差异收敛到约 1.0×；这时指令级优化收益很小，先优化数据布局/分块/缓存，而不是换指令。
4. **手写汇编不是“自动更快”**：本版已把 6 次指针自增改成索引寻址，仍没超过编译器生成的 AVX2。进一步的展开/调度可能缩小差距，但投入产出比要先证明。
5. **汇编仍保留 Tier 0，但按内核选型**：只在编译器代码生成弱、或需要精确控制指令/布局的内核上用汇编；简单 K3 位运算默认用 Rust intrinsics。

## 下一步（按优先级）

- **5 trits/byte 打包/解包**：BMI2 `pext/pdep` 与查找表对比；这是最可能让汇编/位技巧赢的内核之一。
- **GF(3) 算术**：SIMD 查表/位切片 vs 标量；需要带二进制基线。
- **Belnap 4 值/多值位平面**：验证“多值”路径，而不只是 K3。
- **ARM64 NEON/SVE**：跨 ISA 才是汇编最可能拉开差距的地方；x86-64 上编译器已经很强。
- **AVX-512 `vpternlogd`**：本机无 AVX-512，暂不能测；有机器时再补。
- **合并门槛**：任何汇编内核必须同时满足：正确性全过、同算法同数据、比 intrinsics 基线至少快一个预先约定的幅度（例如 ≥10%）、跨大小/跨机器可复现。否则保留 intrinsics。

## 复现

```bash
cd other-languages/asm
cargo run --release
```

文件：`Cargo.toml`、`src/main.rs`。该 crate 是独立 workspace，不进入主 Cargo 工作区，也不参与默认 CI。
