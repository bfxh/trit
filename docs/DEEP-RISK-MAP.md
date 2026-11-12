# 深层风险地图：更新 / 认领 / 牵连

状态：2026-11-12 第二轮。上一轮（#419/#421/#422）把“共享工作树 + 冲突”堵住了；
这一轮处理用户点名的三类深层问题：**负更新（分支更新把东西改回去）、认领先到先得、改动牵连**。
文末附技术/论文对照与仍未修的清单——每条都写“怎么判”，不写“注意一下”。

## 1. 本轮落地的判据

### 1.1 负更新：`scripts/branch_update.py`

| 情形 | 判据 | 结果 |
|---|---|---|
| 新分支（远端 SHA 全 0） | — | 放行 |
| 快进 | `git merge-base --is-ancestor old new` | 放行 |
| 干净重写 | 非快进但 `git cherry <new> <old>` 里 old 的提交全是 `-`（补丁等价），再用 `git range-diff` 取证：旧提交在新历史里无对应 ⇒ 拦；补丁落点变化 ⇒ 列出人工确认 | 警告放行（rebase/amend 常态），矛盾证据 fail-closed |
| **负更新** | 有 `+` 行：old 上有提交在新历史里没有等价补丁 | **拦**，列丢失提交/删除文件 |
| **棘轮回退** | 测试面计数变小、模块预算上限变大/消失、CHANGELOG 字节变小、盘符路径计数变大。单位随文件/crate 一起删除不算回退（删减 ≠ 回退）；现存单位计数变小仍拦 | **拦** |
| 取证失败 | 旧提交对象不在本仓 / git 失败 | **拦**（先 fetch；看不见 ≠ 没问题） |

已接进 `pre-push`（稳定副本 `trit-update-guard.py`）：推任何 `refs/heads/*` 都先过一遍；
`ALLOW_NON_FF_PUSH=1` 只放行“丢提交”，`ALLOW_RATCHET_PUSH=1` 只放行棘轮，
两者分开——不给人一把“全放行”的钥匙。删除分支（local SHA 全 0）放行。

### 1.2 认领：不是先到先得（`scripts/agent_claims.py`）

- 名单是 read-modify-write；两个会话同时 `claim` 会拿到同一个 id（实测 `c3` 撞过）。
  现在 `claim/release/renew/repair/plan` 走**文件锁**（Windows `msvcrt` / POSIX `flock`）。
- `claim --plan` / `--plan-file`：方案存 `CLAIMS.plans/<id>.md`（长文本不进 8 列定宽表）。
- `compare --paths ... [--repo]`：把在效重叠声明的方案并排打出来；
  **没有方案 = 没有资格参与比较**，`compare` 判红（rc=1）。
- `plan --id cN --plan ...`：裁决前补方案也来得及。
- `repair [--apply]`：修历史并发留下的重复 id（默认 dry-run），方案文件跟着改名。
- 裁决标准写进 `AGENTS.md`：问题解释 / 兼容 / 牵连 / 回滚 / 验证，**不看谁先到**；
  败方 `release` 或收窄路径，重叠自然消失。

### 1.3 牵连：`scripts/impact.py`

改动文件 → 归属 crate（最深前缀）→ **反向依赖闭包**，输出风险档、建议门、以及
“会被牵连但没有任何活动声明覆盖”的 crate。实测本仓：

```text
$ python scripts/impact.py --files crates/trit-core/src/lib.rs
牵连面：1 个文件 → crates｜风险 high
  - 反向依赖闭包 37/44（>=66%）
  建议跑：cargo test --workspace
  仅测试面牵连（dev 依赖）：trit-float, trit-gpu
```

依赖按 kind 分账：**运行时 35/44**（normal + build），**测试面 37/44**（再加 dev 依赖）；
`dev_only` 单列。改 `trit-core` 一个文件牵连 35~37 个 crate——只认领 `crates/trit-core/src`
在“谁在动什么”的层面没漏，在“谁会被影响”的层面漏了一大片。`--strict` 把未认领牵连变成 rc=1。

### 1.4 三条“未证栏”的收口（2026-11-12 第二轮）

