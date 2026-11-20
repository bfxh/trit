# trit 智能体护栏手册（agent 必读）

## 这份手册为什么存在

接进这个仓库干活的智能体有两种常见死法：

1. **误操作毁树**——一次裸删、一次 `git push --force`、一次对整棵树的 `restore`，
   让本来正确的工作消失，而且事后无法解释"当时改了什么"。这类事故的共同点是：
   动作不可逆、事前没人拦、事后没有账。
2. **预算断粮死在半路**——会话中途上下文或额度耗尽，任务带着进度悄悄消失；
   接手的会话不知道做到哪一步、哪些结论已证实、下一步从哪继续。

对这两种死法，本仓的回答不是写更多规矩，而是把规矩做成 `tools/` 里**会拒绝你的代码**：
删除只有一条放行口（回收站），git 破坏面默认 DENY，断粮有账本，运行有资源帽。
护栏链上任何一道闸拒绝，整体拒绝——这是本手册最重要的性质。

任何在本仓干活的智能体：读完本手册再动手。动手规则（什么能做、什么不能做）见 `AGENTS.md`；
本手册讲的是**怎么安全地做**。真实事故台账见 `docs/ERRORS.md`（本轮对应 L95–L100）。

## 顺序即安全属性：一条链，四件工具

四件各自正确的工具，如果靠调用者记住顺序，就会在有人跳步时静默失效。
所以护栏链是**单一入口** `tools/agent_verify.py`（bash/PS 转发器 `tools/agent-verify.sh` /
`tools/agent-verify.ps1`），顺序即安全属性：

| # | 步骤 | 工具 | 拒绝效果 |
|---|------|------|----------|
| 0 | `memory-brief` | tools/memory.py | 只读，不拒绝（未初始化则 skipped，跳过也留痕） |
| 1 | `timeline-due` | tools/timeline.py | 只读，到期问题打印出来（rc=1 但不拦路） |
| 2 | `preflight` | tools/refactor_guard.py | 判红 ⇒ 整体拒绝，坏代码不落盘 |
| 2.5 | 预算闸（声明制） | agent-verify 内置 | 声明额度 ≤0 ⇒ run 前挂起，见协议三 |
| 3 | `checks` | tools/checks.py（fast 层） | 已固化检查判红 ⇒ 整体拒绝 |
| 4 | `affected` | tools/affected.py | 出测试计划；没有证明就回退全量（不拒绝，如实标注 unproven） |
| 5 | `policy` | tools/policy.py | 命令分类不通过 ⇒ 整体拒绝，run 永不开始 |
| 6 | `constraints` | tools/constraints.py | diff 触碰禁止操作 ⇒ 整体拒绝 |
| 7 | `journal-begin` | tools/journal.py | 建快照，此后发生的一切可证明、可还原 |
| 8 | `run` | tools/_run.py（`--caps` 时由 run_caps.py 包裹） | 真正干活只有这一步 |
| 9 | `journal-end` | tools/journal.py | 报告实际写了什么；还原走回收站（协议一） |
| 10 | `memory-note` / `timeline-record` | tools/memory.py / timeline.py | 留痕：绿色进记忆，失败进时间线（带时间戳） |

四件护栏工具嵌在链里的位置：

- **trash（回收站）**：policy 的唯一删除放行口 + constraints `no-raw-delete` 的对账对象
  + journal restore 内部的删除通道——三处共用一本账；
- **git_safe**：policy 拒绝 git 破坏面时给出的替代路径；
- **resume**：预算闸与命令失败检查点的账本；
- **run_caps**：`--caps` 时包裹 run 段的每条命令。

回执**纯 ASCII**（`ensure_ascii=True`）写在最后：机器产物必须能在任何解码器下活下来。
退出码：0 全部通过；1 有闸拒绝或命令失败；2 调用错误。**闸门步骤任一拒绝 ⇒ 整体拒绝**。

## 协议一：删除协议（绝不裸删）

**产品规则只有一句：工作区里不许裸删。** 删除的每条路都被堵死，唯一的口是回收站。

### 命令侧：policy 的 DELETION 家族

