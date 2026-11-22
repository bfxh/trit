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
本手册讲的是**怎么安全地做**。真实事故台账见 `docs/ERRORS.md`（本轮对应 L95–L108、L114 与 L117）。

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

与成本台账打通（agent-verify 自动把每次 run 的实际消耗记进 `<Store>/cost-ledger.json`，
见下"成本台账"）：当本会话 scope（`--task-id` / `--agent-id`）在台账里已有 token 记录
（来自此前 run 的 `--spent-token` 声明），闸自动扣减——**剩余预算 = 声明值 - 已记值**；
没有 token 记录就维持纯声明制并如实标注（`recorded_tokens` 为空）。wall 记录不参与
扣减——秒不是 token，ledger.py 分列就是为了这个。台账损坏时闸 fail-closed
（`refused by budget ledger`），绝不静默降级成弱闸。

### 命令失败自动挂起

`--task-id` 下 run 段任何命令失败：自动 `progress run:<序号> --unproven --note "exit N: <命令>"`
+ `suspend --reason manual`。回执 verdict 保持 `command failed`，只**追加**字段（旧字段不动），
并给出 `doc["resume_checkpoint"] = "task <id> suspended (manual) -- next session:
python tools/resume.py resume <task-id>"`。

### 成本台账：run 段实际消耗自动上账（journal-end 后、memory-note 前）

agent-verify 每次跑完自动把实际消耗写进 `<Store>/cost-ledger.json`（ledger.py 的读改写，
同一 accounting 规则：totals derived, never accumulated）：每条 run 命令一条 entry
（命令级 key = runid|cmd<序>|内容哈希——重跑同命令是新 entry，不是去重命中）+ 一条
overhead entry（台账墙钟总和 = 链墙钟）+ 可选一条 tokens entry。`--agent-id` 落到每条
entry；verdict 尾部**追加**一行 `cost: wall=...s +...s overhead tokens=... ledger=... scope=...`
（旧回执字段零改动）。诚实口径跟着走（回执 `cost.note` 原文）：wall 是工具实测；
tokens 只在调用者声明时记录（`--spent-token N` = harness 实测的模型侧消耗）——**工具层
测不了模型侧 token**。costprobe 的测量面保持独立（measure-in-place、fail-closed、单
mtime、derived totals 四条硬规不被编排链碰）；编排链只是另一个 ledger **写入方**。
并发正确性：编排链全程持有 `<Root>/.journal/chain.lock`（O_CREAT|O_EXCL）——同 root
第二条链 fail-closed 拒绝（verdict `refused by chain lock`，诊断点名持锁 pid 与
`journal.py status/restore` 恢复命令），不与它竞争 journal-begin；被杀进程**故意**留下
锁 + 快照——那就是下轮运行的脏状态标记，清锁后快照仍可还原。

### 只读门并行化（--parallel-gates，可证独立才并行）

affected / policy / constraints 三个只读分类门在 opt-in 下并发：三门均无共享状态
（affected 只写自己唯一的临时 plan 文件，policy/constraints 只读），policy 逐命令再
扇出到线程池（它的开销是每命令一次子进程生成）。任一拒绝整体拒绝语义不变——拒绝按
链序评估，回执步骤按链序记账（不是完成序）；policy 拒绝时并发已完成的 constraints
结果被丢弃不记账（串行语义没走到它的判定点）。journal-begin 及其后**严格串行**（写
面）。保守声明：preflight/预算闸/checks 带顺序语义不并行；任何门将来长出共享状态就
回到串行。串行仍是默认（老回执逐字节不变）。

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

## 协议五：分诊与研究语料（signal.py）

协议一到四拦的是**动作**；协议五观测的是**状态**。`tools/signal.py` 在编排链之外：
它不拦任何命令，做的是把"活儿"和"情绪"快速分开（classify），把惩罚信号记账（penalty），
给下一轮注入一行摘要（brief），把台账投影成研究语料（export）。真实回执（临时 fixture 实跑）：

```text
$ python tools/signal.py classify --text "frustrated with the parser, fix parse_date in src/parse.py" --root <root>
{"kind": "mixed", "emotion_score": 1, "signals": ["frustration", "imperative"], "task_hints": ["fix", "src/parse.py"]}

$ python tools/signal.py penalty --score 2 --reason "user had to repeat the same instruction twice" --root <root>
signal: penalty score=2 recorded (penalty events in ledger: 1)

$ python tools/signal.py brief --root <root>
mood: frustrated(score=2, 1 event in last day) -- reflect harder, do not argue user intent

$ python tools/signal.py export --jsonl <out> --root <root>
signal: exported 2 event(s) -> corpus.jsonl (with-text=no)
```

