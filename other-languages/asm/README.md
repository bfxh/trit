# 三进制汇编验证：K3、5 trits/byte、GF(3)、Belnap 4

状态：实验记录，非默认构建，不在 Cargo 工作区。
目标：验证“汇编/低位宽指令做三进制/多值”是否值得，以及和 Rust intrinsics、标量 LUT 的差距。

## 范围

- **K3 位切片**：`AND` / `OR` / `NOT`，双导轨 `(ones, unknowns)`。
- **5 trits/byte**：`pack` / `unpack`，trit 用 `i8` 的 `-1/0/+1`，字节内低位在前，尾部用 trit 0 填充。
- **GF(3)**：`add` / `mul`，输入输出为字节 `0/1/2`，索引 `a*3+b`。
- **Belnap 4**：`truth/falsity` 双位平面，`N=(0,0)`、`T=(1,0)`、`F=(0,1)`、`B=(1,1)`；`AND`/`OR`/`NOT`。
- 实现：
  - `scalar`：K3/Belnap 为 u64 逐字；pack5/GF(3) 为算术与查找表。
  - `intrinsics`：Rust `core::arch::x86_64` 的 AVX2 / BMI2。
  - `asm`：手写汇编（Windows x64；其他平台回退到 intrinsics 或标量）。
- 另外：
  - **ARM64 NEON**：独立 `asm-arm64/` crate，只做交叉编译检查（无 ARM64 硬件/QEMU）。
  - **AVX-512 VBMI**：`src/avx512.rs` 的 GF(3) 路径，本机无 AVX-512，只编译不执行。
- 本机：Intel Core Ultra 7 270K Plus，Windows x64；`avx2=true`、`bmi2=true`、`avx512f=false`。
- 汇编是二进制 ISA 上的 **B 类模拟**，不是原生三值硬件；SAT/BDD/ATPG 等 D 类算法瓶颈不在本片范围。

## 正确性

- **K3**：9 种 trit 组合 × 3 个运算的真值表；23 种长度随机双导轨输入；scalar / AVX2 intrinsics / 汇编逐位一致。尾部由标量处理，覆盖跨字边界。
- **pack5**：全部 243 种 5-trit 组合；25 种长度的随机输入（含尾部）；pack 的 scalar 算术 / LUT / BMI2 intrinsic / 汇编四方一致；unpack 的算术 / LUT / 汇编三方一致；往返恢复原 trit 序列；非法字节 243 被标量路径拒绝。
- **GF(3)**：23 种长度随机输入；9 种 `(a,b)` 真值对；scalar 算术 / LUT / AVX2 intrinsics / 汇编四方一致。
- **Belnap 4**：4×4 = 16 种值对；23 种长度随机双平面输入；scalar / AVX2 intrinsics / 汇编三方一致。
- 快速路径假设输入合法：pack5 的 unpack 快速路径要求字节 `0..=242`；GF(3) 要求 `0..=2`。标量路径负责边界校验；安全 API 应在入口做校验。
- **ARM64 NEON**：`cargo check --target aarch64-unknown-linux-gnu` 通过；**未运行**，没有 ARM64 执行证据。
- **AVX-512**：代码参与编译；运行时检查 `avx512f/bw/vbmi`，本机不满足，因此**未执行**。

## 基准方法

- 绑定 CPU 0（Windows `SetThreadAffinityMask`，尽力而为）。
- scalar / intrinsics / asm 内核**交错采样**，避免频率漂移把某一内核“测快”。
- 每组取 100 / 50 次采样的中位数；`cyc` 为 TSC 参考周期。
- K3/Belnap 4：16,384 words（128 KiB/plane，缓存/计算为主）与 1,048,576 words（8 MiB/plane，内存为主）。
- pack5/GF(3)：65,536 与 1,048,576 个 trit/byte；pack 字节数按输入+输出计，GF(3) 按 2 输入+1 输出计。
- 未做 P/E 核拓扑级 pinning、未固定倍频；读数是本机单次实验，不是跨机承诺。

## K3 读数（较早一轮，2026-11-08）

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

