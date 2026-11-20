# 并行开发与多智能体协作（COLLAB）

> 目标：**同时开发所有模块**时，人和智能体不会互相踩、不会制造垃圾、不会"本地绿 CI 红"。
> 前提：仓库已有三层机制 —— 认领（`scripts/agent_claims.py`）、约束（`.agent/constraints.json`）、
> 检查（`.agent/checks/registry.json`）。本文档把它们串成一套**默认开着**的流程。

## 1. 工作树模型：一个主工作树 + 按任务开 worktree

```bash
git clone git@github.com:bfxh/trit.git ~/wt/trit          # 只做一次
cd ~/wt/trit
git worktree add ../trit-wt-<任务名> -b agent/<名字>/<主题>   # 每个并行任务一个
git worktree remove ../trit-wt-<任务名>                      # 收工就删
```

**默认禁止**：为同一个仓库重复 `git clone`。历史事故：7 份 clone、各自 `target`、外加 31 GB 死缓存。
`git worktree` 共享 `.git`，不重复下载，也不会各长一份 target。

## 2. 认领：动手前先说你要动哪里

```bash
python scripts/agent_claims.py claim --who <宿主:会话> --repo bfxh/trit \
    --paths crates/trit-parts/src crates/trit-parts/tests --note "修 U2" --ttl 90
python scripts/agent_claims.py check      # 有重叠就红，并把双方都点名
python scripts/agent_claims.py release --who <宿主:会话>
```

- 声明 = 「我在动这些路径」+ TTL；**收工必须 release**，长任务用 `renew` 续期。
- 两个在效期内的声明路径相交 ⇒ 判红。不要靠"我们沟通一下"解决，靠这个命令。
- 交接账本只做事后记账，**挡不住同时动手**——认领才是动手前的门。

## 3. 分支与提交

| 约定 | 说明 |
|---|---|
| 分支名 | `agent/<智能体或人名>/<主题>`，例如 `agent/codex/fix-u2`；不要用 `pr-373-check` 这种事后无法辨认的名字 |
| 一个提交一件事 | 修 bug / 加文档 / 改基础设施分开；cherry-pick 与 revert 才可操作 |
| 提交前 | 跑 `cargo fmt --all -- --check`、`python scripts/module_budget.py`、相关 crate 的 `cargo test` |
| 提交信息 | 写清「改了什么 / 为什么 / 怎么验证」，引用台账编号（L/C） |
| 身份 | 只用**你本人的** git 身份；禁止任何 AI 厂商账号或 `Co-authored-by: <AI>` trailer（见 `~/AGENTS.md`） |

`scripts/install-hooks.sh` 装一次 pre-commit（fmt + module_budget + registry 结构），worktree 共用，装一次全生效。

## 4. 默认禁止清单（防呆，不需要每次讨论）

1. **不要重复 clone 同一仓库** —— 用 `git worktree`（§1）
2. **不要抬高 `module_budget` 台账上限** —— 超限就拆文件；`--update` 只用于登记新文件与收紧
3. **不要把 `target/`、`*.bundle`、归档目录入库** —— `.gitignore` 已覆盖
4. **不要跳过 hooks**（`--no-verify`）除非明确知道后果；pre-push 有审计
5. **不要删除/跳过测试**（`constraints.json` 的 `no-test-deletion` 会拦）
6. **新增 `unsafe` 必须同时写 `// SAFETY:`**（`constraints.json` 的 `unsafe-needs-safety-note` 会拦）
7. **不要直接改主工作树给别人准备的分支** —— 先 `agent_claims.py check`
8. **不要把 `.cargo/config.toml` 提交** —— 那是本机落盘策略（`.gitignore` 已覆盖）
9. **不要把"不知道是什么"的东西放进仓库** —— 提交前必须能回答"这是什么、凭什么入库"；
   编译产物/权重/归档/日志/密钥是硬拦，二进制与大文件必须写 `入库理由:`（`docs/ADMISSION.md`）

## 5. 兼容性：一次说清，避免"能跑就行"

| 维度 | 约定 | 怎么保证 |
|---|---|---|
| Rust 版本 | 日常工作用 `rust-toolchain.toml` 钉的 stable | `rustup` 自动解析，本地与 CI 同一套 |
| MSRV | **1.88**（`Cargo.toml` 的 `rust-version`） | CI 的 `msrv` job 单独跑 1.88 |
| Edition | 2024 | `Cargo.toml` 的 `[workspace.package]` |
| 平台 | Linux / Windows / macOS | `ci.yml` 三 OS 矩阵（PR 与 main 同矩阵） |
| 依赖版本 | `Cargo.lock` 入库并锁定 | 改依赖必须同时 `cargo update -p <crate>` 并说明原因 |
| 安全审计 | `cargo audit` 每周 + 手动 | `.github/workflows/quality.yml` |
| 依赖许可 | 见 `deny.toml`（若启用） | `cargo deny check` |
| 跨 crate 接口 | 改公开 API 需要显式放行 | `constraints.json` 的 `public-api-change` |

## 6. 本地门 ↔ CI 门对照