导出的每行只带结构（`{ts, kind, emotion_score, penalty, text_len, signals}`），不带原文——
**默认输出是脱敏的**，`--with-text` 是显式选择，而且树内拒收（会被提交）：

```text
$ python tools/signal.py export --jsonl <root>/with-text.jsonl --with-text --root <root>
signal: refuse -- a --with-text corpus inside the owner root risks being committed -- write it outside the root (fail closed)
```

规则逐条：

- **classify 是规则匹配，不是模型判断**：关键词表固定在模块顶部（`--rules` 可整体替换），
  毫秒级、不调用任何东西。没有信号命中 → `kind=task` 空列表——这是**分诊默认值，不是证据**。
- **penalty 收的是 1..3**：`--score 0` 拒收（0 承载不了信号——拒收，不是记录）；
  没有 `--reason` 拒收（一个没说原因的数字不是信号）。
- **penalty 改变的是 harness 下一步做什么**（更重的反思、更紧的闸），**不是模型行为**——
  这个工具不调用任何 API、控制不了任何东西。它是给 harness 与未来模型训练用的信号。
  边界原文：`penalty cannot change model behavior by itself: this tool calls no API and
  controls nothing -- what it changes is what the harness does next (heavier reflection,
  tighter gates)`。
- brief 只看最近 24h（峰值分决定标签，过期事件不说话），一行 ASCII，给下一轮记忆注入。
- 台账 `<Root>/.agent/signals.jsonl` 本地存原文；超过 8192 字符的输入分诊并计数但不存
  （`text:''` 意思是"没存"，**永远不是**"空输入"）。任何一行坏了，brief/export 拒绝而不是跳过。
- **边界原文**：`classify is rule triage, not a model judgment -- keyword rules with unknown
  false-positive/negative rates, and emotion_score is a signal to weigh, never a fact to
  trust`。无网络访问。

## 协议六：注释预算（declutter.py）

**产品规则：少写注释。** 说明进 `docs/`，模型直接读代码——注释是债，不是文档。
`tools/declutter.py` 四个模式：量（audit）、棘轮（budget）、出无注释阅读视图（export）、
显式命令才真剥（clean）。两种注释**永远不剥**：

- **LANDMINE**：含 `LANDMINE` 标记的注释永不剥（audit 单独列出）——这是老问题的豁免通道：
  一碰就要全系统陪葬的那种。标记是豁免，不是风格，别往新注释上撒；
- python shebang（剥了它脚本就跑不了——那是行为，不是评论）。

真实回执（临时 fixture 实跑，夹具两个文件、7 行注释）：

```text
$ python tools/declutter.py audit --root <root>
declutter: audit files=2 comments=7 code=7 density=1.000 landmines=1 undecodable=0
declutter: src/app.py  lang=python  comments=4  code=4  density=1.000
declutter: src/lib.rs  lang=rust  comments=3  code=3  density=1.000
declutter: landmine src/app.py:7  # LANDMINE: legacy date parsing assumes local timezone; see ERRORS L15

$ python tools/declutter.py budget init --root <root>
declutter: budget baseline written files=2 total-comments=7 at .agent/comment-budget.json

$ python tools/declutter.py budget check --root <root>      # 夹具加了两行闲话注释 + 新文件
declutter: GREW: src/app.py 6 > baseline 4
declutter: UNCAPPED: src/main.rs (1 comment lines) is not in the baseline -- budget init registers it
declutter: budget check RED -- 2 file(s) over baseline, baselines only tighten
declutter: GREW src/app.py 6 > baseline 4 -- init never raises a baseline
declutter: budget init REFUSED -- a baseline is only ever tightened
```

棘轮是**只紧不松**：`check` 对 GREW（超基线）、UNCAPPED（没进基线）、基线读不了（fail closed）
判红；STALE/ORPHAN 只是提示。`init` 对已有基线只收紧——登记新文件、缩陈旧上限、删孤儿——
**任何文件变胖时直接拒绝**，上限永不抬升（抬升就是把肥肉合法化）。

`export --strip` 出**无注释阅读视图**：树拷到树外 `--out`（树内拒收），一行对一行，
file:line 引用不漂；源码树永远不被碰。`clean` 是唯一改树的通道，**缺旗标拒绝**：

```text
$ python tools/declutter.py clean --root <root>
declutter: clean refused -- --in-place is missing
  clean is the destructive pass: it rewrites files in the source tree, so
  it must cost an explicit flag. ...

$ python tools/declutter.py clean --in-place --root <root>
declutter: audit-before files=3 comments=8 code=7 density=1.143 landmines=1 undecodable=0
trash: put t20261121-074305-1 src/app.py kind=file sha256=82A0F2D522EF61F603B492E14714A91EC30C0972D0B92893C09F50F861B47B6F bytes=222 via=rename
declutter: clean src/app.py removed=2 original=t20261121-074305-1
...
declutter: audit-after files=3 comments=2 code=7 density=0.286 landmines=1 undecodable=0
declutter: clean done files=3 originals restorable from the bin
```

