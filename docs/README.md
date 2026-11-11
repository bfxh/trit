# 文档索引

仓库的文档按「定位 → 口径 → 结果 → 工程」四层组织。读之前先看两篇：

- **[DESIGN.md](DESIGN.md)**：主库存什么、三进制在这里指什么。
- **[STATUS.md](STATUS.md)**：状态词只有四个（已跑通 / 已实现未测 / 组件 / 已否证），"完成"必须有复算命令与读数。

## 定位与边界

| 文档 | 一句话 |
|---|---|
| [DESIGN.md](DESIGN.md) | 设计与定位：底座仓、生产链按 git pin 取用 |
| [SCOPE.md](SCOPE.md) | 仓库边界：收什么、不收什么（数据/二进制/跨仓分工） |
| [LAYERS.md](LAYERS.md) | 新代码归位判据（算子 / 算法 / 学习 / 生产链）+ `scripts/layers.py` |
| [TERNARY-BOUNDARIES.md](TERNARY-BOUNDARIES.md) | 三进制四类问题不能混谈（A/B/C/D 归类） |
| [STATUS.md](STATUS.md) | 每条能力的真实状态与证据口径 |

## 规划与路线

| 文档 | 一句话 |
|---|---|
| [ROADMAP.md](ROADMAP.md) | 阶段目标与当前进度 |
| [GENERAL-PLAN.md](GENERAL-PLAN.md) | 通用提取路线（全自建 Rust 三值） |
| [VISUAL-PLAN.md](VISUAL-PLAN.md) | 光影 / 风格 / 视频提取 / 画质 / UI 的扩展计划 |
| [PRIVACY-PLAN.md](PRIVACY-PLAN.md) | 隐私存储（后期）：威胁模型、容器与挂载器路线、硬边界 |
| [MULTI-CHIP.md](MULTI-CHIP.md) | 多芯片后端（x86 / ARM / 移动 SoC）设计 |

## 结果与读数（自动生成目录 + 手写结论）

| 文档 | 一句话 |
|---|---|
| [RESULTS.md](RESULTS.md) / [DEPTH-RESULTS.md](DEPTH-RESULTS.md) / [PARTS-RESULTS.md](PARTS-RESULTS.md) / [TEXTURE-RESULTS.md](TEXTURE-RESULTS.md) / [EXTRACT-RESULTS.md](EXTRACT-RESULTS.md) | 各条线的结果目录 |
| [LESSONS.md](LESSONS.md) | 经验与负结果（先读这篇再谈优化） |
| [CALIBRATION.md](CALIBRATION.md) / [N1-ARM64-CALIBRATION.md](N1-ARM64-CALIBRATION.md) | 真实样本校准工作手册与 ARM64 交叉引用 |
| [ATLAS.md](ATLAS.md) | 组件地图：前端 / 后端 / 语义层 / 横切工具 |
| [CAPACITY.md](CAPACITY.md) | RAM / 显存 / CPU / GPU 容量预测与防爆设计 |

## 模型与数据

| 文档 | 一句话 |
|---|---|
| [MODEL-CARD.md](MODEL-CARD.md) | 部件平面头的模型卡（口径、读数、边界） |
| [DATASETS.md](DATASETS.md) | 训练素材清单与可访问性实测 |

## 工具链

| 文档 | 一句话 |
|---|---|
| [TERNARY-TOOLCHAIN.md](TERNARY-TOOLCHAIN.md) | TTIR → TRB2 三进制构建工具链的定位、论文依据与边界 |
| [TTX.md](TTX.md) | TTX 资产容器的竞争力定位与规范 |

## 安全与并发

| 文档 | 一句话 |
|---|---|
| [SECURITY-NET.md](SECURITY-NET.md) / [SECURITY-RE.md](SECURITY-RE.md) | 网络与逆向面的威胁模型与边界 |
| [GUARD.md](GUARD.md) | 训练看门狗（parts-guard） |
| [HAZARD-POLICY.md](HAZARD-POLICY.md) | 并发/内存高危用法的放行判据（`scripts/hazard_gate.py` 依据） |

## 工程与治理

| 文档 | 一句话 |
|---|---|
| [WORKTREES.md](WORKTREES.md) | worktree 负责人表（由 `git worktree list` 实时生成） |
| [BUILD-PERF.md](BUILD-PERF.md) | 构建性能：本机 / CI 实测、sccache 边界、已落地与未采纳项 |

## 其他

- `assets/`：README 图表（SVG，由 `scripts/gen_docs_charts.py` 生成）。
- `review/`：评审记录。
