# 分支规范（BRANCHING）

> 现状（2026-11-21 实测）：**166 个本地分支，只有 5 个合并进 main**；86 个未合并且超过 3 天没动，
> 最老 28 天；命名里有 `pr130` / `pr126` / `pr128` / `pr-116` / `pr392` / `tmp-457` / `cursor` 这类
> 事后无法辨认的分支。多智能体各开各的分支、合并后不删、失败后不归档，才会积累成这个局面。
>
> 本文档是**默认规则**；`scripts/branch-hygiene.py` 是它的自动检查，CI 与本地都能跑。

## 1. 命名（强制）

| 允许前缀 | 用途 | 例 |
|---|---|---|
| `feat/` | 新功能 | `feat/wspace-m2` |
| `fix/` | 修 bug | `fix/tconv-conv-cap` |
| `perf/` | 性能 | `perf/gpu-dp4a` |
| `docs/` | 文档 | `docs/eval-baselines` |
| `test/` | 只加测试 | `test/pipe-cross-conn` |
| `security/` | 安全修复 | `security/res-square-widening` |
| `ci/` | CI/构建 | `ci/changed-crate-scope` |
| `chore/` | 杂务 | `chore/agent-checks` |
| `refactor/` | 不改行为 | `refactor/setup-split` |
| `release/` | 发布 | `release/0.13.0` |
| `parts/`（历史遗留，逐步迁移） | trit-parts 子模块 | `parts/quilt-size-cap` |

**禁止**：`pr<数字>` / `pr-<数字>`、`tmp-*`、`cursor`、`main-tracking`、无前缀的裸名（如 `vcs-split`）。
理由：半年后没人知道它对应什么；已有分支按 §5 迁移。

## 2. 生命周期（强制）

```text
创建 ──(绑定 worktree)──▶ 开发 ──(阶段完成即 push)──▶ PR ──(合并后 7 天内删)──▶ 删除
                                                     └─(未合并 >30 天)──▶ 归档或删
```

1. **一个分支一个 worktree**：`git worktree add ../trit-wt-<名> -b feat/<名>`。删 worktree 时同步删分支。
2. **阶段完成就 push**（见 `AGENTS.md` 第六节）：`git push -u origin HEAD`。
3. **合并后 7 天内删除**：本地 `git branch -d`，远端 `git push origin --delete <branch>`。
4. **未合并且 30 天未动**：必须先 `bash scripts/archive-worktrees.sh`，再决定「继续 / 删」。
5. **禁止长期保留 stash**：`stash pop` 或立刻导出 patch。

## 3. 每周检查（自动）

```bash
python scripts/branch-hygiene.py            # 人读报告
python scripts/branch-hygiene.py --strict   # 有违规就 rc=1（CI 用）
```

报告五类：

| 类 | 判定 | 建议 |
|---|---|---|
| `merged` | 已合并进 `main` | 删（本地 + 远端） |
| `empty` | `main..branch` 无提交（内容已在 main） | 删 |
| `stale` | 未合并且最后提交 > 30 天 | 归档后删 |
| `badname` | 命名不在 §1 白名单 | 改名或删 |
| `worktree` | 分支被某个 worktree 占用 | 先 `worktree remove` |

## 4. 合并前的最低要求

- `cargo test --workspace`（或至少受影响 crate）
- `cargo fmt --all -- --check`、`python scripts/module_budget.py`
- 提交信息带台账编号（`L<编号>`），改动行为须同步 `docs/ERRORS.md`

## 5. 已有分支的迁移（一次性）

1. `python scripts/branch-hygiene.py` 出清单
2. `merged` / `empty` → 直接删（本地 + 远端）
3. `badname` 且有未合并工作 → `git branch -m <新名>`（改名不改内容）
4. `stale` → 先 `archive-worktrees.sh` 归档，再删
5. 迁移完成后在 `docs/STATUS.md` 记一行日期与剩余分支数

## 6. 为什么要有这份文档

- 2026-11-21 的教训：**没有规范时，智能体只能各自发明**，于是出现 166 个分支、5 个合并、86 个陈旧；
  而一次"防丢失全量推送"把它们全固化到远端，制造了新的混乱。
- 规范 + 自动检查的意义：让"该删的分支自己暴露出来"，而不是靠某个人记得清理。
