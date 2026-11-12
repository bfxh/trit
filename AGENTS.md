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
- **闸门装在本机（不是只写在文档里）**：每台机器/每个 clone 先跑一次
  `python scripts/hooks.py install`，装三道钩子：
  - `pre-commit`：共享工作树停在非 main（含 detached）时拒绝提交；main 上的提交一律拒绝；
  - `post-checkout`：共享工作树被切到非 main 时自动切回 main，切不回大声报；
  - `pre-push`：拒绝直推 main + 高置信度密钥扫描。
  点对点放行用 `TRIT_ALLOW_SHARED_COMMIT=1` / `TRIT_ALLOW_MAIN_COMMIT=1` /
  `TRIT_ALLOW_SHARED_SWITCH=1`；共享树清单可用 `TRIT_SHARED_WORKTREES` 覆盖（`os.pathsep` 分隔）。
  `install` 会把 `hooks.py` 与合并驱动复制成 hooks 目录里的**稳定副本**（`trit-hooks.py` /
  `trit-json-union.py`），钩子不依赖当前分支的脚本版本（老分支缺新子命令时不会 rc=2 误伤）；
  升级 `hooks.py` 后重跑一次 `install` 刷新副本。
- **棘轮台账并发改不冲突**：`CHANGELOG.md` 走 `merge=union`；三本基线
  （`scripts/test_surface_baseline.json`、`scripts/module_budget_baseline.json`、
  `scripts/docs_hygiene_baseline.json`）走 `merge=json-union` 驱动
  （`scripts/merge_json_union.py`，由 `hooks.py install` 注册）：
  测试面取 max、模块预算取 min、文档卫生取 min，不认识形状一律留给 git 报冲突。
- **负更新默认拦**：`pre-push` 会跑 `scripts/branch_update.py`：非快进且 old 上有提交在新历史里
  没有等价补丁（`git cherry` 的 `+`）⇒ 拦；三本基线回退（测试变少 / 上限放松 / CHANGELOG 变小）
  也拦。重写自己的未合并分支用 `ALLOW_NON_FF_PUSH=1`，确属收紧用 `ALLOW_RATCHET_PUSH=1`；
  两个开关分开，别当万能钥匙。
- **认领不是先到先得**：`claim --plan`（或 `--plan-file`）写方案；重叠时
  `compare --paths ... [--repo]` 并排比较「问题解释 / 兼容 / 牵连 / 回滚 / 验证」，
  **没有方案 = 没有资格参与比较**；败方 `release` 或收窄路径。并发 claim/release 有文件锁；
  历史重复 id 用 `repair --apply` 修复。
- **先量牵连再动手**：`python scripts/impact.py --base origin/main --claims <CLAIMS.md>
  --repo bfxh/trit` 给出反向依赖闭包与「会被牵连但没人认领」的 crate；实测改 `trit-core`
  一个文件牵连 37/44 个 crate。认领面要覆盖到下游，否则下游会话不会知道自己在被改。
- **main 必须且只能在一个工作树检出**；合并后立刻回收：`python scripts/worktree_guard.py --apply`
  （只回收「PR 已合 + 干净 + 本地 tip == PR head」的工作树与分支，脏的一律保留）。
- 开工先 `python scripts/agent_claims.py check`，再 `claim`；收工 `release`。
- `CHANGELOG.md` 只在 `## [Unreleased]` 后追加条目，不要重排/改写相邻条目。
- 长 CI（mutation 可达 1 小时）不要连环 rebase：先让基础门绿；同步用 `git merge origin/main`，
  不要 force-push 别人的分支。
