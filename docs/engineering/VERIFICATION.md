# 形式化验证：主库三值语义（Verus）

> 状态：**M1 锚点**。K3 标量算子已有机器检查的规格与证明；Ł3/B3/Belnap、
> 位平面/批量实现、并发与 `unsafe` 仍是缺口（见文末）。

## 1. 目标与边界

主库的三值语义是所有下游共享的稳定 API。测试只能抽样，Verus 用 SMT 对
**所有输入**检查规格。当前只覆盖 `trit-core::value::Ternary` 的 K3 标量算子，
不改变运行时依赖、不改变 `cargo test --workspace` 的口径。

验证锚点是 `verus/`（独立 workspace，`publish = false`）：

```text
verus/
├── Cargo.toml        trit-verified：vstd + trit-core；独立 workspace
├── README.md         范围、运行方式、缺口
├── src/table.rs      NOT/AND/OR/EQ/XOR 编码真值表（单一来源）
├── src/k3.rs         Verus spec + 代数律证明 + exec 提炼
├── src/lib.rs        模块出口
└── tests/bridge.rs   普通 Rust 穷举对拍 trit-core
```

## 2. 信任链

```text
trit-core::value::Ternary  ≡  table.rs   （tests/bridge.rs，3×3 完备）
table.rs                   ≡  spec       （src/k3.rs，Verus 证明）
⇒ trit-core 的 K3 算子满足 spec
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

- **代码**：下游依赖主库的 `trit-core`，拿到的是同一份已验证实现；
  验证锚点不进入运行时依赖。
- **复核**：下游在自己的 CI 里调用 `python /path/to/trit/scripts/verify.py`，
  或在 PR 模板中引用 `verus` workflow 的结论。
- **发布**：主库 tag 的 CI 必须包含 `verus` job；下游锁主库版本即锁验证结论。

“所有依赖主库的都依赖它”在这里的含义是：**验证只做一次，结论随主库版本传播**，
而不是每个下游各复制一份三值逻辑再各自证明。

## 5. 已验证 / 未验证

已验证（K3）：

- `!` 对合；
- `&`/`|` 交换律、De Morgan；
- `teq` 自反、`teq`/`tne` 互补；
- `rank`/`index` 单射；
- `table` 与 spec 逐项一致。

未验证（明确缺口）：

- Ł3 / B3 / Belnap 四值（`trit-belnap`）；
- `trit-core` 的 `bits` / `bulk` / SIMD 路径与标量路径等价；
- 并发、`unsafe`、内存模型；
- Verus 自身、z3 模型、Rust 编译器的可信基。

## 6. 路线图

1. **M1（本文件）**：K3 标量锚点 + bridge + CI。
2. **M2**：把 `trit-core::Ternary` 的标量实现直接纳入 `verus!` 验证
   （而不是“table ≡ spec + bridge”两段式），减少一层对拍假设。
3. **M3**：Ł3/B3/Belnap、位平面与批量路径的等价证明。
4. **M4**：统一入口 `trit-verify`：Verus / Kani / Creusot / 属性测试共用一套
   报告与门禁。
5. **M5**（远期）：面向三值/位平面领域更强的验证器（当前不启动）。
