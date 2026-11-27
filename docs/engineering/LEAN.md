# Lean 4 证明锚点：小仓库如何直证 TriVec

> 状态：**第二个锚点（模型层）**。`interop/lean` 这个零依赖小仓库已经证明
> `TriVec` 的单点操作与位平面公式；代码层（真实 `u64` 字与循环）由
> `verus/` 锚点证明，两者证的是同一句话。数据桥（bulk profile）是下一轮。

## 1. 一句话结论

`interop/lean` 是**一个独立的小 Lake 包**（固定 Lean 4.35.0、零 Mathlib、
7 个构建目标、`Trit/Bulk.lean` 约 330 行），它不需要把 `trit-core` 搬进 Lean，
也不需要枚举 2^n 个位模式，就能对**所有长度、所有下标**证明：

```text
view (fromTernary xs) i        = xs[i]                      -- from_ternary 往返
view (set v i t) i             = t                          -- 写后读（同址）
view (set v i t) j             = view v j        (j ≠ i)     -- 写不影响他址
wf v                           ⇒ wf (set v i t)              -- 写维持结构不变式
view (andv a b) i              = K3.and (view a i) (view b i)
view (orv a b) i               = K3.or  (view a i) (view b i)
view (notv v) i                = K3.not (view v i)
view (andvB3 a b) i            = poisonAnd (view a i) (view b i)
view (orvB3 a b) i             = poisonOr  (view a i) (view b i)
```

这就是“小仓库直证项目”的依据：**三值语义的模型是符号化的**——一条
`∀ i, i < len → …` 的定理覆盖任意长度，不需要样本；位平面公式是对
`Bool` 的等式，逐位成立。需要“真实 64 位字、循环、`Vec`/`Option` 分配”的
部分才需要代码级证明，那正是 Verus 锚点的战场（第 3 节）。

## 2. 小仓库的结构

```text
interop/lean/               独立 Lake 包（lean-toolchain = 4.35.0）
├── lakefile.lean           lean_lib Trit + lean_exe tritLean
├── Trit/K3.lean            K3 语义、位平面 encode/decode、andBits/orBits/notBits
├── Trit/Belnap4.lean       Belnap 四值语义与位平面公式
├── Trit/Pack5.lean         5-trit pack/unpack 往返与错误语义
├── Trit/Bulk.lean          TriVec 模型：view/fromTernary/get/set + 平面公式（本轮）
├── Main.lean               tritLean：输出 738 条 conformance JSONL
└── Trit.lean               汇总导入
```

`Bulk.lean` 的模型只有三个字段：`len : Nat`、`ones : Nat → Bool`、
`unk : Nat → Bool`。位索引 `i` 就是主库的 `(ones[i / 64] >> (i % 64)) & 1`；
**64 位字的打包/掩码不在 Lean 里建模**，由 Verus 证（见 §3 的分工）。
`wf` 是主库 `from_ternary` / `try_set` 维持的性质：`len` 位内两平面互斥。

## 3. 三个锚点如何拼成一条链

```text
                      ┌──────────────────────────────┐
                      │  语义（K3 / Ł3 / B3 / B4）    │
                      └───────────┬──────────────────┘
              ┌───────────────────┴────────────────────┐
              │                                        │
   ┌──────────▼───────────┐                  ┌─────────▼──────────┐
   │ Lean（模型层，本文件）│                  │ Verus（代码层）     │
   │ Trit/Bulk.lean        │                  │ verus/src/bulk.rs  │
   │ 证：模型 ⊨ 规格       │                  │ 证：u64 代码 ⊨ 模型 │
   │ ∀ 长度、∀ 下标        │                  │ ∀ 字数、∀ 位         │
   └──────────┬───────────┘                  └─────────┬──────────┘
              │                                        │
              └───────────────┬────────────────────────┘
                              │
                   ┌──────────▼───────────┐
                   │ 数据桥（interop）     │
                   │ golden vectors 对拍   │
                   │ tritLean ↔ trit-core  │
                   └──────────────────────┘
```

| 层 | 文件 | 证什么 | 覆盖量 |
|---|---|---|---|
| Lean 模型 | `interop/lean/Trit/Bulk.lean` | `view/fromTernary/get/set` 的律、平面公式 ≡ 标量、`wf` 维持 | 35 条定理，∀ `len`/`i` |
| Verus 代码 | `verus/src/bulk.rs` + `bulk/proofs.rs` | 真实 `u64` 平面与 `Vec` 循环的结果视图 ≡ 标量 | 见 `verus/README.md` 的 verified 计数 |
| 数据桥 | `interop/lean/Main.lean` + `interop/runner` | 两边可执行结果逐条一致（K3/B4/Pack5 共 738 条） | 738/738；bulk 下一轮 |

三层不是互相替代：Lean 不建模 `u64` 打包与 `Vec` 分配，Verus 不建模
Lean 的归纳定义；数据桥把“两边说的是同一件事”变成可执行检查。

## 4. 本轮新增的定理（`Trit/Bulk.lean`）

### 4.1 表示与基础

`not_both_isT_isU`、`ofBitsView_encode`、`ofBitsView_toBits`、`ofBitsView_eq`、
`toBits_ofBits`、`bits_of_wf`。