裸删程序（`rm` / `rmdir` / `rd` / `del` / `erase` / `Remove-Item(ri)` / `unlink` / `shred`）
及其走私形态（`find -delete`、`perl -e unlink`、`mv` 进 `/dev/null`、内联
`os.remove`/`shutil.rmtree`、`git rm`、解释器字符串里引号裹着的删除程序）在
workspace-write 档默认 DENY，`-Allow` 救不了它——这是有意的，回收站能留下内容和理由，
逃逸舱不能。真实回执（临时 fixture 实跑）：

```text
$ python tools/policy.py -Profile workspace-write -Command "rm src/core.py" -Root .
policy: DENY  deletion: 'rm' deletes content  (替代路径: python tools/trash.py put <path> --reason <why> -- 工作区不许裸删，这是唯一放行口)
policy: 拒绝执行
```

### 内容侧：constraints 的内核规则 no-raw-delete

policy 拦的是命令文本；绕过命令直接改树的删除由 constraints 兜底：diff 里整删已跟踪文件
（含 `+++ /dev/null` 盲区——git 对整文件删除不打印 `b/<path>`，L96 修的就是这个）而
回收站清单没账 ⇒ BLOCK。companion 是**回收站清单里的 entry**，不是注释——补注释救不了它：

```text
$ python tools/constraints.py check -Root <root>
constraints: BLOCK [no-raw-delete] a.rs (1 line(s)): (file deleted)
                      裸删 = 内容无处可寻；先 python tools/trash.py put <path> --reason <why>，回收站清单里有这条路径的 entry 才放行（命令侧对应 policy 的 DELETION 家族）
constraints: 拒绝 -- 1 条禁止操作（warn 0 条）
```

### 回收站：唯一放行口

`tools/trash.py`（转发器 `trash.sh` / `trash.ps1`）。站点与 journal 快照**同源**：
`-Store` 覆盖，否则 `<Root>/.journal`（已在本仓 `.gitignore`）。真实序列（临时 fixture 实跑）：

```text
$ python tools/trash.py put src/legacy.txt --reason "superseded by new module" -Root <root>
trash: put t20261121-055433-1 src/legacy.txt kind=file sha256=E32B7F63C7D89F75DD4B67933C58D5DD00FCF7CDF5C6443CE15B9DA5A9119B57 bytes=15 via=rename
trash: restore cmd: python tools/trash.py restore t20261121-055433-1 -Root <owner-root>
trash: 1 path(s) binned; site .journal/trash

$ python tools/trash.py list -Root <root>
trash: site .journal/trash (entries: 1, kept 1)
trash: t20261121-055433-1  kept     file src/legacy.txt  sha256=E32B7F63C7D8 bytes=15 reason="superseded by new module"
trash: restore cmd: python tools/trash.py restore t20261121-055433-1 -Root <owner-root>

$ python tools/trash.py restore t20261121-055433-1 -Root <root>
trash: restore t20261121-055433-1 -> src/legacy.txt via=rename
trash: verify sha256=E32B7F63C7D89F75DD4B67933C58D5DD00FCF7CDF5C6443CE15B9DA5A9119B57 bytes=15 ok

$ python tools/trash.py status -Root <root>
trash: site .journal/trash
trash: entries=2 kept=0 restored=1 purged=1 bytes-kept=0
```

规则逐条（全部 fail-closed，实测）：

- **`put` 必带 `--reason`**，一条 entry 记：id、路径、sha256、字节数、理由、恢复命令。
- **进站不丢内容**：同卷 rename（一步原子）；跨卷先 copy + sha256 对照账本核实，才移除源文件。
  没有任何一步内容存在于任何地方之外。
- **restore 拒绝覆盖**：落点被占就是错误，绝不 rename-over；restore 后逐字节校验。
- **purge 是唯一销毁步骤，id 打两遍**：少打一遍就拒绝：

```text
$ python tools/trash.py purge <entry-id> -Root <root>
trash: refuse -- purge needs --Confirm <the same entry id> typed again (explicit double confirmation, one missing is one too many)
```

- **站点没被 gitignore 时 put 拒收**（不能让回收站弄脏 `git status`，宁可拒）：

```text
trash: refuse -- the bin site is not git-ignored, so puts would dirty git status -- add it to .gitignore first (fail closed)
```