clean 逐文件：**原件先进回收站**（`trash.put_paths`，reason `declutter-clean`），剥完写回、
写回再扫描自检；任何失败整轮中止（fail closed），原件用回收站 entry 恢复。剥完剩下的 2 条
注释正是 LANDMINE 与 shebang。

**诚实边界**（usage 原文要点）：字符串里的注释形态**不保证**识别（三态扫描尽力而为）；
python docstring 是字符串——不剥也不计；js 正则字面量靠末 token 启发式，猜错损害限一行；
**unterminated string 时扫描器把剩余全文当字符串——宁可不动，绝不错删**；非 UTF-8 文件
跳过（audit/clean）或原样拷贝（export），从不猜编码。

## 协议七：观察与衰减（watch.py）

`tools/watch.py` 是事件台账 + 观测面：发生了什么、谁干的、在哪个轮位、整盘还健康吗。
台账 `<Root>/.agent/watch.jsonl`，每行固定六键 `{ts, event, agent_id, turn, verdict, note}`。
**事件标签自由**（不设白名单——harness 造词的速度快过白名单老化）；`report` 只数认得的
结果标签（cmd/test/build/check/commit/verify）且必须带 verdict——自由标签进不了分母。

- `record`：追加一行。拒绝机器绝对路径（盘符、`/Users/`、`/home/`、`/mnt/`）——事件文件随仓提交；
- `report`（**疲劳面**）：失败率按轮位分桶（early 1..10 / late 25+）。两桶各 ≥4 观测、
  后期失败率比早期上行 ≥20 个百分点 → 一行 WARN 给 harness：

```text
$ python tools/watch.py report -Root <root>
watch: report counted=9/10 skipped_malformed=0
watch: early turns 1-10: obs=4 fails=0 fail-rate=0.0%
watch: late turns 25+: obs=4 fails=4 fail-rate=100.0%
watch: commands per turn: median=1 max=1 (turn 1)
watch: fail streak: longest=4 trailing=0
watch: agents: -=7, sub-7=2
watch: WARN fatigue suspected at turn 28 (fail-rate 100.0% vs 0.0% early) -- checkpoint & resume in a fresh turn
watch: boundary: fatigue is inferred from behavioral evidence (outcome rates by turn position) -- behavioral evidence inference, not mind-reading; a checkpoint reminder for the harness, not a diagnosis of the model
```

- `health`（**整盘健康**）：四报警面，每条带"再这么干会出什么事"：

| 报警面 | 触发 | 后果一句话（回执原样打印） |
|--------|------|---------------------------|
| fail-streak | 连续失败 ≥3 | 每条下一条命令都建在红地基上——先 checkpoint 修好再继续，否则失败复利 |
| dirty-drift | git status 脏路径 ≥25 | 断粮时现场无法干净还原——小步提交或 checkpoint |
| giant-diff | diff ≥600 行 | 巨型 diff 没法逐块 review 或回滚——一次回滚好坏一起丢 |
| uncommitted | ≥2 天没 commit 且树上有改动 | 只存在于工作树的活儿一条坏命令就能抹掉 |

- `pollution`（**污染面**）：从事件流里挑模式——提 `--no-verify` 的 commit、
  `changed-lines=N`（N≥600）巨型 diff、拒收后绕行链（10 个事件内先 refused/rejected/blocked/denied
  后 --no-verify/--force/bypass/workaround/skip-checks/skip-hooks）。
- **分工原文**：`watch observes and reminds; policy/constraints block -- nothing here refuses
  a command, exit 1 only means there is something to look at`。观测与拦截不冒充彼此。
- **子智能体参与记录**：子智能体跑 agent-verify 时带 `--agent-id <id>`——id 进回执顶部与
  每条 run[] 行（**只追加字段**，旧回执字段不动），并经 `ledger.py add --agent-id` 落到每条
  成本行；watch 台账行级 `agent_id` 同为**追加式**——没有 id 的旧数据显示 `-`，不改造旧数据。
- **边界原文**：`behavioral evidence inference, not mind-reading`——工具看不见模型，只看得见
  痕迹；疲劳线是按轮位分桶的结果率，是给 harness 的 checkpoint 提醒，不是对模型的诊断。

## 协议八：主库删除审批与痛点语料（policy/constraints/pitfalls）

