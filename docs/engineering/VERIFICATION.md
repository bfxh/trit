# 形式化验证：主库三值语义（Verus）

> 状态：**M2 锚点**。K3 / Ł3 / B3 三值与 Belnap 四值的标量算子已有机器检查的
> 规格与证明；位平面/批量实现、并发与 `unsafe` 仍是缺口（见文末）。

## 1. 目标与边界

主库的三值语义是所有下游共享的稳定 API。测试只能抽样，Verus 用 SMT 对
**所有输入**检查规格。当前覆盖 `trit-core::value::Ternary` 的 K3、
`trit-core::algebra` 的 Ł3/B3，以及 `trit-belnap::B4` 的标量算子，
不改变运行时依赖、不改变 `cargo test --workspace` 的口径。

验证锚点是 `verus/`（独立 workspace，`publish = false`）：

```text
verus/
├── Cargo.toml        trit-verified：vstd + trit-core；独立 workspace
├── README.md         范围、运行方式、缺口
├── src/table.rs      K3 编码真值表（单一来源）
├── src/k3.rs         K3：Verus spec + 代数律 + exec
├── src/k3_impl.rs    主库 value.rs 的 K3 实现体（scripts/check_k3_impl.py 同步门）
├── src/l3.rs         Ł3：双倍刻度 spec + 代数律 + exec
├── src/b3.rs         B3：毒值吸收 spec + 代数律 + exec
├── src/b4.rs         Belnap 四值：双轨 bool spec + 代数律 + exec
├── src/bits.rs       位平面 TriBit 的 spec/exec（not/and/or）
├── src/lib.rs        模块出口
├── tests/bridge.rs   对拍 trit-core 的 K3
└── tests/bridge_algebra.rs  对拍 algebra 的 Ł3/B3 与 trit-belnap::B4
```

## 2. 信任链

```text
K3:  trit-core::value::Ternary        ≡  k3_impl.rs  （scripts/check_k3_impl.py，函数体逐字）
     k3_impl.rs                      ≡  spec        （src/k3.rs，Verus ensures）
Ł3:  trit-core::algebra::Lukasiewicz  ≡  l3.rs       （3×3）
B3:  trit-core::algebra::Bochvar      ≡  b3.rs       （3×3）
B4:  trit-belnap::B4                  ≡  b4.rs       （4×4）
BITS: trit-core::bits::TriBit          ≡  bits.rs     （spec/exec + Verus 逐位证明 + 8×8×64 对拍）
BULK: trit-core::bulk::TriVec          ≡  bulk.rs     （spec/exec + Verus 整向量证明 + tests/bridge_bulk.rs）
POINT: trit-core::bulk::TriVec 单点     ≡  point.rs    （get/set/from_ternary 视图等价 + tests/bridge_point.rs）
每行：执行版 ≡ spec（Verus ensures）
```

- `bridge.rs` 是穷举而非抽样：三值只有 3×3 组。
- `table.rs` 与 Verus 规格共用同一张表，避免“证明一份、运行另一份”。
- Verus 的 `exec` 版本（`knot_exec` 等）带 `ensures r == spec(...)`，
  证明可执行提炼与规格一致。

## 3. 运行

### 本地（WSL / Linux）

```bash
# 一次性：下载固定版本 Verus 0.2026.11.22.1ce6a59 + Z3 4.16.0（SHA-256 校验）
python scripts/verify.py --install

# 每次：检查工具链 → cargo-verus verify → bridge 对拍
python scripts/verify.py
python scripts/verify.py --json            # 机器可读
python scripts/verify.py --only verify     # 只跑证明
```

依赖：

- `rustup toolchain 1.100.0-x86_64-unknown-linux-gnu`（`scripts/verify.py` 会检查）；
- `TRIT_VERUS_DIR` / `TRIT_Z3_DIR` / `TRIT_VERUS_TOOLCHAIN` 可覆盖默认路径。

Windows 原生 release 需要 MSVC linker（Visual Studio Build Tools）；
没有时在 WSL 里跑同一脚本。

### CI

`.github/workflows/verus.yml`：ubuntu-latest，装 1.100.0，下载固定版本
Verus（校验 SHA-256），跑 `scripts/verify.py --json` 并上传报告。

## 4. 下游如何继承

- **代码**：下游依赖主库的 `trit-core`，拿到的是同一份实现（复核入口 `scripts/verify.py`）；
  验证锚点不进入运行时依赖。
- **复核**：下游在自己的 CI 里调用 `python /path/to/trit/scripts/verify.py`，
  或在 PR 模板中引用 `verus` workflow 的结论。
- **发布**：主库 tag 的 CI 必须包含 `verus` job；下游锁主库版本即锁验证结论。