- **诚实边界原文**：`a rename ledger on the same machine, not a backup`——能写工作区的人
  也能清空回收站；它是账，不是异地备份。
- **journal restore 也走回收站**（ERRORS.md L95）：快照之后新增的文件一律 `trash.put_paths`
  进站（reason 记 `journal-restore:<快照时间>`）；回收站拒收时文件**留在原地**、restore 返回 1
  ——不完整的还原不许看起来像成功。回执尾部追加 `journal: trash <entry-id> <rel>`。

## 协议二：Git 协议（追加式）

**智能体对 git 的合法动作是追加式**：commit、branch、merge——凡重写或丢弃，默认 DENY。

### 破坏面清单（policy 拒绝）

`push --force` / `-f` / `+refspec`、`branch -D`、`tag -f`、`remote prune`、
`checkout -- <path>`、`restore` 的 worktree 侧覆盖（含裸 `git restore <path>`）、
保护分支上下文里的 `rebase`。真实回执：

```text
$ python tools/policy.py -Profile workspace-write -Command "git push --force origin main" -Root <root>
policy: DENY  destructive: git push --force/-f rewrites the remote  (替代路径: 先 python tools/git_safe.py checkpoint --label <name> 留快照，再由人工执行)
policy: 拒绝执行
```

放行语 `-Allow git-destructive` 是**逐次放行**：一次调用放一条，回执里大声打印
`allowed by -Allow git-destructive (per-invocation grant)`。每次都要重新声明——没有永久豁免。

### checkpoint：任务开始时、任何危险操作前

`tools/git_safe.py checkpoint`（转发器 `git-safe.sh` / `git-safe.ps1`）用临时索引 +
`write-tree` + `commit-tree` 打快照，挂在 `refs/recycle/checkpoint/<ts>-<label>`，
**不动 HEAD、不动 index、不动工作区**。真实回执：

```text
$ python tools/git_safe.py checkpoint --label before-risky-edit -Root <root>
git-safe: checkpoint 32484c77d870bd5814055aa06ef7e47ce307b4cd
git-safe: ref refs/recycle/checkpoint/20261121-055459-before-risky-edit
git-safe: tree 58c546349068887ceebfbe7c384257c840dd9b8c
git-safe: parent ceeb3b7792f69b8abec1b0c5032545ef01878792
git-safe: HEAD/index/worktree untouched
git-safe: restore:
git-safe:   git -C <owner-root> restore --source=32484c77d870bd5814055aa06ef7e47ce307b4cd --worktree -- :/
git-safe:   git -C <owner-root> restore --source=32484c77d870bd5814055aa06ef7e47ce307b4cd --staged --worktree -- :/
```

时机：**任务开始打一个；任何危险操作（大范围重构、手工放行的破坏性 git、影响多 crate 的
改动）之前各打一个**。标签写清楚为什么打（`before-rename-orch`，不是 `tmp`）。

### 出事了：rescue

`rescue` 打印 reflog 尾部 + 全部 checkpoint refs + **逐字可复制**的恢复命令块：

```text
$ python tools/git_safe.py rescue -Root <root>
git-safe: rescue -- everything below is copy-paste ready
git-safe: [1] reflog (last 10)
git-safe:   ceeb3b7 HEAD@{2026-11-21 05:54:59 +0800}: commit (initial): init
git-safe: [2] checkpoint refs (1)
git-safe:   32484c77d870  refs/recycle/checkpoint/20261121-055459-before-risky-edit  recycle checkpoint 20261121-055459 before-risky-edit
git-safe: [3] restore commands (newest checkpoint first)
git-safe:   # recycle checkpoint 20261121-055459 before-risky-edit
git-safe:   git -C <owner-root> restore --source=32484c77d870bd5814055aa06ef7e47ce307b4cd --worktree -- :/
git-safe:   git -C <owner-root> restore --source=32484c77d870bd5814055aa06ef7e47ce307b4cd --staged --worktree -- :/
git-safe: last resort (moves HEAD and the branch; policy DENYs this for agents):
git-safe:   git -C <owner-root> reset --hard 32484c77d870bd5814055aa06ef7e47ce307b4cd
```

