# 任务：把「入库准入」从提交期扩到推送期、CI 与历史体检

> 派发对象：高能力智能体（可读全仓、跑命令、长时自主）。**没有本会话上下文**，按本文自包含执行。
> 标准：`docs/AGENT-TASK-STANDARD.md`。级别 **A**。

## 级别与当量

A（跨 3 个面：本地 hook / 脚本 / CI；有现成探针与自测范式）｜预计墙钟 1–2 天｜可并行 2 条线：
①推送期与 CI 预算（G1、G2）；②审计与历史体检（G3、G4、G5）。

## 起点

- 仓库：本项目根（主工作树 `wt/tconv-cap`；主仓库与 worktree 布局见 `docs/COLLAB.md §9`）
- 分支：`fix/mesh-open-cap`；起点 SHA：`a984135f` 之后那次「入库准入」提交（`git log --oneline -5 | grep admission`）
- 环境：`rustc 1.99.1`、`Python 3.14.7`、`bash`；**不需要**联网、不需要 GPU
- 已就位（本任务的前提，不要重做）：`scripts/repo_admission.py`、`scripts/hooks/pre-commit` 第 0 步、
  `docs/ADMISSION.md`、`.agent/admission.json`、`docs/ERRORS.md` 的 `L144`、`.agent/checks/registry.json` 的 `C094`

## 必读（按顺序，只读这几处）

1. `docs/ADMISSION.md` —— 三档判定、可接受产物表、9 条连锁表、开关口径
2. `scripts/repo_admission.py` —— 判定函数 `classify`、`report`、`self_test` 的写法（新能力照它的风格加）
3. `scripts/hooks/pre-commit` —— 第 0 步如何调用闸门
4. `docs/COLLAB.md §10` —— 工作防丢失三条硬规则（本任务不得与之冲突）
5. `scripts/secret_scan.py` 的 `self_test()` —— 自测必须"正例 + 逐条反证"

## 目标（每条都是可判真假）

**G1 推送期配额保护**（`scripts/hooks/pre-push` 与仓库内的 `scripts/prepush-admission.sh`）
输入：一次 `git push` 携带的提交集合 → 输出：拒绝或放行。
- 计算本次推送新增 blob 的**总字节数**与**最大单文件**（`git rev-list --objects` + `git cat-file --batch-check`）
- 默认阈值：总增量 20 MiB、单文件 5 MiB；超限 exit 1，并打印"哪几个文件、各多大、怎么处理"
- 阈值可被 `TRIT_PUSH_BUDGET_MB`、`TRIT_PUSH_MAXFILE_MB` 覆盖；`TRIT_ADMISSION=off` 时放行但仍打印风险
- 拒绝与放行都追加一行审计到 `.agent/admission.log`（与提交期同一文件、同一 JSON 形状，增加 `"stage": "pre-push"`）

验收：
```bash
# 正例：创建一个 3 MiB blob 的分支并推本地裸库 → 必须被拒
bash scripts/selfcheck-push-budget.sh    # 由本任务新增；内部用临时裸库，不碰 origin
```
期望：脚本退出 0，且其中三个断言全 PASS（超限拒绝 / 小改动放行 / 审计两行）。

**G2 CI 侧的产物预算**（`.github/workflows/ci.yml`）
- 所有 `actions/upload-artifact` 步骤加体积上限（默认 20 MiB），超限 fail 该 job
- 新增 `python scripts/ci_budget.py --artifacts` 能在本地列出将被上传的目录与体积（不联网）
- 在 `docs/COLLAB.md §6` 的门对照表加一行

验收：`python scripts/ci_budget.py --artifacts` 在干净工作树退出 0 并打印 `total=`；`--self-test` 正反例各 3 条。

**G3 关闭期审计汇总**（`scripts/repo_admission.py --audit`）
- 读 `.agent/admission.log`，输出：关闭次数、按 `disabled_by` 分组计数、风险路径 Top 10、时间范围
- 空日志 / 文件不存在：打印 `无关闭期记录` 并 exit 0

验收：`python scripts/repo_admission.py --audit --self-test` 用临时日志覆盖"空 / 3 条 / 损坏行"三种情况。