| 原缺口 | 现在怎么处理 | 读数/限制 |
|---|---|---|
| impact 闭包把 dev/build 混在一起，偏保守 | `load_graph` 按 `kind` 分账：normal+build → `runtime`，dev → `dev_dependents`；输出 `runtime` / `closure` / `dev_only` | 实测 trit-core：运行时 35/44、测试面 37/44、dev-only 2；风险按 runtime 判 |
| `git cherry` 的补丁等价可能忽略落点差异 | 等价时再跑 `git range-diff`：`<`（旧提交无对应）与等价结论矛盾 ⇒ 拦；`!` 且文件区有 +/- ⇒ 列出“落点不同（人工确认）”，不直接拦（rebase 常态） | 自测覆盖：amend 只改消息不误报；cherry-pick 到新基线会点名；冲突 rebase 照样走 `--allow-rewrite` |
| 棘轮回退对“故意删 crate/收缩测试”误拦 | 计数变小/上限消失时，先看新树里该单位/文件是否还存在；随文件删除 ⇒ 删减放行；文件还在 ⇒ 仍判回退 | 自测：删 `scripts/gone.py` + 删台账单位 ⇒ 绿；`scripts/u.py` 还在但计数 5→4 ⇒ 红 |

## 2. 仍未修（按“爆炸半径 × 可判性”排序）

| # | 问题 | 现状证据 | 建议判据（可复算） | 代价 |
|---|---|---|---|---|
| 1 | `trit-vcs` 中央仓 ref 更新无 CAS / 无鉴权 | `docs/VCS-BRANCH-ISSUES.md` §1.2、§2；主库侧只能在本机 pre-push 拦 | `ref update --old <sha>`：旧值不匹配 ⇒ 拒绝（结构化原因码）；并发两写只有一条成功 | 中（独立仓，需在那边加测试） |
| 2 | GitHub 服务端不执行仓库里的自定义 merge driver | 本地 `git merge` 实测生效；服务端仍按文本合并 | 合并队列 / 服务端可识别的 `.gitattributes` 策略；或接受冲突并在 CI 重跑 | 低-中（需 admin） |
| 3 | CI 只按 crate 闭包选测试；mutation 全量约 1 小时 | `scripts/ci_scope.py`；mutation.yml 90/120 分钟 | 改动→测试映射（历史失败 + 文件级），先 crate 内选择、保守回退全量 | 中 |
| 4 | feature 组合只在手工 step 覆盖 png/gpu；MSRV 只跑默认特性 | ci.yml 的 feature steps；MSRV job | 声明式 feature 矩阵（`cargo-hack --feature-powerset` 限定组），MSRV × 关键 feature | 低-中 |
| 5 | 无 API 兼容门 | 大多数 crate `publish=false`，但内部 API 仍会破 | `cargo-semver-checks` 对核心 crate + 例外登记 | 低 |
| 6 | 语义冲突（两边改动互相抵消）无检测 | `merge_forensics` 只查“整块丢失”；union 驱动只保并集 | 合并结果跑关键语义测试（DSL/eval/不变量），或结构化合并 | 高 |
| 7 | 棘轮只防放松，不防“误登记过紧” | `test_surface/module_budget --update` 只许收紧 | 台账变更需两条证据（测试输出 + 文件行数），或双人复核记录 | 低 |
| 8 | 牵连面未接入认领流程 | `impact.py` 只报告；claims 仍按显式路径 | 认领时自动跑 impact，未认领牵连列在 `check` 输出 | 低（本轮已备工具） |
| 9 | 分支/工作树堆积 | 14 worktrees、229 分支；阈值 180 已超 | 归档命名空间 + 定时 `worktree_guard --apply` + 未合并超期清单 | 低 |
| 10 | 一键更新未实现 | `docs/AUTO-UPDATE.md`（A-415 草案），`self_update.py` 不存在 | 先做 `--check`（干净树 + 快进预检 + 门读数，零写入） | 中 |
| 11 | 跨仓副本漂移（trit / trit-immune / trit-vcs） | claims 的 `crate_twin` 只提醒；无文件对账 | 双仓 manifest 哈希对账 + 冲突清单 | 中 |
| 12 | 平台覆盖：钩子只在 Windows Git Bash 真装过 | 本机 `.git/hooks`；CI 只跑自测 | Linux/macOS 安装脚本实机验证；镜像仓同步测试 | 低 |