> K3/Belnap 表与 pack5/GF(3) 表来自不同轮次，不跨轮比较绝对数；每轮内相对比较才有效。`NOT` 的汇编读数波动明显，不把单轮数字当稳定结论。

## 5 trits/byte 读数（2026-11-08）

### 65,536 trits

| 方向 | kernel | cyc/trit | vs 标量 LUT |
|---|---|---:|---:|
| pack | scalar_lut | 0.728 | 1.00x |
| pack | pext_int | 0.182 | **4.01x** |
| pack | asm_pext | 0.180 | **4.05x** |
| unpack | scalar_lut | 0.239 | 1.00x |
| unpack | asm_lut | 0.158 | **1.51x** |

### 1,048,576 trits

| 方向 | kernel | cyc/trit | vs 标量 LUT |
|---|---|---:|---:|
| pack | scalar_lut | 0.741 | 1.00x |
| pack | pext_int | 0.185 | **4.01x** |
| pack | asm_pext | 0.182 | **4.08x** |
| unpack | scalar_lut | 0.212 | 1.00x |
| unpack | asm_lut | 0.165 | **1.29x** |

**解读**：pack 的 4× 来自 **BMI2 `pext` + 1024 项 LUT**，不是“手写汇编”本身；手写 asm 与 Rust `_pext_u64` intrinsic 基本持平（0.180–0.185 cyc/trit）。unpack 的 1.3–1.5× 来自 LUT 批量拷贝，asm 与编译器生成的拷贝在同一量级。

## GF(3) 读数（2026-11-08）

### 65,536 bytes

| 运算 | kernel | cyc/byte | vs 标量 LUT |
|---|---|---:|---:|
| add | scalar_lut | 1.214 | 1.00x |
| add | avx2_int | 0.065 | **18.64x** |
| add | asm_vpshufb | 0.083 | **14.54x** |
| mul | scalar_lut | 1.215 | 1.00x |
| mul | avx2_int | 0.068 | **17.95x** |
| mul | asm_vpshufb | 0.087 | **14.02x** |

### 1,048,576 bytes

| 运算 | kernel | cyc/byte | vs 标量 LUT |
|---|---|---:|---:|
| add | scalar_lut | 1.265 | 1.00x |
| add | avx2_int | 0.107 | **11.86x** |
| add | asm_vpshufb | 0.107 | **11.86x** |
| mul | scalar_lut | 1.274 | 1.00x |
| mul | avx2_int | 0.108 | **11.74x** |
| mul | asm_vpshufb | 0.108 | **11.75x** |

**解读**：12–19× 来自 **AVX2 `vpshufb` 16 项查表**：`add` 用 `idx=a+b`，`mul` 用 `idx=3a+b`。大数组时 asm 与 intrinsics 完全打平；小数组 asm 略慢。没有观察到“手写汇编超过 intrinsics”的稳定优势。

## Belnap 4 读数（2026-11-08）

### 16,384 words（128 KiB/plane）

| kernel | cyc/word | vs scalar |
|---|---:|---:|
| scalar_and | 2.406 | 1.00x |
| avx2_int_and | 2.177 | 1.11x |
| asm_and | 2.178 | 1.10x |
| scalar_or | 2.486 | 1.00x |
| avx2_int_or | 2.412 | 1.03x |
| asm_or | 2.645 | 0.94x |
| scalar_not | 1.970 | 1.00x |
| avx2_int_not | 2.019 | 0.98x |
| asm_not | 2.154 | 0.91x |

### 1,048,576 words（8 MiB/plane）

| kernel | cyc/word | vs scalar |
|---|---:|---:|
| scalar_and | 8.818 | 1.00x |
| avx2_int_and | 8.342 | 1.06x |
| asm_and | 8.590 | 1.03x |
| scalar_or | 9.217 | 1.00x |
| avx2_int_or | 8.854 | 1.04x |
| asm_or | 8.858 | 1.04x |
| scalar_not | 5.904 | 1.00x |
| avx2_int_not | 5.566 | 1.06x |
| asm_not | 5.674 | 1.04x |

**解读**：Belnap 4 只是两条位平面上的 `and/or` 与交换，编译器已经做得很好；AVX2 只有 1.03–1.11×，汇编没有优势。多值位平面的价值在表示与真值表，不在手写指令。