`verify <ref>` 核对一个 checkpoint 还在并打印其 tree/parents。**边界原文**：
`checkpoint refs live in this clone's .git`——`gc`/`prune`/删 ref 会丢掉它们；
`.gitignore` 的文件不进快照（`git add -A` 服从 gitignore）；打印出的恢复命令**故意不执行**，
由人工运行。

## 协议三：断粮协议（censored 不是失败）

**任务中途断粮是"观测在结局之前结束"，不是失败。** 所以账本里没有 `failed` 这个状态，
状态机是：`open → suspended → finished / censored`。工具 `tools/resume.py`
（转发器 `resume.sh` / `resume.ps1`），session 存 `<Root>/.agent/resume/<task-id>.json`
（只含 id、时间戳、状态，不含机器路径）。完整循环，真实回执（临时 fixture 实跑）：

```text
$ python tools/resume.py begin fix-parse-bug --budget-token 40000
resume: task fix-parse-bug open (budget=40000)

$ python tools/resume.py progress fix-parse-bug repro --note "minimal sample reproduces the panic" --proven
resume: task fix-parse-bug +repro (proven) steps=2

$ python tools/resume.py suspend fix-parse-bug --reason budget
resume: task fix-parse-bug suspended (reason=budget) -- censored, not a failure
resume: continue later with: resume.py resume fix-parse-bug
```

换一个会话，接手者只需要读 `resume` 的输出——已完成步骤 + 逐字可复制的续跑命令块：

```text
$ python tools/resume.py resume fix-parse-bug
resume: task fix-parse-bug  status=suspended  steps=2  budget=40000
resume: censored at 2026-11-21T05:55:19.093694+08:00, reason=budget -- censored is not a failure
  - patch  unspecified
  - repro  proven  minimal sample reproduces the panic

--- resume checkpoint (copy verbatim) ---
# task fix-parse-bug: done=patch,repro, last=repro proven
python tools/resume.py progress fix-parse-bug <next-step> --proven # <next-step>: the plan's next step (ledger does not hold the plan)
python tools/resume.py finish fix-parse-bug # close the task when everything is done
python tools/resume.py suspend fix-parse-bug --reason budget # stop cleanly if the budget runs out again: censoring, not failing
------------------------------------------
```

规则逐条：

- `suspend --reason` 只有三值：`budget` / `deadline` / `manual`；
- `progress` 对 suspended 会话先隐式 resume（打印并记入历史）；
- `finish` 对 suspended 会话 ⇒ `censored`（关闭而未完成，**明确不是失败**）；对 open ⇒ `finished`；
- 已关闭的会话拒绝一切写操作（begin 撞已有 session 也拒绝——被静默覆盖的账本就是丢了历史）；
- task-id 必须匹配 `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`（session id 永远成不了路径）。

### 预算闸 --budget-token：声明制，说穿

工具层**测不了模型侧 token 消耗**，所以 `--budget-token N` 是调用者的**声明**：
"我认为我还剩 N"。声明 ≤0 ⇒ run 之前挂起（需要 `--task-id` 才挂账），不执行任何命令：

```text
$ python tools/agent_verify.py -Root <root> -Changed src/core.py -Lang python -DryRun \
    --budget-token 0 --task-id guardrail-demo
agent-verify: budget gate -- declared 0 <= 0, suspend before run
resume: task guardrail-demo suspended (reason=budget) -- censored, not a failure
resume: continue later with: resume.py resume guardrail-demo
agent-verify: suspended (budget) before run -- censored is not a failure
```

回执（JSON）如实记录声明与闸的性质：`verdict = "suspended (budget) before run"`、
`exit = 1`、`budget = {"declared": 0, "gate": "declaration-based gate: the tool layer
cannot measure model-side token usage; ..."}`。**不传这个 flag 就没有闸**——闸门诚实于它
能知道什么；声明错了它救不了你，所以声明要保守。

### 命令失败自动挂起

`--task-id` 下 run 段任何命令失败：自动 `progress run:<序号> --unproven --note "exit N: <命令>"`
+ `suspend --reason manual`。回执 verdict 保持 `command failed`，只**追加**字段（旧字段不动），
并给出 `doc["resume_checkpoint"] = "task <id> suspended (manual) -- next session:
python tools/resume.py resume <task-id>"`。