**G4 目录级批量门槛**
- `classify` 之外新增"目录聚合"：单次提交里同目录新增 > 100 个文件，视为批量导入，要求该目录在
  `.agent/admission.json` 的 `allow` 里显式登记（而不是整批写一句理由）
- 触发时打印目录、文件数、建议的 allow 条目 JSON

验收：`--self-test` 增加 3 条（99 个放行 / 101 个拦 / 已登记 allow 放行）。

**G5 历史体检（只报告，不改写）**（`scripts/admission_forensics.py`）
- 扫描历史里 ≥ 2 MiB 的 blob、命中高置信度密钥的 blob、扩展名在黑名单里的 blob
- 输出：数量、Top 20（路径 + 首次进入的 commit + 体积）、以及**建议**的 `git filter-repo` 命令（只打印，不执行）
- 保证不修改任何 ref：运行前后 `git rev-parse --all` 输出一致（脚本自己断言）

验收：`python scripts/admission_forensics.py --self-test` 在临时仓库（3 个提交，1 个 3 MiB blob）上跑通并断言
"报告含该 blob"且"ref 未变"。

## 非目标

- 不改 `docs/ADMISSION.md` 的判据口径，不提高 `max_mb` / `reason_mb`
- 不执行 `git filter-repo`、不强推、不碰 `main`、不删除任何分支或 `stash`
- 不把本机绝对路径写进仓内文档（`python scripts/docs_hygiene.py` 会拦）
- 不改 `scripts/secret_scan.py` / `scripts/ai_vendor_scan.py` 的判定标准（只允许调用）

## 验收（一次性可复算）

```bash
python scripts/repo_admission.py --self-test
python scripts/repo_admission.py --audit --self-test
python scripts/ci_budget.py --artifacts && python scripts/ci_budget.py --self-test
python scripts/admission_forensics.py --self-test
bash scripts/selfcheck-push-budget.sh
python scripts/module_budget.py
python scripts/docs_hygiene.py && python scripts/doc_pointers.py
python scripts/check_script_guards.py
```
期望：全部 exit 0；`module_budget` 报告 `debt=0`；`docs_hygiene`、`doc_pointers` 均 OK。

## 禁区

- 不推 `main`；不 `git push --force`；不使用 `--no-verify`
- 不提交 `target/`、权重、归档、日志（`repo_admission.py` 会拦，别去改它的策略绕过）
- 不抬高 `module_budget` 台账（新脚本单文件 ≤ 800 行，超了就拆）
- 不动 `scripts/hooks/pre-push` 里既有的版权护栏 v2 逻辑（只允许在其后追加预算检查）

## 失败模式与回退

- 推送体积算不准（浅克隆缺对象）→ 打印"无法判定，跳过并记审计"，**不猜**、不放宽阈值
- 历史体检在浅克隆上跑 → 明确输出 `shallow=true` 并说明结论只覆盖已取到的历史
- CI artifact 体积在本地测不出（依赖构建产物）→ 用 `du` 估算并在 CI 里做硬判，本地只给估算值
- 负结果（例如某类 blob 无法归因）→ 写进 `docs/ERRORS.md` 的 D 节"欠账"，不许静默跳过

## 交付物

1. 代码：上述 4 个脚本/hook + CI 改动，每个都带 `--self-test`
2. 文档：`docs/ADMISSION.md` 增加「§7 推送期与 CI 侧」；`docs/COLLAB.md §6` 门对照表加行
3. 台账：`docs/ERRORS.md` 新编号 + `.agent/checks/registry.json` 新 `Cxxx`（`kind: cmd`，`run` 串必须能一次跑通）
4. 原子交付：一个 PR，描述里贴 9 条验收命令的读数

## 自检清单

- [ ] 5 个目标各有独立验收命令，且我贴出了真实输出
- [ ] 新增脚本都有 `--self-test`，正例与反证数目写进 PR
- [ ] 没有新增盘符绝对路径（`docs_hygiene` OK）
- [ ] 没动 `main`、没强推、没 `--no-verify`
- [ ] `module_budget` `debt=0`
