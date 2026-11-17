# Fork-and-Flush：给多智能体任务加「分叉 + 清空」

> 论文：[Fork-and-Flush: Escaping Idea Basins in Autoresearch Agents](https://arxiv.org/abs/2610.07447)
> （Cai 等，Microsoft Research，2026-10-05）。13 个长时程任务上比单次运行 / Best-of-N
> 分别高 **66.0% / 44.4%**；探索一个新想法的收益约为细化现有想法的 **9×**；
> 58% 的运行从未到达包含最优解的「想法盆地」。

本仓已有的多智能体规则（每个任务开 worktree、共享树只读、claims 防撞车）其实已经做了
**Fork** 的工程版；缺的是 **Flush**（清空对话历史）和**按验收分数选最优轨迹**。
`scripts/fork_flush.py` 补这两步。

## 规则

1. **S 级任务默认至少分叉 2 条轨迹**。每条轨迹：
   - 独立 worktree（`git worktree add`，分支 `fork/<task>-i`）；
   - 从干净上下文开始（Flush）：只带任务包与工作区，不带之前的探索历史；
   - 固定 horizon（默认 8 次尝试），到点就停，不无限细化。
2. **收分**：每条轨迹收工时运行验收命令，stdout 最后一行输出 `SCORE=<float>`（越大越好）。
3. **选优**：`pick` 比较分数，最高分轨迹继续合并；其余保留或回收。
4. **记录探索比例**：一条轨迹里「新想法」与「细化」的尝试次数应可区分——论文的 9×
   收益意味着流程应偏向探索，而不是无限磨同一个方案。

## 用法

```bash
# 1. 分叉：为同一任务开 3 条轨迹（各自 worktree + FORK_TASK.md）
python scripts/fork_flush.py plan --task docs/AGENT-TASKS/S-YARA-FULL.md --n 3

# 2. 每条轨迹独立执行（不同会话 / 不同智能体，互不共享对话）
#    ...按 FORK_TASK.md 做完，输出 SCORE=...

# 3. 收分：在每个 worktree 里跑验收命令
python scripts/fork_flush.py score \
  --runs ../trit-fork-1 ../trit-fork-2 ../trit-fork-3 \
  --cmd "python scripts/module_budget.py && python -m pytest -q tests/ | tail -1"

# 4. 选最优（--merge 会把它合入当前分支）
python scripts/fork_flush.py pick \
  --runs ../trit-fork-1 ../trit-fork-2 ../trit-fork-3 --merge
```

验收命令必须自己把结果压成最后一行 `SCORE=<float>`，例如：

```bash
python -m pytest -q 2>&1 | tail -1        # 不能直接当分数
# 包一层：
python - <<'PY'
import subprocess, re
out = subprocess.run(["python","-m","pytest","-q"], capture_output=True, text=True).stdout
m = re.search(r"(\d+) passed", out)
print(f"SCORE={int(m.group(1)) if m else 0}")
PY
```

## 与现有规则的关系

| 现有规则 | Fork-and-Flush 补什么 |
|---|---|
| 每任务一个 worktree | 同一任务 **N 个** worktree，互不共享对话 |
| 共享树只读 / claims 防撞车 | 轨迹之间不读彼此结果，避免互相污染 |
| AGENT-TASK-STANDARD 的验收命令 | 统一成 `SCORE=<float>`，供自动选优 |
| 收工门（test/module_budget 等） | 作为分数来源之一，不替代 |

## 未证栏

- 脚本**不调用任何模型**：只负责开 worktree、记分、选优；「清空上下文」靠会话隔离实现，
  脚本无法强制。
- `SCORE` 的判别力取决于验收命令；单一指标可能奖励过拟合的轨迹（与论文的
  min-max 归一化不同）。
- horizon 默认 8 次没有调参依据；论文的固定 horizon 是按时长/预算，不是次数。
- 只在单机、Windows 上做过 `score/pick/self-test` 的端到端测试；`plan` 依赖
  `git worktree` 的权限与路径约定。