## 协议四：资源帽协议（何时开 --caps）

`tools/run_caps.py`（转发器 `run-caps.sh` / `run-caps.ps1`）给**单条命令**套内核级资源帽：

```text
run-caps --mem-mb N --procs N --seconds S -- <cmd> [args...]
```

三顶帽子，逐顶说诚实：

- `--mem-mb`：每进程已提交内存帽。Windows 是 Job Object 的 `PROCESS_MEMORY` 限制——
  **内核**让超帽分配直接失败（进程吃到硬分配错误自己死）；POSIX 是 `RLIMIT_AS`。按进程计，不是全树求和；
- `--procs`：job 内活跃进程数帽。Windows 内核强制（第 N+1 个 `CreateProcess` 直接失败）；
  POSIX **故意没有等价物**——`RLIMIT_NPROC` 按用户计数，会误伤同账号的所有进程。如实说明，不假装；
- `--seconds`：父进程看门狗（不是内核计时器）。超时杀整个 job（父、子、孙），退出码 124。

真实回执（临时 fixture 实跑）：

```text
$ python tools/run_caps.py --seconds 2 -- python -c "import time; time.sleep(10)"
run-caps: seconds=2 exceeded -- job killed (exit 124)
```

`KILL_ON_JOB_CLOSE`：父进程自己死了，系统关掉 job 句柄，job 里所有进程陪葬。
校验失败 **exit 2 不降级**——绝不出现"本该加帽却没加帽继续跑"：

```text
$ python tools/run_caps.py --mem-mb 0 -- python -c "print('x')"
run-caps: --mem-mb must be >= 1        # 退出码 2
```

### 诚实边界与分工

原文：`kernel-level resource caps, NOT a security boundary`——不隔离文件系统与网络，
防不了蓄意逃脱的进程。与 policy 的分工一句话：**policy 拦误用（事前分类，命令没跑就拒），
caps 限制已放行命令失控的爆炸半径（事后兜底）。一个把门，一个装围栏，谁也不冒充谁。**

### 何时开

- 长构建、测试矩阵（agent-verify `--caps` 默认 mem 2048 MB / 64 procs / 1800 s）；
- 不熟悉的生成器或脚本第一次跑；
- 任何可能 fork 轰炸或内存膨胀的命令。

在 agent-verify 里开：`--caps`（可调 `--caps-mem-mb / --caps-procs / --caps-seconds`），
run 段每条命令被包裹为 `run_caps.py ... -- python _run.py ...`——帽子罩住 runner 与它
派生的一切（job 成员资格继承），回执 run 条目追加 `caps` 字段留证据。

## 防什么 / 不防什么（诚实边界总表）

| 工具 | 防 | 不防（诚实边界） |
|------|----|------------------|
| policy | 事前拦：DELETION 家族、git 破坏面、越界写、未授权网络 | 不是内核沙箱；拦不到工具**内部**的 unlink（由 constraints/journal 补）；`-Allow` 逐次放行之后靠 journal 取证 |
| trash | 裸删不可逆：内容与理由留账 | 同机 rename ledger，**不是备份**；能写工作区的人也能清空回收站 |
| git_safe | 不可逆 git 操作之前有快照、出事有 rescue | refs 只在本 clone 的 `.git`，gc/删 ref 会丢；ignored 文件不进快照；恢复命令人工执行 |
| constraints | diff 里的禁止操作（整删 `/dev/null` 盲区已修） | 只看 diff 与给定命令文本；规则没写的问题不响 |
| resume | 断粮/中断丢上下文：有账、有续跑块 | `budget` 是声明制，测不了模型侧真实 token；账本不替你记计划本身 |
| run_caps | 已放行命令的爆炸半径（内存/进程数/超时） | **不是安全边界**：不隔离文件系统/网络，防不了蓄意逃脱；POSIX 无 procs 帽等价物（如实说明） |
| agent-verify | 调用者跳步：单一入口、闸门串联、回执可审计 | 回执写完之后的事它管不着；不传 `--budget-token` 就没有预算闸 |