**主检出判定只有一个机器无关判据：`.git` 是目录。** 在检出根，git 给主检出留的是 `.git`
**目录**，给链接 worktree 留的是 `.git` **文件**（gitdir 指针）。判定从 `-Root` 向上走
（子目录根也能找到所属检出），完全没有 `.git` 就没有主库规则。如实写明的例外：
`git init --separate-git-dir` 的主检出也是文件形态，会被判成 worktree——用这种布局的话，
主库删除审批不会生效，审批注记里要说明。**危险方向**是把主检出误判成 worktree——删除
就只剩回收站要求、没人批过；反方向只多花一次审批。所以形态不可判时没有可判的东西：
目录=main，文件=worktree，缺席=空。

主库删除是**人批的决策**，不是工具决策。命令侧（policy）与 diff 侧（constraints）同一口径：

```text
$ python tools/policy.py -Profile workspace-write -Root <main-root> -Command "rm src/old.py"
policy: DENY  deletion: 'rm' deletes content  (main tree delete needs human approval — rerun with -Allow main-delete --approval-note "<谁批的批什么>")
policy: 拒绝执行

$ python tools/policy.py ... -Allow main-delete            # 没有注记 = 不是审批
policy: DENY  deletion: -Allow main-delete without --approval-note is not an approval -- state 谁批的批什么

$ python tools/policy.py ... -Allow main-delete --approval-note "owner approved removing legacy module"
policy: OK    allowed by -Allow main-delete (per-invocation grant, approval note: owner approved removing legacy module): 'rm' deletes content
```

```text
$ python tools/constraints.py -Mode check -Root <main-root>    # diff 整删已跟踪文件
constraints: BLOCK [main-tree-raw-delete] src/old.py (1 line(s)): (file deleted)
                      主库整删 = 主线状态被人改走，要人批：-Allow main-tree-raw-delete -ApprovalNote "<谁批的批什么>" 是唯一放行口，回收站有账不抵扣（分支 worktree 上这条不响，走 no-raw-delete）
constraints: 拒绝 -- 1 条禁止操作（warn 0 条）
```

**回收站有账不抵扣**：bin 的 entry 保存的是内容，不决定主线状态——所以主库整删没有
companion 可以满足它。分支树维持回收站要求即可（同一命令在 worktree 检出只报
`no-raw-delete` 的替代路径；回收站清单有这条路径的 entry 才放行——协议一）。

污染 WARN 族四条——**WARN 不拦**，只打一行后果，而且**没有 `-Allow` 豁免**（"能豁免的警告
就是套着友好名字的拒绝"）：

| WARN | 触发 | 后果一句话 |
|------|------|-----------|
| `no-verify` | `git commit --no-verify` | 绕过的门原本会拦的东西进了树，回头以更高的价冒出来 |
| `add-all` | `git add -A` 全树暂存 | 无关改动混进同一个提交，回滚粒度毁掉 |
| `wide-diff` | 提交 diff 超 30 文件/800 行 | 巨型混合 diff 让 review 与二分同时失效 |
| `secret-path` | 暂存了密钥形态文件名（.env、*.pem、id_rsa…） | 凭据进公开树就是 L93 级事故 |

诚实说明它看得见什么：wide-diff 的数字是**检查时刻**的 diff（vs HEAD），不是这条命令将产生的
diff；secret 扫描读**文件名形态**，不读内容。

`tools/pitfalls.py` 是**痛点语料**：真实运维坑一行一条 JSON（`{id, cat, trigger,
consequence, prevention, source, ts}`），五域 `git|github|infra|api|semantics`，存
`<Root>/.agent/pitfalls.jsonl` **随仓提交**——给未来编程模型在再犯之前学到什么真的会出问题。
`seed` 一次性注入 10 条创始条目（蒸馏自 docs/ERRORS.md L15/L63/L93/L95–L100 加两条协作规则），
按 id 幂等（seed ids 是承重的，改名会重复导入分裂语料）：

```text
$ python tools/pitfalls.py seed -Root <root>
pitfalls: seed added pf-git-001
...
pitfalls: seed done (added=10 kept=0 total=10)

$ python tools/pitfalls.py count -Root <root>
pitfalls: count=10 git=3 github=2 infra=2 api=1 semantics=2

$ python tools/pitfalls.py add --cat infra --trigger "fixture at <家目录形态的本机绝对路径> breaks on other machines" ...
pitfalls: refuse -- trigger carries a machine user path (privacy gate, L93); the corpus is committed to the public tree -- rewrite it machine-free
```

**边界原文**：`a corpus informs, it does not enforce`——语料不拒危险 diff，那是
policy/constraints 的活；这是**下一个模型读的东西**。条目内容只与其 source 行一样真；
并发写最后写赢——这是语料，不是台账。

## 给模型的行为规则（写给读这份手册的智能体）

以上协议管的是动作怎么安全；这一节管的是行为怎么不歪。四条，都是硬规则：

