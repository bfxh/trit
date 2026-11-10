# trit worktree 负责人表（2026-11-10 治理更新）

数据源：`git worktree list` 实时生成；负责人经用户确认为 bfxh（本人工程，非智能体所有）。
脏工作区在确认前不要 rebase/清理；“已并入 main”按 `git merge-base --is-ancestor HEAD main` 判定（squash 合入的分支此法判 no，需 PR 证据）。

| 路径 | 分支 | 负责人 | 脏 | 已并入 main | 最后提交 |
|---|---|---|---:|---|---|
| `trit` | `chore/texture-gpu-integration` | bfxh | 1 | no | `677a047 chore: YAN 智能体清理——活动路径迁出 .yanagent` |
| `trit-cap` | `feat/drift-next` | bfxh | 0 | no | `f25f3a2 feat(trit-net): 漂移门逐特征归因——从「哪个 band 变坏」到「哪个特征漂了」` |
| `trit-docs` | `docs/readme-positioning` | bfxh | 0 | no | `03b24b9 docs: README 重写（定位/可视化强弱对比/代价清单）+ 经验与负结果` |
| `trit-f16` | `fix/f16-shader-split` | bfxh | 0 | no | `d376d4b fix(trit-gpu): fp16 着色器独立成文件，缺 SHADER_F16 的 adapter 不再拖垮 f32 基` |
| `trit-fix-portable` | `ci/macos-and-gpu-gates` | bfxh | 0 | no | `923cc5e ci: gpu 测试收窄到 1x1x1 冒烟（软件后端跑完整形状太慢）` |
| `trit-fl` | `feat/float-m1` | bfxh | 0 | no | `25248d7 fix(float): 修变异锚点并删掉不可达的平局分支` |
| `trit-graph` | `feat/graph-pool` | bfxh | 2 | yes | `8cad77f feat(graph): 端到端三态节点分类 M0（软训练 + 硬推理） (#165)` |
| `trit-immune` | `feat/merge-auto-sync` | bfxh | 0 | no | `975d030 Merge branch 'main' into feat/merge-auto-sync` |
| `trit-lean-oai` | `feat/openai-math-next` | bfxh | 0 | no | `ce15bfc Merge remote-tracking branch 'origin/main' into feat/openai-ma` |
| `trit-lsp` | `feat/tri-lsp` | bfxh | 0 | no | `387dcc4 feat(lsp): .tri 三进制语言服务器 M0` |
| `trit-mask` | `perf/mask-2row` | bfxh | 0 | no | `d9181ef perf(trit-ops): AUTO_LUT_ROWS 40960 -> 65536（掩码变快后 tryte 反超点后移` |
| `trit-nopython` | `feat/no-python-training` | bfxh | 0 | no | `93cb47c ci: 训练禁 Python 硬门（四类信号 + pre-commit）` |
| `trit-ops2` | `perf/tryte-map` | bfxh | 0 | no | `a45246f docs(trit-ops): 修 rustdoc 私有项链接（-D warnings）` |
| `trit-orch` | `feat/orchestration-core` | bfxh | 0 | no | `9b0b2bb feat(trit-orch): 认领/租约 + 四态回执 + 交接门` |
| `trit-parts` | `feat/trit-parts` | bfxh | 1 | no | `98f5428 Merge remote-tracking branch 'origin/main' into feat/trit-part` |
| `trit-re` | `feat/ordered-cooccurrence` | bfxh | 1 | no | `0f3f89b Merge remote-tracking branch 'origin/main' into feat/ordered-c` |
| `trit-split` | `refactor/split-matmul` | bfxh | 0 | no | `2f0442d refactor(trit-gpu): 拆 matmul 的 655 行驱动函数为 5 个模块` |
| `trit-train` | `feat/trit-train` | bfxh | 0 | no | `3d2e4fb fix(train): bin 改名 train-bench，避免与 trit-ops 的 bench 撞 rustdoc ` |
| `trit-wspace-wt` | `main` | bfxh | 0 | yes | `a809875 feat(trit-train): Adam 优化器 + trit_train_tasks 大 hidden 读数 (#38` |
| `trit-claims` | `feat/vcs-status-cache` | bfxh | 11 | yes | `7361e60 feat(vcs): trit-vcs M1 —— 内容寻址 + CDC 打包 + delta + 哈希链审计 + 认领门 ` |
| `trit-count` | `perf/dpll-alloc-probe` | bfxh | 2 | no | `7776f51 test(gate): trit-count 两条变体重锚到扁平表中真实的代码（锚点唯一性当场断言）` |
| `trit-dp4a` | `feat/gpu-dp4a` | bfxh | 0 | no | `e21a01c perf(gpu): DP4A 加 tiling + 2×2 寄存器阻塞（32×32 tile / K 步 32 / 共享 ` |
| `trit-gates` | `feat/default-gates` | bfxh | 0 | no | `125b91d Merge remote-tracking branch 'origin/main' into feat/default-g` |
| `trit-grew` | `feat/budget-grew-splits` | bfxh | 0 | no | `b20f0b8 Merge remote-tracking branch 'origin/main' into feat/budget-gr` |
| `trit-parity` | `chore/vcs-moved-out` | bfxh | 0 | no | `76a11ab fix(ci): 采集器别再喂「已经不存在的靶子」给变异门 —— 删除型 PR 会被 fail-closed 误判红` |
| `trit-sample` | `feat/ttx-sampler` | bfxh | 1 | no | `1ae6ac0 feat(gpu): 光栅管线——顶点/片元/深度全在 GPU，片元自己从 dpdx 选 LOD` |
| `trit-sweep` | `feat/registry-sweep` | bfxh | 21 | no | `bf1927c Merge remote-tracking branch 'origin/main' into feat/registry-` |

