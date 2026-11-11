# trit 仓库规则（智能体必读）

## 死命令（2026-10-29 用户口述）

1. **这是用户自己的 Rust 三进制工程**：模型、训练、推理、工具一律在本仓用 Rust 三进制实现。
2. **缺实现就自己造**；不许拿 Python 训练、外部模型或二进制现成件顶替。Python 只作对齐基线，不交付。
3. **模型权重与训练产物属于本仓**：代码进 `crates/`，大件进 `D:\KF\trit-data\`，不许放在 `.yanagent/`。
4. **没做完必须当场说明缺口**，不得把"文档里写了规矩"当成实现。

## 工程口径

- 实验层 `publish = false`；单文件 ≤800 行（`python scripts/module_budget.py`，台账只减不增）。
- 收工一条命令：`cargo test --workspace` + `python scripts/module_budget.py`。
- 训练层在 `crates/trit-train`（STE + 位平面 ALU）；新模型优先复用同一套量化/STE 口径。
- 三进制结论按 `docs/TERNARY-BOUNDARIES.md` 归 A/B/C/D，且必须带二进制基线。

## 默认阅读（三进制 / 无限图 / 游戏环境）

涉及三进制智能体、无限图/无限世界、游戏环境扫描或故障注入时，先读：

- OpenAI, *The free uniform spanning forest is a factor of IID*, 2026-09-25：Abstract、Theorem 1.1、Section 1.3，以及明确说明 “without a finitary coding radius or a finite input alphabet” 的段落。
- `docs/GAME-SCAN.md`（本仓游戏环境扫描、故障注入与恢复设计）。

引用规则：只引用“观测/规则分离、无根/重标记等变、Borel 因子 ≠ finitary 局部算法”这三个设计原则；必须说明论文对象是 FUSF，不是本仓扫描器或生成树，不得写成“论文证明了本实现”。

## 多会话 / 多智能体规则（2026-11-12 事故后）

- **共享工作树只读**：`D:/KF/trit-wspace-wt` 是共享主工作树，**禁止在里面 `git switch`/提交**。
  每个任务用 `python scripts/gitflow.py worktree add <分支>` 新开工作树；实测多会话在里面反复切分支，
  会让另一个会话/服务一读就"失灵"。
- **main 必须且只能在一个工作树检出**；合并后立刻回收：`python scripts/worktree_guard.py --apply`
  （只回收「PR 已合 + 干净 + 本地 tip == PR head」的工作树与分支，脏的一律保留）。
- 开工先 `python scripts/agent_claims.py check`，再 `claim`；收工 `release`。
- `CHANGELOG.md` 走 `merge=union`：只在 `## [Unreleased]` 后追加条目，不要重排/改写相邻条目。
- 长 CI（mutation 可达 1 小时）不要连环 rebase：先让基础门绿；同步用 `git merge origin/main`，
  不要 force-push 别人的分支。
