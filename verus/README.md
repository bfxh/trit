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
BULK: trit-core::bulk::TriVec          ≡  bulk.rs     （spec/exec + Verus 整向量证明 + tests/bridge_bulk.rs）
POINT: trit-core::bulk::TriVec 单点     ≡  point.rs    （get/set/from_ternary 视图等价 + tests/bridge_point.rs）
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
- `src/bulk.rs`：位切片向量 `TriVec` 的 spec/exec（`not` / `and` / `or` 的 K3·Ł3 格序与 B3 毒值），
  以及结构不变式 `wf`（两个平面在 `len` 位内互斥）；
- `src/bulk/proofs.rs`：整向量等价证明——结果视图逐位等于标量 `knot` / `tand` / `tor` /
  `poison_and` / `poison_or`；
- `src/point.rs`：单点 `get_at` / `set_planes` 的 spec/exec 与视图口径
  `set_at_spec`（掩码 `mask_word` 显式参数化）；
- `src/point/from_ternary.rs`：`from_ternary_planes` 逐点打包 + `prefix` 循环不变式；
- `src/point/wf_lemmas.rs`：`wf` 保持性 lemma（`set_word_wf` / `wf_none`）；
- `src/point/proofs.rs`：单点写的位级 lemma（`mask_bit_at` / `bit_self` /
  `tri_of_word_ext` / 命中与未命中的三值形式）与 `set_view_equiv`；
- `tests/bridge_bulk.rs`：边界长度（0/1/63/64/65/130）× 三种代数，对拍主库 `tri_core::TriVec`
  与标量 `Ternary`，并覆盖 padding 掩码与二值/未知混合；
- `tests/bridge_point.rs`：get / try_get / try_set / from_ternary 与标量逐位对拍，
  含越界错误、`unknowns()` 归一化与 padding。

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

已验证（有限域穷举 + Verus，当前 `cargo-verus verify` 为 188 verified）：

- K3：`!` 对合、`&`/`|` 交换与 De Morgan、`teq` 自反/互补、`rank` 单射；
- Ł3：格序 `∧/∨`、强加/强合交换、`U⊕U=T`、`U⊗U=F`、`U⇒U=T`、De Morgan、等价自反；
- B3：毒值吸收（任一 `U` → `U`）、`∧/∨` 交换、De Morgan、`U⇒U=U`；
- B4：双轨往返、否定对合、De Morgan、结合/分配/吸收、`xor=¬equiv`、
  consensus/gullibility 交换、conflation 对合与知识序 De Morgan；
- 位平面 `TriBit`：`not` / `and` / `or` 的 Verus 逐位等价（8×8×64 对拍保留为回归）；
- 位切片向量 `TriVec`：`not` / `and` / `or`（K3·Ł3 格序 + B3 毒值）的整向量等价，
  即 `view(len, out_planes, unknowns) == Seq::new(len, |i| scalar_op(a[i], b[i]))`；
- 位切片向量单点操作：`get_at`（读一位 == `tri_at`）、`set_planes`（写一位后视图 ==
  `set_at_spec`，含「清空最后一个 `Unknown` 后归一为 `None`」的分支）、
  `from_ternary_planes`（逐位写 == 标量序列），且三条 exec 都保持结构不变式
  `wf`（两平面在 `len` 位内互斥；`point/wf_lemmas.rs::set_word_wf` / `wf_none`），由 `tests/bridge_point.rs`
  与主库对拍（含越界错误与 `unknowns()` 归一化）。

**未验证**（明确缺口，不当作已完成）：

- 批量运算的 padding 掩码：`bulk.rs` 的 exec 省略了 `len` 之外的掩码写入（`view` 不读 padding），
  padding 清零由 `tests/bridge_point.rs` / `tests/bridge_bulk.rs` 对拍主库覆盖；
- 任何并发或 `unsafe` 代码；
- Verus 本身的可靠性、z3 模型与 Rust 编译器的可信基。

## 为什么独立 workspace

`verus/` 是独立 workspace（`publish = false`），不并入主库 `Cargo.toml`：

- 主库的 stable/MSRV 构建不引入 `vstd`，不改变主库 `Cargo.lock`；
- 验证由 `scripts/verify.py` 与单独的 CI job 驱动，不会拖慢 `cargo test --workspace`；
- 下游项目要获得同一保证，依赖主库的 `trit-core`（同一份代码）即可；
  需要本地复核时复用 `scripts/verify.py`。
