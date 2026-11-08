# Lean 4 形式化（interop v0 语义）

本目录把 `interop/spec/v0.md` 的核心语义写成 Lean 4 定理，作为 Rust/Go/汇编之外的**形式语义锚点**。

- Lean：`4.35.0`（`lean-toolchain` 固定）
- 依赖：仅 Lean 核心；不依赖 Mathlib
- 构建：`cd interop/lean && lake build`
- CI：`conformance` workflow 的 `lean 4 semantics` job

## 文件

| 文件 | 内容 |
|---|---|
| `Trit/K3.lean` | K3 的 NOT/AND/OR、交换律/结合律、位平面 encode/decode、`KGate` 表与 `gate U T ≠ implies U T` |
| `Trit/Belnap4.lean` | Belnap4 的 NOT/AND/OR、位平面公式、`N∧B=F`、`N∨B=T` 等关键值 |
| `Trit/Pack5.lean` | 5-trit `pack`/`unpack` 往返定理，`pack < 243` 上界 |
| `Trit.lean` | 汇总导入 |

## 已证明

- `K3.not_not`、`K3.and_comm`、`K3.or_comm`、`K3.and_assoc`、`K3.or_assoc`
- `K3.ofBits_toBits`（合法编码往返）、`K3.ofBits_invalid`（`(1,1)` 非法）
- `K3.andBits_correct` / `orBits_correct` / `notBits_correct`（位平面公式 = 语义运算）
- `K3.gate_F` / `gate_U` / `gate_T`（准入表三行）
- `K3.gate_not_implies`（`required=U, verified=T` 仍为 `U`）
- `B4.not_involutive`、`B4.and_comm`、`B4.or_comm`、`B4.and_N_B`、`B4.or_N_B`
- `B4.andBits_correct` / `orBits_correct` / `notBits_correct`
- `Pack5.unpack_pack`（243 种 5-trit 组合全覆盖）、`Pack5.pack_lt_243`

## 未覆盖

- pack5 尾部填充、非法字节/`short_input` 的错误语义：由 `interop/vectors/pack5.json` 与 Rust runner/Go/asm conformance 覆盖。
- 跨进程容器 v0.2、Belnap 5/9 值、SAT/BDD/ATPG：不在本目录范围。
- Lean ↔ Rust/Go 的运行时桥：尚未做；当前是语义与证明层，不是可执行后端。

## 与 `openai/math` 的关系

Lean 侧保持**无 Mathlib 依赖**，保证 CI 可复现；`openai/math` 的 Lean 语料是未来的应用/评测目标，不是本目录的依赖。后续可以在不改变本目录核心定理的前提下，把更完整的数学/证明语料接进来。