1. **方案可以反驳，但必须按用户要求的做。** 用户意图不许降权：不同意，用嘴说——提出更好的
   方案、摆出证据，让用户改主意；执行，用手做——按用户的要求把事做完。把"我不同意"当成
   "我不做"或"我做个打折版"，是把用户意图当可选项。反驳记录在案、执行不打折扣，两件事同时成立。
2. **知道自己不行的长程任务就只做一个任务，不贪多。** 预算不够、能力不够、上下文装不下——
   心里有数就按协议三把一个任务做完挂好账，而不是同时开三个任务全部 censored。
   多任务并行赌的是"半途都不会断"，赌输的代价是三份半成品、零份可续跑。
3. **能后台并行的活并行跑。** 互不依赖的构建、测试矩阵、扫描——能放后台就放后台，
   串行等待是把墙钟预算白白送人。需要编判断的活才排队；其余的让它们同时跑。
4. **基准协议：对比取最强基准，弱基准/只报正结果的读数不入册。** 这是 `docs/specs/DOCUMENTATION.md`
   §5 三条硬规则的精神，也是 AGENTS.md 工程口径的原文：弱基线 + 只报正结果会把结论放大一个量级
   （公开系统综述量到 79% 的论文用弱基线、94.8% 只报正面结果）。拿"我赢过的那个基准"当
   默认对比面，等于自己给自己发奖；负结果照旧必须留档。

## 防什么 / 不防什么（诚实边界总表）

| 工具 | 防 | 不防（诚实边界） |
|------|----|------------------|
| policy | 事前拦：DELETION 家族、git 破坏面、越界写、未授权网络；主库删除要人批（`.git` 目录=主检出，审批注记随放行落账） | 不是内核沙箱；拦不到工具**内部**的 unlink（由 constraints/journal 补）；`-Allow` 逐次放行之后靠 journal 取证；`--separate-git-dir` 主检出被误判 worktree（如实写明的盲区） |
| trash | 裸删不可逆：内容与理由留账 | 同机 rename ledger，**不是备份**；能写工作区的人也能清空回收站 |
| git_safe | 不可逆 git 操作之前有快照、出事有 rescue | refs 只在本 clone 的 `.git`，gc/删 ref 会丢；ignored 文件不进快照；恢复命令人工执行 |
| constraints | diff 里的禁止操作（整删 `/dev/null` 盲区已修）；主检出整删已跟踪文件要人批（`main-tree-raw-delete`） | 只看 diff 与给定命令文本；规则没写的问题不响 |
| resume | 断粮/中断丢上下文：有账、有续跑块 | `budget` 是声明制，测不了模型侧真实 token；账本不替你记计划本身 |
| run_caps | 已放行命令的爆炸半径（内存/进程数/超时） | **不是安全边界**：不隔离文件系统/网络，防不了蓄意逃脱；POSIX 无 procs 帽等价物（如实说明） |
| agent-verify | 调用者跳步：单一入口、闸门串联、回执可审计；run 段实际消耗自动上账（台账 + `cost:` 行）；同 root 并发链 fail-closed；journal-begin 判红不放 run；多智能体协调门（`--claim-gate` / `--handoff`）opt-in 且 false/unknown 都拒绝 | 回执写完之后的事它管不着；不传 `--budget-token` 就没有预算闸；tokens 是声明不是测量；compact 砍掉的是自由文本证据（审计要全量回执）；并行门只覆盖三个只读分类器，写面永远串行；认领账本是协调文件不是 ACL（能写它的人也能改它）；handoff 的 state_hash 只覆盖 `-Root` 下 git 看得见的东西，树外写入不在范围 |
| model-router | 子智能体按角色选模型：只探测可用性、不发网络请求；配置拒绝密钥字面量；选不到可用模型 exit 2 | 不是质量评测器（可用 ≠ 聪明）；`call` 会真实计费；`command` provider 继承环境，不适合不可信提示词；没有 keychain/secret manager |
| subagent-dispatch | 批量任务按 role 分模型、真并行；id/out 唯一、out 在 root 内、已存在默认拒绝；硬链接/符号链接/junction、大小写/尾点 denied component 全部拒绝；`budget_tokens<=0` 调用前拒绝，无 usage 的预算 fail-closed；默认 `--guard enforce` 在调用 provider 前隔离 `context_guard` 判 `quarantine` 的 prompt，并把高风险管理产物移到 `.quarantine`（原路径留空） | 不验证产出质量；预算 prompt 侧不可测，usage 只能事后核对；文字扫描是启发式，不能保证模型不被语义改写绕过；不自动套 `sandbox_run`，也不替代 `agent_verify`；二进制产物只抽可打印段与有限 zip/tar 文本成员，不做 OCR、不递归嵌套归档 |
| context-guard | 外部文字入口卫生：剥离隐形/控制字符、HTML 实体/percent/`\u`/`\x` 多层解码、NFKD+组合标记清除、同形字折叠后扫描，并解 URL-safe/换行/嵌套四层/任意位置 base64 与分隔 hex/ROT13；带 nonce 的不可信数据封套；`quarantine` 默认 exit 1，`wrap` 明确标记隔离 | **不是模型对齐/训练去毒**；静态规则会被翻译、开放式语义改写绕过，base64 仍受 64 MiB 总预算限制（adaptive attack 可绕多数防御）；`clean` 只表示规则集没发现；不自动获得工具动作权限控制 |
| sandbox-run | 子智能体命令的 workspace 边界：cwd 在宿主外、只复制声明输入（拒绝 reparse point/硬链接/大小写尾点）、policy 先拒、run_caps 兜底、宿主指纹事后判 `host-mutated`（exit 3，含 `.agent/.git/.qoder`、git index 语义哈希、构建缓存 name/size/mtime；64 MiB 总预算内大文件也内容哈希）；`-Isolation wsl` 走 WSL+bwrap 真内核命名空间（user/pid/net/ipc/uts/cgroup 全切、空根只 bind `/work`、`--cap-drop ALL`） | **默认模式不是内核隔离**：绝对路径/网络 socket/子进程/ctypes 可逃出；要真隔离必须显式 `-Isolation wsl`，且它只接受 Linux 命令、拒绝 Windows `.exe`/绝对路径参数；宿主写完改回 size/mtime 在构建缓存和超 64 MiB 预算时仍可逃；`-AllowNetwork` 在 WSL 模式不可用；POSIX 无 procs 帽等价物 |
| signal | （观测面）分诊与惩罚信号记账，给 harness 与未来模型训练用 | 规则分诊不是模型判断，查全率/误报率未知；**改不了模型行为**——它只改变 harness 下一步做什么；`--with-text` 语料树内拒收，但树外仍靠人自律 |
| declutter | 注释债只紧不松（budget 棘轮）；clean 的每个原件先进回收站 | 字符串里的注释形态不保证识别；unterminated string 时宁可不动；audit/budget/export 本身不改正本 |
| watch | （观测面）失败率按轮位分桶、整盘健康四报警、污染模式点名 | 行为证据推断，不是读心；**什么也不拦**——exit 1 只表示"有东西要看"；事件标签自由，认不出的标签进不了分母 |
| pitfalls | 痛点语料随仓走，给下一个模型读；机器用户路径进门就拒 | 语料只启不发（informs, does not enforce）——拒危险 diff 是 policy/constraints 的活；条目内容只与其 source 行一样真 |

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
python tools/agent_verify.py -Root . -Changed <改动文件> -Lang python \
  --compact -Receipt .agent/last-receipt.json        # 单行机读回执（子代理协作）
