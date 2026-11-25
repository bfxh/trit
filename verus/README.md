# trit-verified：主库三值语义的验证锚点

`trit-verified` 用 [Verus](https://github.com/verus-lang/verus) 为 `trit-core` 的
K3（强 Kleene）三值语义建立机器检查的规格与证明。它是**验证锚点**，不是运行时依赖：
主库的 `cargo build/test` 不构建它，验证由 `scripts/verify.py` 单独驱动，CI 走
`.github/workflows/verus.yml`。

## 验证链

```text
trit-core::value::Ternary  ≡  table.rs   （tests/bridge.rs：3×3 完备穷举）
table.rs                   ≡  spec       （src/k3.rs：Verus 证明）
⇒ trit-core 的 K3 算子满足 spec
```

- `src/table.rs`：`NOT` / `AND` / `OR` / `EQ` / `XOR` 的编码真值表（0=F、1=U、2=T），
  是普通测试与 Verus 规格共用的单一来源。
- `src/k3.rs`：`spec fn knot/tand/tor/teq/tne/rank`、代数律证明（双重否定、
  交换律、De Morgan、`teq` 自反、`rank` 单射）、`table_*_matches_spec`、
  以及 exec 提炼（`knot_exec` 等带 `ensures`）。
- `tests/bridge.rs`：普通 Rust 穷举对拍 `trit-core` 的 `!`/`&`/`|`/`^` 与
  `index()`/`from_index()`。

## 本地运行

```bash
# 1) 安装 Verus（固定版本 + 官方 SHA-256 校验）
python scripts/verify.py --install          # 下载 Linux/Windows release 到 ~/tools/verus

# 2) 验证
python scripts/verify.py                    # 检查工具链 + cargo-verus verify + cargo test
python scripts/verify.py --json             # 机器可读报告
```

`scripts/verify.py` 会读取：

- `TRIT_VERUS_DIR`：Verus 解压目录（默认 `~/tools/verus-x86-linux`）；
- `TRIT_Z3_DIR`：Z3 4.16.0 解压目录（默认 `~/tools/z3-4.16.0-x64-glibc-2.39`）；
- `TRIT_VERUS_TOOLCHAIN`：rustup toolchain（默认 `1.100.0-x86_64-unknown-linux-gnu`）。

## 范围与限制

已验证（K3，有限域穷举 + Verus）：

- `!`（否定）的对合；
- `&`/`|` 的交换律与 De Morgan；
- `teq` 自反、`teq`/`tne` 互补；
- `rank`/`index` 单射；
- `table` 与 spec 逐项一致。

**未验证**（明确缺口，不当作已完成）：

- Ł3 / B3 / Belnap 四值（`trit-belnap`）尚未写 spec；
- `trit-core` 的位平面/批量实现（`bits`、`bulk`）与标量实现的等价；
- 任何并发或 `unsafe` 代码；
- Verus 本身的可靠性、z3 模型与 Rust 编译器的可信基。

## 为什么独立 workspace

`verus/` 是独立 workspace（`publish = false`），不并入主库 `Cargo.toml`：

- 主库的 stable/MSRV 构建不引入 `vstd`，不改变主库 `Cargo.lock`；
- 验证由 `scripts/verify.py` 与单独的 CI job 驱动，不会拖慢 `cargo test --workspace`；
- 下游项目要获得同一保证，依赖主库的 `trit-core`（同一份代码）即可；
  需要本地复核时复用 `scripts/verify.py`。