## 3. 技术 / 论文对照（这些判据不是拍脑袋）

| 方向 | 依据 | 本仓对应 | 差距 |
|---|---|---|---|
| 任务分配按“投标质量”而不是先后 | Smith, *The Contract Net Protocol*, 1980（[PDF](https://www.reidgsmith.com/The_Contract_Net_Protocol_Dec-1980.pdf)） | `agent_claims.py` 的 `--plan` + `compare` + 败方 release | 质量判断仍是人/智能体做；工具只保证“有方案才能比、比完必须收窄” |
| 变更牵连（CIA / ripple） | Li et al., *A survey of code-based change impact analysis techniques*, STVR 2012（[PDF](https://zhang-sai.github.io/pdf/li-stvr12.pdf)）；Kretsou et al., *Change impact analysis: a systematic mapping study*, 2021（[ScienceDirect](https://www.sciencedirect.com/science/article/abs/pii/S016412122030282X)） | `impact.py` 反向依赖闭包 + 未认领牵连 | 只到 crate 粒度；没有函数/测试粒度，没有动态调用信息 |
| 按改动选测试 | Machalica et al., *Predictive Test Selection*, Meta 2018/ICSE-SEIP 2021（[arXiv](https://arxiv.org/pdf/1810.05286)、[工程博客](https://engineering.fb.com/2018/11/21/developer-tools/predictive-test-selection/)） | `ci_scope.py` 只做 crate 闭包 | 没有历史失败/覆盖率/测试级映射；“选少”与“漏测”的权衡未量化 |
| 结构化/语义合并 | Zhu et al., *Conflict Resolution for Structured Merge via Version Space Algebra*, OOPSLA 2018（[PDF](https://feihe.github.io/materials/oopsla18.pdf)）；Cavalcanti et al., *Semistructured versus Structured Merge*（[PDF](https://www.se.cs.uni-saarland.de/publications/docs/CBS+19.pdf)） | `merge_json_union.py`（确定性 union）、`merge_forensics.py`（整块丢失） | 只处理“台账/整块丢失”，没有语义冲突检测 |
| feature 组合兼容 | `cargo-hack` 的 `--feature-powerset`（[crates.io](https://crates.io/crates/cargo-hack)）；Cargo Book：features 与 SemVer（[doc.rust-lang.org](https://doc.rust-lang.org/cargo/reference/features.html)） | 手工 feature steps | 没有声明式矩阵；组合数会爆炸，需要分组 |
| 库 API 兼容 | `cargo-semver-checks`（[docs.rs](https://docs.rs/crate/cargo-semver-checks/latest)） | 无 | 先对核心 crate 跑，例外写清单 |
| 非快进/补丁等价 | Git `git cherry`（补丁等价）、`--force-with-lease`（CAS 式推送） | `branch_update.py`、pre-push 守卫 | 中央仓（trit-vcs）还没有同样的租约/CAS 语义 |

## 4. 下一批建议（按顺序，带验收判据）

1. **把 impact 接进认领**：`claim` 后自动跑 `impact.py --strict`，未认领牵连直接列在该声明输出里；
   验收：`claim` 改 `crates/trit-core/src` 必须列出 37 个下游 crate 并提示补认领/通知。
2. **`self_update.py --check`**（AUTO-UPDATE 第一步）：干净树 + 可快进 + 门读数，零写入；
   验收：脏树 rc=1 且 `git status --porcelain` 前后逐字节一致。
3. **mutation 分片**：`--shard i/n` + 按文件哈希分片；验收：4 片并行总时长 < 20 分钟，
   合并后 caught 集合与全量一致。
4. **feature 矩阵门**：先覆盖 `trit-texture png`、`trit-gpu gpu`、`trit-build gpu` 的
   “单独开 / 全开 / 全关”三态；验收：每个组合一条命令 + 失败点名到组合。
5. **trit-vcs CAS**：在独立仓加 `ref update --old` 与并发测试；主库侧把 `docs/VCS-BRANCH-ISSUES.md`
   的 §2 待查改成实测结论。