python tools/agent_verify.py -Root . -Changed <改动文件> -Lang python \
  --parallel-gates -Receipt .agent/last-receipt.json # 三个只读门并发（写面仍串行）
python tools/agent_verify.py -Root . -Changed <改动文件> -Lang python \
  --spent-token <本次实测 token> --agent-id <id> -Receipt .agent/last-receipt.json
python tools/agent_verify.py -Root . -Changed <改动文件> -Lang python \
  --claim-gate --claim-who <宿主:会话id> --claims-ledger <CLAIMS.md> \
  -Receipt .agent/last-receipt.json                  # 多智能体：没认领的路径不许跑
python tools/handoff.py create -Root . --id <交接id> --from <谁> --to <谁> \
  --goal "<目标>" --next-action "<下一步>"             # 交接包（默认落 .agent/handoff/）
python tools/handoff.py gate -Root . -Packet <包.json> --json  # True/Unknown/False 三态
python tools/agent_verify.py -Root . -Changed <改动文件> -Lang python \
  --handoff <包.json> -Receipt .agent/last-receipt.json  # 状态对不上就拒绝接活
python tools/bench_orchestration.py                  # 护栏开销/回执体积/并行加速/崩溃恢复
python tools/model_router.py route --role docs --json  # 按角色选模型（不发网络请求）
python tools/model_router.py call --role docs --prompt-file <任务.md> \
  --out <草稿.md> --receipt .agent/model-receipt-docs.json
python tools/subagent_dispatch.py --plan <tasks.json> --parallel 3 \
  --receipt .agent/subagent-dispatch.json               # 按 role 分模型；默认 --guard enforce
python tools/context_guard.py scan --file <外部文本> --json \
  --receipt .agent/context-guard.json                   # high → exit 1；不写原文件
python tools/context_guard.py wrap --file <外部文本> --out <封套.md> \
  --label <来源>                                        # 不可信数据封套 + nonce