共 27 个 worktree。2026-11-10 治理：移除已合并且干净的 trit-codec-pad / trit-neuro / trit-ops / trit-netsec / trit-re-cfg 及 E:/tmp 两个 detached 副本；yan/* 分支全部改名或删除（PR 证据见各合并 PR）。

## 相邻副本（2 个 submodule）

两份 `trit-tml` 的 `third_party/trit` 已统一到 `46e7eb2`（main HEAD），且本地 submodule origin 已指回 `.gitmodules` 的 GitHub URL：
- `Projects/trit-tml`：本地提交 `06298af`，`cargo check` 通过；
- `trit-tml`（原 `.yanagent/repos/trit-tml`，YAN 清理时迁出至 `D:/KF/trit-tml`）：本地提交 `6f050d7`，`cargo check` 通过。
两笔 trit-tml 提交尚未 push；如果 trit-tml 的 remote 需要同步，单独走 trit-tml 仓的 PR。

## 发布 preflight

```bash
python scripts/release_preflight.py --out target/release-manifest.json --artifact target/release/trit-build
```

拒绝条件：非 git 顶层、origin 不是 `bfxh/trit`、分支不是 `main`/`release/*`、工作区不 clean（含 untracked）、当前不是第一个 canonical worktree。
`--allow-noncanonical` 只给 CI/发布专用 worktree，使用时会把 `canonical_worktree=false` 写进清单。
自测：`python scripts/release_preflight.py --self-test`。

## 清理候选（clean + 已并入 main）

（2026-11-10 已清：trit-netsec / trit-re-cfg / trit-codec-pad / trit-neuro / trit-ops；当前无 clean 已合候选。）

## 脏工作区（先由负责人处理）

- `trit`：CLAUDE.md 未跟踪（本地智能体配置，与 AGENTS.md 同源）
- `trit-graph`：graph_classifier.rs 新文件 + lib.rs 改动（WIP）
- `trit-parts` / `trit-re`：test_surface_baseline.json 各 1 处
- `trit-claims`：trit-vcs 11 处（分支已 squash 入 main，WIP 未提交）
- `trit-count`：hazard_gate.py + HAZARD-POLICY.md
- `trit-sample`：Cargo.toml
- `trit-sweep`：trit-re 21 处
