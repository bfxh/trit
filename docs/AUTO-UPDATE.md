# 自动更新与一键更新（任务草案）

状态：**第一步已实现**（`--check`；账本登记 A415）。`--apply` 仍未实现。
目标：一条命令把主库的工具链、依赖、生成台账与门状态对齐到最新，并给出可回滚的证据。

## 1. 为什么需要

- 多 worktree / 多会话并行是常态，手工 `git pull` + `cargo update` + 重新生成台账容易漏项。
- 台账（`module_budget` / `test_surface` / `mutants`）与依赖锁是“只许收紧”的棘轮；漏更新会让门在别人 PR 上红。
- 跨仓（`trit-vcs`、`repo-tools`）更新没有统一入口。

## 2. 一条命令的边界（`scripts/self_update.py` 草案）

```text
python scripts/self_update.py [--check|--apply] [--offline] [--json]
```

| 步骤 | 做什么 | 失败时 |
|---|---|---|
| 1 预检 | 工作区必须干净；当前分支与 upstream 可快进；不在 detached HEAD | 拒绝，不改任何文件 |
| 2 同步 | `git fetch` + 只快进到 `origin/main` | 停在旧 SHA 并报告 |
| 3 依赖 | `cargo fetch --locked`；只有 `--update` 才跑 `cargo update` | 回滚 `Cargo.lock` |
| 4 台账 | `test_surface --update`；`module_budget` 只读校验 | 不写台账，报告差异 |
| 5 门 | fmt / clippy / doc / module_budget / test_surface / hazard / layers / docs_hygiene | 报告红项 |
| 6 报告 | 旧 SHA → 新 SHA、依赖差异、门结果、回滚命令 | — |

- `--check` 只做预检 + fetch，不落地。
- 默认不自动提交、不自动推、不自动合并；“一键更新”指**工作区与门**，不是替用户做发布决定。
- 脏工作区一律拒绝（与 `trit-vcs pull` 的“脏则拒”口径一致）。

## 3. 与 trit-vcs 的关系

- `trit-vcs pull` 已做“过滤后的树清单 + 稀疏物化”，但**不动客户端 HEAD、不删本地多余文件**（账本 A344 自曝缺口）。
- 自动更新的最终形态应复用 `trit-vcs` 的 ref/commit 语义：更新 = 快进 ref，分叉 = 显式拒绝，不静默覆盖。
- 在 `trit-vcs` 补齐 merge / branch update 之前，主库侧只做 Git 快进 + 门校验。

## 4. 验收判据（实现时补读数）

- 干净工作区：`--check` 前后 `git status --porcelain` 一致（不写任何文件）。
- 脏工作区：rc=1，零改动。
- 快进失败 / 分叉：rc=1，输出旧 SHA、新 SHA 与分叉点。
- 离线：`--offline` 只跑本地门，不访问网络。
- 同一 SHA 下，`self_update` 的门结果与逐条手工跑一致。

第一步（`--check`）实测读数：

- 自测 22/22：夹具 git 仓真跑 fetch/clone/push，覆盖 干净最新 / 脏树点名 / 落后可快进 /
  分叉打印分叉点 / detached / offline 无 origin / fetch 失败 / `cargo fmt --check` 红绿 /
  python 门红绿 / 零写入逐字节自证。
- 变异 6/6 全抓住：脏树不拦 / 分叉不拦 / detached 不拦 / check 不 fetch / 门红不判红 /
  fmt 用写模式（零写入自爆）。
- 门清单按存在性挑：`cargo fmt --all -- --check` + module_budget / test_surface /
  docs_hygiene / doc_pointers / hazard_gate / layers；空集明说「不是绿」。

## 5. 未证栏

- `--apply` 未实现（只交付 `--check`）；没有与 `trit-vcs` 的 ref 更新对接。
- `cargo update` 的依赖升级需要人审（semver / 安全），草案默认不做。
- 零写入自证是防御性断言：工具不写文件时它永远绿，没有独立反证（等价变异不登记）。