“所有依赖主库的都依赖它”在这里的含义是：**验证只做一次，结论随主库版本传播**，
而不是每个下游各复制一份三值逻辑再各自证明。

### 4.2 单点操作（`get` / `set` / `from_ternary`）

`verus/src/point.rs` 新增三条 exec 与配套 spec（位级 lemma 在 `point/proofs.rs`）：

| exec | 语义口径 | 证明位置 |
|------|----------|----------|
| `get_at(len, ones, unknowns, i)` | 第 `i` 位 == `tri_at(ones@, unknowns_view, i)` | `point.rs`（`bit_at_extract` 对齐 `(w >> b) & 1`） |
| `set_planes(len, ones, unknowns, i, v)` | 结果视图 == `set_at_spec`：第 `i` 位换成 `v`、其余不变 | `point/proofs.rs::set_view_equiv` |
| `from_ternary_planes(values)` | 结果视图 == `values@` | 逐点调用 `set_planes` + `prefix` 循环不变式 |

关键设计：把「第 `i` 位的掩码」显式写成参数
`mask_word(w, word, bit) = if w == word { bit } else { 0u64 }`，
位级推理只落在 `u64` 上（`int` 位移不进 bit-vector 求解）；
`mask_bit_at` 证 `bit_at(mask_word(...), b) == (w == word && bit_at(bit, b))`，
`bit_self` 证 `bit == 1 << s` 时 `bit_at(bit, b) == (b == s)`。

诚实边界：`wf`（两平面互斥）未随单点写一起证；`from_ternary` 的 exec 用逐点调用
`set_planes` 代替主库的惰性 `get_or_insert_with`（语义一致，验证工作区不追性能）；
padding 位不在 `view` 语义内，由 `tests/bridge_point.rs` 对拍主库覆盖。
单点操作同时补了运行时对拍：`get`/`try_get` 逐位与标量一致、越界报错；
`try_set` 后逐位一致且清空最后一个 `Unknown` 时 `unknowns()` 归一为 `None`。

## 5. 覆盖范围（已证 / 未证）

覆盖范围（当前 `scripts/verify.py --only verify` 报 182 verified）：

- K3：`!` 对合、`&`/`|` 交换与 De Morgan、`teq` 自反/互补、`rank` 单射、
  `table` 与 spec 一致；
- Ł3：格序 `∧/∨`、强加/强合交换、`U⊕U=T`、`U⊗U=F`、`U⇒U=T`、De Morgan、
  等价自反；
- B3：毒值吸收、`∧/∨` 交换、De Morgan、`U⇒U=U`；
- B4：双轨往返、否定对合、De Morgan、结合/分配/吸收、`xor=¬equiv`、
  consensus/gullibility 交换、conflation 对合与知识序 De Morgan；
- 位平面 `TriBit`：`not`/`and`/`or` 的 Verus 逐位等价（`bits/proofs.rs`）；
- 位切片向量 `TriVec`：`not`/`and`/`or`（K3·Ł3 格序与 B3 毒值）的整向量等价
  （`bulk/proofs.rs` + `point/proofs.rs`，累计 182 verified）与 `tests/bridge_bulk.rs`
  `tests/bridge_point.rs` 对拍；
- 位切片向量单点操作：`get`/`set`/`from_ternary` 的视图等价（`point.rs` 的
  `set_at_spec` / `prefix`），以及 `tests/bridge_point.rs` 中 get/set/from_ternary
  与主库的对拍；

未验证（明确缺口）：

- SIMD 路径与标量路径等价（见 §7 的分层口径）；
- `TriVec` 单点写的 `wf` 保持性：`set`/`get`/`from_ternary` 已证视图等价（见 §4.2），
  但「两平面互斥」的结构不变式未证；
- 并发、`unsafe`、内存模型；
- Verus 自身、z3 模型、Rust 编译器的可信基。

## 6. 路线图

1. **M1（本文件）**：K3 标量锚点 + bridge + CI。
2. **M2（落地）**：Ł3 / B3 / Belnap 四值的 spec、代数律与 bridge。
3. **M3（落地）**：`verus/src/k3_impl.rs` 复制主库 `value.rs` 的 K3 实现体，
   由 `scripts/check_k3_impl.py` 逐字比对，再用 Verus 的 `ensures` 验证该实现；
   `table ≡ spec` 的 bridge 降级为冗余检查。位平面/批量路径与单点操作
   已完成（182 verified，`python3 scripts/verify.py --only verify` 复算）；
   剩余：单点写的 `wf` 保持、padding 掩码、SIMD 口径（见 §7）。
4. **M4**：统一入口 `trit-verify`：Verus / Kani / Creusot / 属性测试共用一套
   报告与门禁。
5. **M5**（远期）：面向三值/位平面领域更强的验证器（当前不启动）。
