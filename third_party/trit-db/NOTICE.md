# NOTICE

当前骨架不引入任何外部 crate；`trit-db` 之后若引入主库依赖，按以下规则记录：

- `trit-core`、`trit-count`、`trit-ir`：可发布；按 tag/rev pin。
- `trit-codec`、`trit-texture`、`trit-ops`：`publish = false`；只通过 `git = "https://github.com/bfxh/trit", rev = "<sha>"` 依赖，不复制源码。
- 本仓 `publish = false` 到 1.0；1.0 前不谈 semver。

上游仓库：<https://github.com/bfxh/trit>
