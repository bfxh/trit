# trit-verified：主库三值语义的验证锚点

`trit-verified` 用 [Verus](https://github.com/verus-lang/verus) 为 `trit-core` 的三值语义
（K3 / Ł3 / B3）与 `trit-belnap` 的四值语义（B4：N/F/T/B）建立机器检查的规格与证明。它是**验证锚点**，不是运行时依赖：
主库的 `cargo build/test` 不构建它，验证由 `scripts/verify.py` 单独驱动，CI 走
`.github/workflows/verus.yml`。

## 验证链

```text
K3:  trit-core::value::Ternary        ≡  k3_impl.rs  （scripts/check_k3_impl.py，函数体逐字）
     k3_impl.rs                      ≡  spec        （src/k3.rs，Verus ensures）
     trit-core::value::Ternary        ≡  table.rs    （tests/bridge.rs，3×3，冗余检查）
Ł3:  trit-core::algebra::Lukasiewicz  ≡  l3.rs       （tests/bridge_algebra.rs，3×3）
B3:  trit-core::algebra::Bochvar      ≡  b3.rs       （tests/bridge_algebra.rs，3×3）
B4:  trit-belnap::B4                  ≡  b4.rs       （tests/bridge_algebra.rs，4×4）
BITS: trit-core::bits::TriBit          ≡  bits.rs     （spec/exec + Verus 逐位证明 + 8×8×64 对拍）
每行：执行版 ≡ spec（src/*.rs 的 Verus ensures）
```

- `src/table.rs`：`NOT` / `AND` / `OR` / `EQ` / `XOR` 的编码真值表（0=F、1=U、2=T），
  是普通测试与 Verus 规格共用的单一来源。
- `src/k3.rs`：`spec fn knot/tand/tor/teq/tne/rank`、代数律证明（双重否定、
  交换律、De Morgan、`teq` 自反、`rank` 单射）、`table_*_matches_spec`、
  以及 exec 提炼（`knot_exec` 等带 `ensures`）。
- `src/k3_impl.rs`：主库 `value.rs` 的 K3 算子实现体（同步门校验，逐字）。
- `tests/bridge.rs`：对拍 `trit-core` 的 `!`/`&`/`|`/`^` 与 `index()`/`from_index()`。
- `src/l3.rs`：Ł3 的 spec（双倍刻度 `0/1/2` 精确表达 `0/0.5/1`）、代数律、exec。
- `src/b3.rs`：B3 的毒值吸收、De Morgan、`U ⇒ U = U`、exec。
- `src/b4.rs`：Belnap 四值的双轨 `bool` 表示、De Morgan/结合/分配/吸收、
  `xor = ¬equiv`、知识序共识/轻信/共轭与 exec。
- `tests/bridge_algebra.rs`：对拍 `Algebra::{Lukasiewicz, Bochvar}` 与 `trit_belnap::B4`。
- `src/bits.rs`：位平面 `TriBit` 的 spec/exec（`not` / `and` / `or`），与主库位平面公式一致；
- `src/bits/proofs.rs`：位级参数化 lemma 与 `not` / `and` / `or` 的逐位等价证明。
- `tests/bridge_bits.rs`：8 组边界平面 × 64 位 × 3 种代数，逐位对拍 `TriBit` 与标量 `Ternary`。

## 本地运行

```bash
# 1) 安装 Verus（固定版本 + 官方 SHA-256 校验）
python scripts/verify.py --install          # 下载 Linux/Windows release 到 ~/tools/verus

# 2) 验证
python scripts/verify.py                    # 同步门 + 检查工具链 + cargo-verus verify + cargo test
python scripts/verify.py --only sync        # 只跑实现体同步门（不需要 Verus）
python scripts/verify.py --json             # 机器可读报告
```

`scripts/verify.py` 会读取：

- `TRIT_VERUS_DIR`：Verus 解压目录（默认 `~/tools/verus-x86-linux`）；
- `TRIT_Z3_DIR`：Z3 4.16.0 解压目录（默认 `~/tools/z3-4.16.0-x64-glibc-2.39`）；
- `TRIT_VERUS_TOOLCHAIN`：rustup toolchain（默认 `1.100.0-x86_64-unknown-linux-gnu`）。

## 范围与限制

已验证（有限域穷举 + Verus，当前 `cargo-verus verify` 为 120 verified）：

- K3：`!` 对合、`&`/`|` 交换与 De Morgan、`teq` 自反/互补、`rank` 单射；
- Ł3：格序 `∧/∨`、强加/强合交换、`U⊕U=T`、`U⊗U=F`、`U⇒U=T`、De Morgan、等价自反；
- B3：毒值吸收（任一 `U` → `U`）、`∧/∨` 交换、De Morgan、`U⇒U=U`；
- B4：双轨往返、否定对合、De Morgan、结合/分配/吸收、`xor=¬equiv`、
  consensus/gullibility 交换、conflation 对合与知识序 De Morgan；
- 位平面 `TriBit`：`not` / `and` / `or` 的 Verus 逐位等价（8×8×64 对拍保留为回归）。

**未验证**（明确缺口，不当作已完成）：

- `bulk`（`TriVec`）与标量实现的等价；
- 任何并发或 `unsafe` 代码；
- Verus 本身的可靠性、z3 模型与 Rust 编译器的可信基。

## 为什么独立 workspace

`verus/` 是独立 workspace（`publish = false`），不并入主库 `Cargo.toml`：

- 主库的 stable/MSRV 构建不引入 `vstd`，不改变主库 `Cargo.lock`；
- 验证由 `scripts/verify.py` 与单独的 CI job 驱动，不会拖慢 `cargo test --workspace`；
- 下游项目要获得同一保证，依赖主库的 `trit-core`（同一份代码）即可；
  需要本地复核时复用 `scripts/verify.py`。
