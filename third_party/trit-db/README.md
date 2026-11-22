# third_party/trit-db

`trit-orch` 的 `tritdb-*` 依赖的**源码副本**（vendored），用于离线构建：不再从
GitHub 拉取。

放在 `third_party/` 而不是 `vendor/`，是因为本仓 `.gitignore` 忽略了 `/vendor/`
（`scripts/vendor.py` 的 crates.io 离线自足目录，`restore` 时会整目录删除）。
把上游源码放在那里会被 `vendor.py restore` 一并删掉，path 依赖随即断链。

- 上游：<https://github.com/bfxh/trit-db>
- 固定版本：`aca718880e0d918a064c878b39564225e46db80a`
- 保留范围：`tritdb-txn` / `tritdb-sql` / `tritdb-cas` / `tritdb-pgwire` 及其内部
  依赖闭包（`tritdb-radix`、`tritdb-page`、`tritdb-wal`、`tritdb-exec`、
  `tritdb-tritvec`，外加测试用的 `tritdb-testkit`）。
- 许可证：上游 `Cargo.toml` 声明 `MIT OR Apache-2.0`；本目录不新增许可证文件，
  只保留上游 `NOTICE.md`。

`crates/trit-orch/Cargo.toml` 用 `path` 指向这里的 crate，所以 `cargo build
--offline` 在没有网络、也没有 git 凭据的机器上也能完成。

## 边界

- 本目录已在 `scripts/module_budget.py` 的 `EXCLUDE_DIRS` 里：上游源码
  （例如 `tritdb-pgwire/src/lib.rs` 有 4000+ 行）不按本仓预算拆分。
- 上游代码不跑本仓的 `cargo fmt`：`cargo fmt --all` 只看本 workspace 成员。

## 更新

```bash
scripts/vendor-tritdb.sh <上游 git rev>
```

脚本从 `TRIT_DB_SRC`（必须显式设置，无默认值）按给定 rev 重新导出上面这些 crate，
并刷新 `NOTICE.md` 与 `rustfmt.toml`。更新后跑：

```bash
cargo test --offline -p trit-orch --features tritdb-pgwire
```
