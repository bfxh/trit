# Lean 4 形式化（interop v0 语义）

本目录把 `interop/spec/v0.md` 的核心语义写成 Lean 4 定理，作为 Rust/Go/汇编之外的**形式语义锚点**。

- Lean：`4.35.0`（`lean-toolchain` 固定）
- 依赖：仅 Lean 核心；不依赖 Mathlib
- 构建：`cd interop/lean && lake build`
- 数据桥：`lake exe tritLean > /tmp/lean-vectors.jsonl`，再由 `interop/runner -- --lean ...` 对比 golden vectors
- CI：`conformance` workflow 的 `lean 4 semantics` job（2026-11-08 已通过；含 Lean→Rust 桥）

## 文件

| 文件 | 内容 |
|---|---|
| `Trit/K3.lean` | K3 的 NOT/AND/OR、交换律/结合律、位平面 encode/decode、`KGate` 表与 `gate U T ≠ implies U T` |
| `Trit/Belnap4.lean` | Belnap4 的 NOT/AND/OR、位平面公式、`N∧B=F`、`N∨B=T` 等关键值 |
| `Trit/Pack5.lean` | 5-trit `pack`/`unpack` 往返、尾部填充、错误语义、`pack < 243` |
| `Main.lean` | `tritLean` 可执行：输出 738 条 conformance JSONL |
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
- `Pack5.unpackGroup_packGroup1..5`（尾部 1–4 与完整 5 的往返）
- `Pack5.unpackGroup_invalidByte`、`Pack5.unpackGroup_shortInput`（错误语义）
- `Pack5.ofInt_neg_one`、`Pack5.ofInt_two`（`invalid_trit` 的类型外入口）

> `Pack5` 的有限枚举证明已从 `native_decide` 换成 `decide`（kernel 检查）；没有 `sorry`。

## Lean→Rust 数据桥

- Lean 端：`lake exe tritLean` 按 golden vector 的 `id` 输出 738 条 JSONL（`expected` 或 `error`）。
- Rust 端：`interop/runner -- --lean <file>` 逐条与 `interop/vectors` 对比；缺项、多项、值不一致都判红。
- 这是**数据级**桥：Rust 调 Lean 可执行产物并比对，不是 Lean runtime FFI；后者仍未做。

## 未覆盖

- 跨进程容器 v0.2、Belnap 5/9 值、SAT/BDD/ATPG：不在本目录范围。
- Lean runtime FFI（Rust 直接调用 Lean 编译产物）：未做；当前是数据级 conformance 桥。

## 与 `openai/math` 的关系

Lean 侧保持**无 Mathlib 依赖**，保证 CI 可复现；`openai/math` 的 Lean 语料是未来的应用/评测目标，不是本目录的依赖。后续可以在不改变本目录核心定理的前提下，把更完整的数学/证明语料接进来。