`bits_of_wf` 是桥梁：`wf` 状态下第 `i` 位的原始位对就是
`toBits (view v i)`，后面所有“位公式 ≡ 标量”的证明都复用它。

### 4.2 单点操作（对应 `from_ternary` / `try_get` / `try_set`）

| 定理 | 内容 |
|---|---|
| `fromTernary_wf` | `from_ternary` 的输出满足 `wf` |
| `fromTernary_view` | `view (fromTernary xs) i = xs[i]`（`i < len`） |
| `set_len` / `set_bits_self` | `set` 不改长度；写后第 `i` 位就是 `toBits t` |
| `set_view_self` | 写后读同址 = 写入值 |
| `set_view_other` | 写后读他址 = 原值（frame） |
| `set_wf` | `wf` 下写不破坏互斥 |
| `set_view_fromTernary(_other/_out)` | `from_ternary` 后写入的三条视图律（含越界读回 `F`） |
| `get?_set?_same` / `get?_set?_other` | 带边界检查版本（对应 `try_get`/`try_set`） |
| `set?_out_of_range` | 越界写返回 `none`，不改状态 |

### 4.3 位平面公式 ≡ 标量

`andBits_valid` / `orBits_valid` / `notBits_valid`（公式输出永远是合法编码）、
`falseBit_eq_or`（`falseBit = !(ones || unknowns)`，与主库 `!(o | x)` 同值）、
`andv_view` / `orv_view` / `notv_view` / `notv_unk_eq` / `notv_ones_eq_code`、
`andvB3_view` / `orvB3_view`，以及四个 `wf` 维持定理。

`K3.andBits` 的公式与 `crates/trit-core/src/bulk.rs` 的
`o_out = o1 & o2`、`f_out = (!o1 & !x1) | (!o2 & !x2)`、`x_out = !(o_out | f_out)`
逐字同形；`notBits` 的 `falseBit` 与主库 `!(ones | unknowns)` 在 `wf` 下同值
（`falseBit_eq_or` + `notv_ones_eq_code`）。B3 的 `x_out = x1 | x2`、
`ones = (o1 & o2) & !x_out` 同样逐字同形。

## 5. 运行与复核

```bash
cd interop/lean
lake build                                   # 7 个目标；无 sorry、无 Mathlib
lake exe tritLean > /tmp/lean-vectors.jsonl  # 738 条 conformance JSONL
cd ../runner && cargo run --release -- --bridge /tmp/lean-vectors.jsonl
```

CI：`.github/workflows/conformance.yml` 的 `lean 4 semantics` job
（`leanprover/lean-action` → `lake build` → `lake exe tritLean` → `--bridge`）。

复核“无额外公理”：

```bash
cd interop/lean
cat > /tmp/Audit.lean <<'EOF'
import Trit
#print axioms TritFormal.andv_view
#print axioms TritFormal.fromTernary_view
#print axioms TritFormal.set_wf
EOF
lake env lean /tmp/Audit.lean
```

实测输出（2026-11-26，本机 Lean 4.35.0）：

```text
'TritFormal.andv_view' depends on axioms: [propext]
'TritFormal.fromTernary_view' depends on axioms: [propext]
'TritFormal.set_wf' depends on axioms: [propext]
```

没有 `sorryAx`，也没有 `Classical.choice` / `Quot.sound`。

## 6. 未证 / 不在本仓库范围

- **bulk 的数据桥**：`Main.lean` 还没有输出 `TriVec` 向量，`interop/vectors/`
  没有 `bulk.json`，runner 没有 `bulk` profile。下一轮按 K3 的同一模板加三处
  （`vectors/bulk.json`、`schema` 的 profile 枚举、`runner` 的 `run_bulk`），
  把“`from_ternary → get/set → and/or/not` 的端到端结果”变成 700+ 条对拍。
- **`u64` 打包、padding 掩码、`usize` 溢出**：Lean 模型按无限位平面，
  不建模 64 位窗口与 `len` 外清零；这些由 `verus/` 与 `tests/bridge_bulk.rs` 覆盖。
- **`Vec` / `Option` 的分配与别名**：模型是纯函数，不建模
  `unknowns.get_or_insert_with` 与“清空后置 `None`”的表示归一化；
  Verus 侧按 `wf`（允许 `Some(全零)`）证明视图不变。
- **`count_unknowns` / `to_bools` / `PartialEq`**：不在本轮定理集。
- **可信基**：Lean 内核、编译器、以及 `lake` 的构建；与 Verus 的 SMT/z3
  可信基是两套独立假设，这正是双锚点的价值。

## 7. 路线图

1. **Bulk 数据桥**：`vectors/bulk.json` + schema + runner，纳入 `conformance.yml`。
2. **公理审计**：CI 增加 `#print axioms` 步骤，禁止 `sorryAx` 进入主分支。
3. **对齐表**：把“Lean 定理名 ↔ Verus 证明名 ↔ 主库函数”的对应写成
   `docs/engineering/VERIFICATION.md` 的一张三列对照表（人工维护，CI 检查名字存在）。
4. **SIMD**：见 `VERIFICATION.md` §7 的口径拆分——位级掩码可证，浮点
   AVX2 内核需要 Kani 一类位精确浮点工具，不在 Lean/Verus 的能力范围。