python tools/sandbox_run.py -Root . -Copy <相对输入> -- <命令>  # cwd 在宿主外；宿主变了 exit 3
python tools/sandbox_matrix.py --cases 40 --parallel 8 --json   # 40 个独立沙盒并发对抗

# 崩溃恢复（锁 + 快照都在时）
python tools/journal.py status -Root .               # 看快照还在不在
python tools/journal.py restore -Root .              # 把树放回快照
rm .journal/chain.lock                               # 确认死进程后清锁重跑

# E. 单条命令套资源帽
python tools/run_caps.py --mem-mb 2048 --procs 64 --seconds 1800 -- <命令>

# F. 分诊与研究语料（观测面，不拦命令）
python tools/signal.py classify --text "<待分诊文本>" --root .
python tools/signal.py penalty --score <1..3> --reason "<为什么扣>" --root .
python tools/signal.py brief --root .                      # 下一轮注入的一行摘要
python tools/signal.py export --jsonl <树外路径> --root .   # 默认脱敏；--with-text 树内拒收

# G. 注释预算（少写注释，说明进文档）
python tools/declutter.py audit --root .
python tools/declutter.py budget init --root .              # 只紧不松棘轮
python tools/declutter.py budget check --root .
python tools/declutter.py export --strip --out <树外目录> --root .
python tools/declutter.py clean --in-place --root .         # 破坏档：原件先自动进回收站

# H. 观察与衰减（观测面，不拦命令）
python tools/watch.py record <事件> -Turn <轮位> -Ok -Note "<备注>" -Root .
python tools/watch.py report -Root .                        # 疲劳面：后期失败率上行 → WARN
python tools/watch.py health -Root .                        # 整盘健康四报警面
python tools/watch.py pollution -Root .                     # 污染模式点名

