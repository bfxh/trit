# trit worktree 负责人表（2026-11-09）

数据源：`copies_inventory.py` 的 33 个 linked worktree；最后提交人来自 `git log -1`。
所有 33 个 worktree 的最后提交人都是 `bfxh`；负责人一列目前是**暂定**，需要本人确认。
脏工作区在确认前不要 rebase/清理；“已并入 main”按 `git merge-base --is-ancestor HEAD origin/main` 判定。

| 路径 | 分支 | 主题 | 负责人（待确认） | 脏 | 已并入 main | 最后提交 |
|---|---|---|---|---:|---|---|
| `D:/KF/trit` | `yan/texture-optimization` | 集成 / 纹理优化主 worktree | bfxh | 2 | no | `c2b2a75` Merge pull request #374 from bfxh/feat/trit-gpu-ttx2 |
| `D:/KF/trit-cap` | `feat/capacity-m52` | capacity M5.2 | bfxh | 0 | no | `618c942` exp(trit-3d): 真 ARAP（局部旋转 + 全局 CG）+ 折叠检查工具 |
| `D:/KF/trit-codec-pad` | `yan/codec-canonical-padding` | codec canonical padding | bfxh | 0 | no | `c470774` refactor(gpu): 抽 bench_variant 去重三处变体流程，matmul.rs 838→786 回到 |
| `D:/KF/trit-docs` | `docs/readme-positioning` | README 定位 | bfxh | 0 | no | `03b24b9` docs: README 重写（定位/可视化强弱对比/代价清单）+ 经验与负结果 |
| `D:/KF/trit-f16` | `fix/f16-shader-split` | f16 shader 拆分 | bfxh | 0 | no | `d376d4b` fix(trit-gpu): fp16 着色器独立成文件，缺 SHADER_F16 的 adapter 不再拖垮 f32 |
| `D:/KF/trit-fix-portable` | `ci/macos-and-gpu-gates` | macOS / GPU gates | bfxh | 0 | no | `923cc5e` ci: gpu 测试收窄到 1x1x1 冒烟（软件后端跑完整形状太慢） |
| `D:/KF/trit-fl` | `feat/float-m1` | float M1 | bfxh | 0 | no | `25248d7` fix(float): 修变异锚点并删掉不可达的平局分支 |
| `D:/KF/trit-graph` | `feat/graph-pool` | graph pool | bfxh | 2 | yes | `8cad77f` feat(graph): 端到端三态节点分类 M0（软训练 + 硬推理） (#165) |
| `D:/KF/trit-immune` | `feat/merge-auto-sync` | merge auto sync | bfxh | 0 | no | `975d030` Merge branch 'main' into feat/merge-auto-sync |
| `D:/KF/trit-lean-oai` | `feat/openai-math-next` | openai math | bfxh | 0 | no | `ce15bfc` Merge remote-tracking branch 'origin/main' into feat/openai- |
| `D:/KF/trit-lsp` | `feat/tri-lsp` | trit-lsp | bfxh | 0 | no | `387dcc4` feat(lsp): .tri 三进制语言服务器 M0 |
| `D:/KF/trit-mask` | `perf/mask-2row` | mask 2-row perf | bfxh | 0 | no | `d9181ef` perf(trit-ops): AUTO_LUT_ROWS 40960 -> 65536（掩码变快后 tryte 反超点 |
| `D:/KF/trit-netsec` | `feat/trit-net-drift` | net drift | bfxh | 0 | yes | `6c6b71b` SECURITY-NET：校准回写实测读数入册 ( #323 ) |
| `D:/KF/trit-neuro` | `yan/neuron-graded-loop` | neuron graded loop | bfxh | 0 | no | `898fb5a` docs(gpu): 张量核路径存在性核查——四条路径全是 SIMT/popc |
| `D:/KF/trit-nopython` | `feat/no-python-training` | no-python training | bfxh | 0 | no | `93cb47c` ci: 训练禁 Python 硬门（四类信号 + pre-commit） |
| `D:/KF/trit-ops` | `yan/trit-ops` | trit-ops | bfxh | 0 | no | `114ff37` fix(trit-ops): AUTO_LUT_ROWS 2048 -> 40960（原阈值让 2k-40k 行走更慢的 |
| `D:/KF/trit-ops2` | `perf/tryte-map` | tryte map | bfxh | 0 | no | `a45246f` docs(trit-ops): 修 rustdoc 私有项链接（-D warnings） |
| `D:/KF/trit-orch` | `feat/orchestration-core` | orchestration core | bfxh | 0 | no | `9b0b2bb` feat(trit-orch): 认领/租约 + 四态回执 + 交接门 |
| `D:/KF/trit-parts` | `feat/trit-parts` | trit-parts | bfxh | 1 | no | `98f5428` Merge remote-tracking branch 'origin/main' into feat/trit-pa |
| `D:/KF/trit-re` | `feat/ordered-cooccurrence` | ordered cooccurrence | bfxh | 1 | no | `0f3f89b` Merge remote-tracking branch 'origin/main' into feat/ordered |
| `D:/KF/trit-re-cfg` | `feat/trit-re-cfg` | re-cfg | bfxh | 0 | yes | `48e7ee4` 4 条崩溃红变异改指到有界递归测试上（守卫在产品代码里，不是 Rust 的问题） (#203) |
| `D:/KF/trit-split` | `refactor/split-matmul` | matmul 拆分 | bfxh | 0 | no | `2f0442d` refactor(trit-gpu): 拆 matmul 的 655 行驱动函数为 5 个模块 |
| `D:/KF/trit-train` | `feat/trit-train` | trit-train | bfxh | 0 | no | `3d2e4fb` fix(train): bin 改名 train-bench，避免与 trit-ops 的 bench 撞 rustdo |
| `D:/KF/trit-wspace-wt` | `main` | wspace（main） | bfxh | 12 | yes | `5058a09` feat(wspace): 按层 λ + 32×32 hidden128 对照 (#375) |
| `E:/KF/trit-claims` | `feat/vcs-status-cache` | vcs status cache | bfxh | 11 | yes | `7361e60` feat(vcs): trit-vcs M1 —— 内容寻址 + CDC 打包 + delta + 哈希链审计 + 认领 |
| `E:/KF/trit-count` | `perf/dpll-alloc-probe` | DPLL alloc probe | bfxh | 2 | no | `7776f51` test(gate): trit-count 两条变体重锚到扁平表中真实的代码（锚点唯一性当场断言） |
| `E:/KF/trit-dp4a` | `feat/gpu-dp4a` | GPU DP4A | bfxh | 0 | no | `e21a01c` perf(gpu): DP4A 加 tiling + 2×2 寄存器阻塞（32×32 tile / K 步 32 / 共 |
| `E:/KF/trit-gates` | `feat/default-gates` | default gates | bfxh | 0 | no | `125b91d` Merge remote-tracking branch 'origin/main' into feat/default |
| `E:/KF/trit-grew` | `feat/budget-grew-splits` | budget grew splits | bfxh | 0 | no | `b20f0b8` Merge remote-tracking branch 'origin/main' into feat/budget- |
| `E:/KF/trit-immune-fence` | `feat/lock-flag-error-named` | lock flag error | bfxh | 0 | no | `7c647fd` 把「未知旗标判红」的断言做成判决性的：错误要点名旗标并给新旧对应 |
| `E:/KF/trit-parity` | `chore/vcs-moved-out` | vcs moved out | bfxh | 0 | no | `76a11ab` fix(ci): 采集器别再喂「已经不存在的靶子」给变异门 —— 删除型 PR 会被 fail-closed 误判红 |
| `E:/KF/trit-sample` | `feat/ttx-sampler` | TTX sampler | bfxh | 1 | no | `1ae6ac0` feat(gpu): 光栅管线——顶点/片元/深度全在 GPU，片元自己从 dpdx 选 LOD |
| `E:/KF/trit-sweep` | `feat/registry-sweep` | registry sweep | bfxh | 21 | no | `bf1927c` Merge remote-tracking branch 'origin/main' into feat/registr |

## 相邻副本（2 个 submodule）

两份 `trit-tml` 的 `third_party/trit` 已统一到 `46e7eb2`（main HEAD），且本地 submodule origin 已指回 `.gitmodules` 的 GitHub URL：
- `C:/Users/lbx13/Projects/trit-tml`：本地提交 `06298af`，`cargo check` 通过；
- `D:/KF/.yanagent/repos/trit-tml`：本地提交 `6f050d7`，`cargo check` 通过。
两笔 trit-tml 提交尚未 push；如果 trit-tml 的 remote 需要同步，单独走 trit-tml 仓的 PR。

## 发布 preflight

```bash
python scripts/release_preflight.py --out target/release-manifest.json --artifact target/release/trit-build
```

拒绝条件：非 git 顶层、origin 不是 `bfxh/trit`、分支不是 `main`/`release/*`、工作区不 clean（含 untracked）、当前不是第一个 canonical worktree。
`--allow-noncanonical` 只给 CI/发布专用 worktree，使用时会把 `canonical_worktree=false` 写进清单。
自测：`python scripts/release_preflight.py --self-test`。

## 清理候选（clean + 已并入 main）

- `D:/KF/trit-netsec`
- `D:/KF/trit-re-cfg`

## 脏工作区（先由负责人处理）

- `D:/KF/trit`：2 个未提交条目
- `D:/KF/trit-graph`：2 个未提交条目
- `D:/KF/trit-parts`：1 个未提交条目
- `D:/KF/trit-re`：1 个未提交条目
- `D:/KF/trit-wspace-wt`：12 个未提交条目
- `E:/KF/trit-claims`：11 个未提交条目
- `E:/KF/trit-count`：2 个未提交条目
- `E:/KF/trit-sample`：1 个未提交条目
- `E:/KF/trit-sweep`：21 个未提交条目
