# AArch64 NEON 参考验证（编译检查）

状态：**只做交叉编译检查**；本机没有 ARM64 硬件/QEMU，因此没有运行正确性或性能读数。

## 范围

- `GF(3) add/mul`：NEON `vqtbl1q_u8` 16 项查表，对标 scalar LUT。
- 输入输出为字节 `0/1/2`；`add` 索引 `a+b`，`mul` 索引 `3a+b`。
- 代码：[src/main.rs](src/main.rs)。

## 检查命令

```bash
cd other-languages/asm-arm64
cargo check --target aarch64-unknown-linux-gnu
```

在 x86-64 主机上直接 `cargo check` 会触发 `compile_error!`，这是刻意的：该 crate 只面向 AArch64。

## 运行条件

```bash
# 需要 AArch64 链接器/运行环境；当前机器没有
cargo run --release --target aarch64-unknown-linux-gnu
```

## SVE

SVE 暂不实现：

- 本机无 SVE 硬件，也没有可用的 SVE 模拟器；
- 稳定 Rust 1.99 的 SVE intrinsics 仍是 `stdarch_aarch64_sve` 不稳定特性（rust-lang/rust#145052）。

先在 ARM64 硬件上跑 NEON；SVE 等稳定工具链和硬件到位后再做。

## 证据等级

- **编译检查**：`cargo check --target aarch64-unknown-linux-gnu` 通过。
- **未运行**：没有 ARM64 执行证据，不写性能数字，不当成“NEON 更快”的结论。