| 本地（pre-commit / 手动） | CI |
|---|---|
| `python scripts/repo_admission.py`（入库准入，第 0 步） | `secret_scan.py` / `ai_vendor_scan.py` / registry 的 `C094` |
| `cargo fmt --all -- --check` | `ci.yml` 的 fmt |
| `python scripts/module_budget.py` | `ci.yml` 的 budget 步 |
| 相关 crate `cargo test` | `ci.yml` 三 OS 的全量测试 |
| `python scripts/mutation_check.py` | `mutation.yml` |
| `.agent/checks/registry.json` 的 cmd 检查 | 各 workflow 按 id 复跑 |

**原则**：本地能挡的，不要让 CI 挡（CI 排队 10–30 分钟，智能体等不起）。

## 7. 撞车了怎么办

1. `python scripts/agent_claims.py check` → 看双方会话名与路径
2. 用 `compare --paths <a> <b>` 并排比方案，不是比谁先提交
3. 一方 `release` 或换路径；不要"我先 push 了算我的"
4. 已经分叉的，用 `git rebase` 到最新 `main` 后重跑本地门，再推

## 8. 一次性的"新镜像"检查

任何新机器/新镜像克隆后，按下面顺序过一遍（全部通过才算准备好）：

```bash
rustup show                                  # 1. rust-toolchain.toml 生效
bash scripts/install-hooks.sh                # 2. 装 pre-commit
cargo build                                  # 3. 能构建
cargo test --workspace                       # 4. 测试
python scripts/module_budget.py              # 5. 台账
python scripts/agent_claims.py list          # 6. 认领表（空 = 没人占）
```

## 9. 本机真实布局与 worktree 生命周期（2026-11-21 实测）

`git worktree list` 的真相：**24 个 worktree，跨三个盘**。

- 主仓库：`<主仓库根>`（detached HEAD）
- 当前工作树：`wt/tconv-cap`（`.git` 是文件，gitdir 指向 `<主仓库根>/.git/worktrees/tconv-cap`）
- <主仓库盘> 下 13 个、<编译盘> 下 4 个、<用户目录> 下 6 个（已删目录的 5 个 + 当前）
- 清理失效条目后：`git worktree prune` 删掉 5 条「目录已删、记录还在」的，剩 19 条

worktree 是并行开发的**推荐做法**，但必须管生命周期：

```bash
git worktree list                 # 巡检：有哪些、在哪个分支
git worktree remove <path>        # 任务完成就删（不删就会累积到 20+）
git worktree prune                # 目录已删但记录残留时清掉
```

**每个 worktree 会在 `CARGO_TARGET_DIR` 下按名字各占一份 `target`** —— 本机的 `<编译盘>/cargo-target`、`<编译盘 B>/cargo-target` 就是按 `trit-*` 名字分的。想让 18 个 worktree 只占一份 target，就把它们的 `CARGO_TARGET_DIR` 指向同一个目录（代价：切 worktree 会重编）。**默认策略：按 worktree 分开，任务结束就 `worktree remove` + 清它那份 target**。

巡检一眼看占用：

```bash
git worktree list && du -sh <target 根>/*/ | sort -rh
```


## 10. 防丢失：多智能体会话会因额度中断（2026-11-21 取证）

实测 19 个工作树：

| 风险项 | 数量 | 说明 |
|---|---|---|
| 未合并进 `main` 的提交 | **1008 个** | `trit-parts` 361、`wt-dim-sweep` 139、`wt-dim-sweep-ci` 109、当前 `tconv-cap` 118… |
| `stash` | **38 个**（每个工作树 2 个） | stash 不属于任何分支 |
| `git fsck --lost-found` 捞出的悬空提交 | **547 个** | 被删分支/重置后的孤儿提交，gc 后永久消失 |
| 未提交文件 | 39 个 | `trit-sweep` 21、`trit-claims` 11… |

**根因链**：智能体在独立 worktree 里提交 → 只留本地分支、从不 push → 会话额度耗尽被中断 →
工作树被清理/删除 → 分支成孤儿、**stash 直接消失**（`git bundle --all` 也不含 stash）→ gc 后彻底没了。
这不是某一方疏忽，是流程缺了"退出前必须留下可恢复副本"这一环。

**三条硬规则（默认执行，不需要讨论）**：

1. **阶段完成就 push**：`git push -u origin HEAD`（分支不必合并）。只有本地分支 + 未推送 = 随时会丢。
2. **不要长期留 `stash`**：`git stash pop`，或者立刻 `git stash show -p > 文件.patch`。stash 是仓库里最脆弱的东西。
3. **删 worktree / 清理目录之前，必须先归档**：
   ```bash
   bash scripts/archive-worktrees.sh          # bundle + 全部 stash + dirty + 未跟踪 → <归档根>/<日期>/full/
   bash scripts/rescue-dangling.sh            # 把 fsck 的悬空提交引用化并打包
   ```

**2026-11-21 已做的一次完整抢救**：

- 19 个工作树全量归档（185 MB）：每份 `bundle --all` + 每个 stash 的 patch + dirty patch + 未跟踪文件
- 547 个悬空提交引用化为 `refs/rescue/<hash>` 并打包（10.9 MB）
- 全部在 `<归档根>/2026-11-21/`；bundle 只含分支与标签，**stash 必须用同目录的 `*-stash<N>.patch` 恢复**

恢复一份：

```bash
git clone <归档根>/2026-11-21/full/<name>.bundle <dir>
git -C <dir> apply <归档根>/2026-11-21/full/<name>-dirty.patch     # 未提交改动
git -C <dir> apply <归档根>/2026-11-21/full/<name>-stash0.patch    # 第 1 个 stash
```
