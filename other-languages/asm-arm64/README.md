# AArch64 NEON 参考验证（CI 实机）

状态：本地 x86-64 只做交叉编译检查；GitHub Actions `ubuntu-24.04-arm` 已实机运行正确性与 NEON 基准。

## 范围

- `GF(3) add/mul`：NEON `vqtbl1q_u8` 16 项查表，对标 scalar LUT。
- 输入输出为字节 `0/1/2`；`add` 索引 `a+b`，`mul` 索引 `3a+b`。
- 代码：[src/main.rs](src/main.rs)。

## CI 实测（2026-11-08，ubuntu-24.04-arm）

- `arch=aarch64 os=linux`
- 正确性：20 种长度 + 9 真值对全过
- GF(3) add：scalar 1.075 ns/B，NEON 0.047 ns/B，**23.03×**
- GF(3) mul：scalar 1.073 ns/B，NEON 0.047 ns/B，**23.02×**

> CI runner 型号/频率未记录；这是单次 CI 读数，不跨机承诺。

## 检查命令

```bash
cd other-languages/asm-arm64
cargo check --target aarch64-unknown-linux-gnu
```

在 x86-64 主机上直接 `cargo check` 会触发 `compile_error!`，这是刻意的：该 crate 只面向 AArch64。

## 运行

CI 的 `conformance` workflow `arm64-neon` job 在 `ubuntu-24.04-arm` 上执行：

```bash
cd other-languages/asm-arm64
cargo run --release
```

本地 x86-64 只能做交叉编译检查，不能运行。

## SVE

SVE 暂不实现：

- 本机无 SVE 硬件，也没有可用的 SVE 模拟器；
- 稳定 Rust 1.99 的 SVE intrinsics 仍是 `stdarch_aarch64_sve` 不稳定特性（rust-lang/rust#145052）。

先在 ARM64 硬件上跑 NEON；SVE 等稳定工具链和硬件到位后再做。

## 证据等级

- **编译检查**：本地 `cargo check --target aarch64-unknown-linux-gnu` 通过。
- **CI 实机**：`ubuntu-24.04-arm` 上 `cargo run --release` 通过，读数见上。
- **SVE**：未实机验证，不写性能结论。