一条通则：**每个工具的边界声明跟着工具走**（usage 文本、回执字段里都带），不单独住在文档里。
文档会过时，工具打印的边界不会。

## 快速命令块（逐字可复制）

以下 `<...>` 是占位符；`-Root .` 在仓库根执行。bash/PS 转发器（`tools/trash.sh` 等）
与 python 直调行为一致；回执里工具自己打印的恢复命令用 python 直调形态，照抄即可。

```bash
# A. 删除东西（唯一的删除方式）
python tools/trash.py put <path> --reason "<为什么删>" -Root .
python tools/trash.py list -Root .            # 找 entry-id
python tools/trash.py restore <entry-id> -Root .
python tools/trash.py purge <entry-id> --confirm <entry-id> -Root .   # id 打两遍

# B. 任务开始 / 危险操作前：打 checkpoint
python tools/git_safe.py checkpoint --label <为什么打> -Root .
python tools/git_safe.py list -Root .
python tools/git_safe.py rescue -Root .       # 出事后跑这个，照抄恢复块
python tools/git_safe.py verify <ref> -Root .

# C. 断粮循环：挂起 → 换会话 → 续跑
python tools/resume.py begin <task-id> --budget-token <剩余额度>
python tools/resume.py progress <task-id> <step-id> --proven --note "<做了什么>"
python tools/resume.py suspend <task-id> --reason budget
python tools/resume.py resume <task-id>       # 新会话里第一个命令
python tools/resume.py finish <task-id>

# D. 干活：先排练，再真跑
python tools/agent_verify.py -Root . -Changed <逗号分隔的改动文件> -Lang python -DryRun
python tools/agent_verify.py -Root . -Changed <改动文件> -Lang python \
  --budget-token <剩余额度> --task-id <task-id> --caps \
  -Receipt .agent/last-receipt.json

# E. 单条命令套资源帽
python tools/run_caps.py --mem-mb 2048 --procs 64 --seconds 1800 -- <命令>
```

## 验收口径

改完任何护栏相关的东西，验收只有一条命令：

```bash
pwsh tools/selftest.ps1
```

结尾行必须长这样（实测）：

```text
selftest: passed=237 failed=0 skipped=0
```

**必须 237 条全绿。** A–Z 26 组（W 组 trash 18 条、X 组 git_safe、Y 组 resume 32 条、
Z 组 run_caps 是本轮新增）。护栏的断言全部遵循"先看着它响，才算数"：每条拒绝路径都有
对应的判红用例（W13/W14 journal restore 走回收站、W20/W21 no-raw-delete、Z1/Z4/Z5
run_caps 内核帽、I2b 编排顺序、M9 bash/PS 台账互通）。本轮真实台账在 `docs/ERRORS.md`
L95–L100：L95 journal restore 裸删改走回收站；L96 整删盲区与 no-raw-delete 内核规则；
L97 run_caps 两个内核常量凭记忆写错；L98 selftest 断言与编排层漂移；L99 夹具写死本机路径；
L100 中文控制台编码伪红。

### 回执怎么读

agent-verify 的回执（`-Receipt` 指定的 JSON，或 stdout 尾部）按三处读：

1. **`verdict`**——只有这几种，各是什么一眼定论：

| verdict | 含义 | exit |
|---------|------|------|
| `dry-run` | 排练通过，还没真跑 | 0 |
| `passed on proven subset` | 在带证明的子集上通过 | 0 |
| `passed on full run (plan was unproven)` | 没有证明，跑的是全量——通过了也算全量成绩 | 0 |
| `refused by preflight / checks / policy / constraints` | 对应闸拒绝，run 未开始 | 1 |
| `suspended (budget) before run` | 声明额度 ≤0，run 前挂起（censored，不是失败） | 1 |
| `command failed` | 命令真跑了且失败（有 `--task-id` 时已自动挂起） | 1 |

2. **`steps[]`**——每步 `{step, verdict, exit, seconds, output(尾部≤6行)}`；
   跳过的步骤以 `skipped` 留痕（跳过也算一步，不允许"跳了就消失"）。

3. **`run[]` / `changes[]`**——每条命令的 exit 与耗时；journal-end 报告实际落盘的
   `added/modified/deleted`，这就是"改了什么"的证据链。
