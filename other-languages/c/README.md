# C 参考实现（header-only，core profile）

本目录是 `interop/spec/v0.md` 的 C99 参考切片：K3、`KGate`、pack5。非默认实现，不进入 Cargo 工作区，不被 Rust crate 依赖。

## 范围

| 能力 | API | 状态 |
|---|---|---|
| K3 NOT/AND/OR | `k3_not` / `k3_and` / `k3_or` | 已实现 |
| KGate 准入表 | `k3_gate` | 已实现 |
| K3 位平面 | `k3_encode_bits` / `k3_decode_bits` | 已实现，`(1,1)` 返回 `invalid_bitplane` |
| 5 trits/byte | `trit_pack5` / `trit_unpack5` | 已实现，字节 `>=243` 返回 `invalid_byte` |
| pack5 尾部 | `trit_pack5_tail` / `trit_unpack5_tail` | 已实现，缺失位置填 trit 0；`count>5` 返回 `short_input` |
| 非法 trit | `trit_pack5` / `trit_pack5_tail` | 返回 `invalid_trit` |
| Belnap4 | — | 未实现（opt-in profile，不在本切片范围） |

- 无分配、无全局状态、无外部依赖；所有函数为 `static inline`，直接 include `trit.h`。
- 错误类别与 spec 一致：`invalid_bitplane` / `invalid_byte` / `short_input` / `invalid_trit`。
- 不做 SAT/BDD/ATPG、容器格式或 SIMD；性能结论不在本目录声明。

## 构建与测试

```sh
cd other-languages/c
make test          # K3 真值表、243 组 pack5、全部尾部、错误路径
make conformance   # 生成 build/c-vectors.jsonl（694 条 core 向量）
```

Windows 本地没有 `make` 时可直接：

```sh
gcc -std=c99 -O2 -Wall -Wextra -Werror -pedantic -o build/test_trit.exe test_trit.c
./build/test_trit.exe
gcc -std=c99 -O2 -Wall -Wextra -Werror -pedantic -o build/trit_conformance.exe conformance.c
./build/trit_conformance.exe > build/c-vectors.jsonl
```

## Conformance

```sh
cd interop/runner
cargo run --release -- --bridge ../../other-languages/c/build/c-vectors.jsonl --profiles k3,kgate,pack5
```

- 结果：**694/694**（`k3` 28、`kgate` 9、`pack5` 657）。
- `--profiles` 必须显式声明；桥不会静默跳过缺失 profile，未声明的 profile 不参与比较。
- 全量 738 条（含 Belnap4）由 Lean 桥覆盖；C 若要声明 Belnap4 conformance，需先实现该 profile。

## 与 Rust/Lean 的关系

- 语义以 `interop/spec/v0.md` 为准，Rust runner 是参考实现，Lean 4 是形式语义锚点。
- C 是**可移植 ABI / 嵌入式 / WASM 入口**的最小切片；不是 Rust 的替代，也不承担性能承诺。
- 在二进制 ISA 上，pack5/K3 位平面属于 B 类（二进制模拟三态）；任何性能结论都必须给 Rust/C 同算法基线。