## ARM64 NEON / AVX-512（编译验证）

- **ARM64 NEON**：[`asm-arm64/`](../asm-arm64/) 实现 GF(3) `add/mul` 的 `vqtbl1q_u8` 查表；`cargo check --target aarch64-unknown-linux-gnu` 通过。**没有 ARM64 硬件/QEMU，未运行**，因此不写 NEON 性能数字。
- **SVE**：稳定 Rust 1.99 的 SVE intrinsics 仍是 `stdarch_aarch64_sve` 不稳定特性（rust-lang/rust#145052），且本机无 SVE 硬件；暂缓，NEON 先行。
- **AVX-512 VBMI**：[`src/avx512.rs`](src/avx512.rs) 用 `_mm512_permutexvar_epi8` 做 GF(3) add/mul（`vpermb` 64 字节查表）；代码已编译。本机无 `avx512f/bw/vbmi`，`run()` 直接跳过，**未执行**、不写性能数字。

## 结论

1. **K3 位运算不是汇编的优势区**：AVX2 intrinsics 对 `AND/OR` 有 1.13–1.24× 提升；手写汇编只有 1.03–1.07×，没有打赢 intrinsics。
2. **5 trits/byte 是真正的低位宽收益区**：BMI2 `pext` + LUT 对 pack 有约 **4×**，unpack LUT 有 **1.3–1.5×**。
3. **GF(3) 是 SIMD 的强收益区**：`vpshufb` 查表对 add/mul 有 **12–19×**。
4. **Belnap 4 多值位平面偏编译器友好**：AVX2 只有 1.03–1.11×，汇编没有优势；多值的价值在表示与真值表。
5. **大赢来自指令集，不是汇编语法**：pack 的 `pext`、GF(3) 的 `vpshufb` 都可以通过 Rust intrinsics 直接拿到；手写 asm 与 intrinsics 基本持平。汇编的价值在跨 ISA / ABI / 编译器 codegen 弱的场景。
6. **ARM64/AVX-512 目前只有编译证据**：NEON 交叉检查通过、AVX-512 路径编译通过；没有硬件执行数据，不能写成“更快”。
7. **汇编仍是 Tier 0，但按内核选型**：优先把低位宽/SIMD 内核做到位（pack5、GF(3)、多值位平面），默认用 intrinsics；只有 asm 比 intrinsics 稳定快一个约定幅度（如 ≥10%）且可复现时，才让 asm 进默认路径。

## 下一步（按优先级）

- **ARM64 硬件/QEMU**：跑 `asm-arm64` 的正确性与 NEON 基准；没有执行数据前不写“NEON 更快”。
- **AVX-512 硬件/Intel SDE**：跑 `src/avx512.rs`；再考虑 `vpternlogd`（K3/Belnap）与 `vpermb`（pack5）。
- **SVE**：等 `stdarch_aarch64_sve` 稳定 + SVE 硬件。
- **unpack 的 `pdep` / AVX-512 VBMI 版本**：当前 asm LUT 是 1.3–1.5×，看更小的表/更宽的查表能否再赢。
- **合并门槛**：正确性全过、同算法同数据、比 intrinsics 基线至少快一个预先约定的幅度（例如 ≥10%）、跨大小/跨机器可复现；否则保留 intrinsics。

## 复现

```bash
# x86-64：K3 / pack5 / GF(3) / Belnap4；AVX-512 路径自动跳过
cd other-languages/asm
cargo run --release

# 互操作 conformance：pack5（644 pass / 0 fail / 15 skipped）
cargo run --release -- --conformance
# 当前只覆盖 pack5；K3/Belnap4 语义向量待补（由 runner/Go 覆盖）

# ARM64：只做交叉编译检查（无硬件运行）
cd other-languages/asm-arm64
cargo check --target aarch64-unknown-linux-gnu
```

文件：`src/main.rs`（K3）、`src/pack5.rs`、`src/gf3.rs`、`src/belnap4.rs`、`src/avx512.rs`、`asm-arm64/src/main.rs`。两个 crate 都是独立 workspace，不进入主 Cargo 工作区，也不参与默认 CI。
