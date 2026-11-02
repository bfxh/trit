## 这个 PR 做了什么

<!-- 一段话说明动机与改动面；列出新增/修改的公开 API 与 CLI 行为。 -->

## 判据（可复算）

<!-- 给出命令与读数；性能类改动必须有前后对照。 -->

## 门禁

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] MSRV 1.88 可构建（CI `MSRV (1.88)` job）
- [ ] 变异门：改动面有登记条目，`python scripts/mutation_check.py --pkg <改动面>` 全绿
- [ ] 门脚本自测：`snapshot` / `deps plan` / `vendor` / `mutation gate`
- [ ] `python scripts/secret_scan.py`（合成夹具用 `secret-scan: allow` 显式豁免）
- [ ] base 是 `main`；如果是叠栈 PR，用 `python scripts/gitflow.py stack` 确认链形态（按 base 过滤的 CI 会零 check）

## 风险与回滚

<!-- 默认行为是否改变；回滚方式（revert 本 PR / 具体提交）。 -->