# I. 痛点语料（五域：git|github|infra|api|semantics）
python tools/pitfalls.py seed -Root .                       # 幂等，按 id 去重
python tools/pitfalls.py add --cat <域> --trigger "<什么会踩>" --consequence "<踩了会怎样>" --prevention "<怎么防>" -Root .
python tools/pitfalls.py list [--cat <域>] -Root .
python tools/pitfalls.py count -Root .
python tools/pitfalls.py export --jsonl <ML训练就绪路径> -Root .
```

## 验收口径

改完任何护栏相关的东西，验收是两条命令，**双绿才算过**（术语漂移是门禁级，不是文档装饰）：

```bash
pwsh tools/selftest.ps1
python tools/terminology.py check   # 术语契约：锚点漂移/混用/孤儿 三类漂移机器可查
```

结尾行必须长这样（实测）：

```text
selftest: passed=667 failed=0 skipped=0
```

```text
terms: files=27 terms=17 anchors=35 drift=0
terms: clean -- authorities still say what the registry cites
```

**必须 675 条全绿 + terminology check 干净。**（编排链成熟轮 +37：AF 组——并发锁/崩溃恢复/畸形存储/超大计划/
compact 一致性/并行门一致性；此前 affected 的 rust 减集下沉到 cargo target 级时 +16：
B 组新增 B5 与 B(续) B6–B19，组数不变；cognition/collect 落地时 +32：AE 组；防偷换概念 +26：AH 组；
多智能体协调门 +8：AF23/AF24 认领/交接，AF 组从 38 到 46；子智能体执行面 +5：AN1/AN2/AN3、AO1、
AP1–AP3） 36 组：
A–Z 26 组 + AA/AB/AC/AD/AE/AF/AH/AN/AO/AP 十组（W 组 trash 19 条、X 组 git_safe、
Y 组 resume 32 条、Z 组 run_caps；AA 组 signal 35 条、AB 组 declutter 21 条、AC 组 watch 44 条、
AD 组主库删除审批 / 污染 WARN 族 / pitfalls 24 条、AE 组 cognition 32 条、
AF 组编排链稳定性 46 条、AH 组术语契约 26 条、AN 组 3 条（模型路由 16/16、沙盒 37/37、
40 沙盒矩阵 40/40）、AO 组 1 条（派发器 25/25）、AP 组 4 条（文字门 59/59、
90 例 stress 90/90、malicious CLI 隔离/良性不误杀、81 例红队 0 攻击漏检），是新增的十个观测、审批、稳定性、
概念契约与入口污染面）。护栏的断言全部
遵循"先看着它响，才算数"：每条拒绝路径都有对应的判红用例（W13/W14 journal restore 走
回收站、W20/W21 no-raw-delete、Z1/Z4/Z5 run_caps 内核帽、I2b 编排顺序、M9 bash/PS 台账
互通、AA12/AA14 signal 默认路径拒绝崩溃、AB1 audit 行计数精确数、AC4/AC5/AC6 疲劳 WARN
双桶阈值、AD3/AD9 主库删除无注记必红、AD11 回收站有账不抵扣、AF1 并发链恰好一胜一拒、
AF2c 崩溃脏状态必诊断、AF4c journal-begin 判红不放 run、AF5b 坏账预算闸 fail-closed、
AF7/AF8 五百条计划顺序保持、AF10 compact/full 关键字段逐项相等、AF19 并行门 verdict
逐项相等；AH5 混用必中、AH6 对照行不误报、AH7/AH8 改坏锚点必报、AH10 棘轮缺 --reason 必拒）。
本轮真实台账在
`docs/ERRORS.md` L95–L108、L114 与 L117：
L95 journal restore 裸删改走回收站；L96 整删盲区与 no-raw-delete 内核规则；
L97 run_caps 两个内核常量凭记忆写错；L98 selftest 断言与编排层漂移；L99 夹具写死本机路径；
L100 中文控制台编码伪红；L101 signal 默认路径读未初始化键直接崩溃（AA12/AA14）；
L102/L105/L108 worktree 检出的 `.git` 是**文件**形态，isdir 探测三次踩同一坑——判据单一
归宿是 policy 的 `checkout_kind`（AC21/N1/N2/AD0）；L103 declutter 模板字符串插值吞真
注释（AB10）；L104 audit 行计数按段标行错（AB1）；L106 隐私夹具输入拆形，坏输入原文不落
selftest.ps1（AC3/AC3b）；L107 `Get-Item` 看不见 Hidden 的 `.git` 文件，形态判定走 .NET
存在性检查（W18f/AD0）；L114 journal-end 把崩溃记成 "changed"、journal-begin 失败照旧
放 run——退出码三分支显式化 + begin 升级为闸（AF4/AF4b/AF4c）；L117 概念漂移没有机器面，
术语注册表 + 三类漂移检查落地（C046/AH 组）。

### 回执怎么读

agent-verify 的回执（`-Receipt` 指定的 JSON，或 stdout 尾部）按三处读：

1. **`verdict`**——只有这几种，各是什么一眼定论：

| verdict | 含义 | exit |
|---------|------|------|
| `dry-run` | 排练通过，还没真跑 | 0 |
| `passed on proven subset` | 在带证明的子集上通过 | 0 |
| `passed on full run (plan was unproven)` | 没有证明，跑的是全量——通过了也算全量成绩 | 0 |
| `refused by preflight / checks / policy / constraints` | 对应闸拒绝，run 未开始 | 1 |
| `suspended (budget) before run` | 声明额度（扣减台账已记后）≤0，run 前挂起（censored，不是失败） | 1 |
| `command failed` | 命令真跑了且失败（有 `--task-id` 时已自动挂起） | 1 |
| `refused by chain lock` | 同 root 已有另一条链（或上次崩溃留下的锁）——fail-closed 不竞争；诊断带恢复命令 | 1 |
| `refused by budget ledger` | 预算台账损坏，剩余预算无法核算——不降级成弱闸 | 1 |
| `refused by journal` | journal 快照建不起来——没有证据链的 run 不发生 | 1 |
| `refused by claim gate` | 改动路径没有被 `--claim-who` 的在效认领完整覆盖（false=别人占着，unknown=没登记） | 1 |
| `refused by handoff` | 交接包与本树对不上（Unknown=过期/挪树/状态不可证；False=包结构不合法） | 1 |
| `refused by plan` | 注入的 plan 文件无法解析（坏 JSON 不许静默变成空计划） | 1 |

2. **`steps[]`**——每步 `{step, verdict, exit, seconds, output(尾部≤6行)}`；
   跳过的步骤以 `skipped` 留痕（跳过也算一步，不允许"跳了就消失"）。
   journal-end 的 verdict 是三分支：`clean`（0）/ `changed`（1）/ `failed`（其余）——
   工具崩溃不再被记成"改了东西"（L114）。

3. **`run[]` / `changes[]`**——每条命令的 exit 与耗时；journal-end 报告实际落盘的
   `added/modified/deleted`，这就是"改了什么"的证据链。

4. **`cost`（追加字段）+ stdout 尾部的 `cost:` 行**——`wall_s`/`overhead_s`/`tokens`/
   `recorded`/`scope` + 诚实 note；对应台账 `<Store>/cost-ledger.json` 的本轮 entry。

5. **`--compact` 单行回执**（opt-in，全量仍是默认与审计面）——保留路由字段
   （verdict/exit/proven/wall_s）、每步 `{step,verdict,exit,seconds}`、每条 run 命令
   `{command,exit,seconds}`、budget 数字、cost、agent_id、changes 计数；砍掉自由文本
   证据。实测见 `python tools/bench_orchestration.py`。
