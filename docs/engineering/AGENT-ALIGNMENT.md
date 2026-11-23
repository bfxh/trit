# 智能体对齐门（目标 / 需求 / 产物 / 证据 / 变更）

## 为什么加这一层

现有智能体最常报的是“做完了”，最少报的是“做完的东西还是不是原来那个东西”。
完成度检查只能证明某个命令返回 0、某个文件存在（`tools/alignment_gate.py` 把目标、需求、
产物、证据、变更这条链单独建模）；它不能证明：

- 这条需求仍然挂在最初的目标上；
- 这个产物实现的确实是这条需求，而不是顺手加的另一件事；
- 这条证据能定位到产物/测试里的具体位置；
- 这条证据宣称的测试命令是否真的在本次 run 里成功跑过；
- 这次改动的每个文件都能被某条需求解释；
- 改动没有落进明确划出的 non-goal 范围；
- 两条需求没有互相矛盾。

这类失败不是“代码写错”，而是意图/产物/证据之间的对齐链断了。RTADev 在
多智能体软件流程里把这一步做成实时的 alignment checking 与共享认证仓库；
TraceDev 用异构 traceability graph 维护需求、设计、代码之间的一致性；
执行溯源研究则把 support / derive / depend-on / contradict 等关系当成一等
公民。本仓的 `alignment_gate.py` 是这套思想的一个**静态、可离线复算**的
最小实现：先把关系写成清单，再在写盘前逐条拒绝断链。

## 清单格式 `alignment/1`

```json
{
  "schema": "alignment/1",
  "goal": {"id": "G1", "text": "把输入整理成可验收的产物"},
  "requirements": [
    {
      "id": "R1",
      "text": "产物包含目标字段",
      "priority": "must",
      "parent": "G1",
      "artifacts": ["A1"],
      "evidence": ["E1"],
      "changes": ["src/a.py", "docs/a.md"],
      "contradicts": []
    }
  ],
  "artifacts": [
    {"id": "A1", "path": "src/a.py", "locator": "L10-L40", "sha256": "<可选>"}
  ],
  "evidence": [
    {"id": "E1", "path": "tests/test_a.py", "locator": "text:test_a"}
  ],
  "non_goals": [
    {"id": "N1", "text": "不碰 vendor 目录", "paths": ["vendor/**"]}
  ],
  "allow_unlinked": ["Cargo.lock"]
}
```

`parent` 是“这条需求对齐到哪个上层目标/需求”；`artifacts` 是“这条需求由
哪些产物实现”；`evidence` 是“哪些证据能证明它”；`changes` 是“本次允许
改哪些路径”。三者都是显式边，不写就是断链。

### 多目标/子目标 `alignment/2`

单个目标容易把多个诉求压成一句话，最后“都完成了”但没人知道各自对应哪一条。
`alignment/2` 用一棵**单根目标树**保留层级；旧 `alignment/1` 继续兼容：

```json
{
  "schema": "alignment/2",
  "goals": [
    {"id": "G1", "text": "把输入整理成可验收的产物"},
    {"id": "G2", "text": "补齐验收证据", "parent": "G1"}
  ],
  "requirements": [
    {"id": "R1", "text": "产物包含目标字段", "priority": "must", "parent": "G2",
     "artifacts": ["A1"], "evidence": ["E1"], "changes": ["src/a.py"]}
  ],
  "artifacts": [{"id": "A1", "path": "src/a.py"}],
  "evidence": [{"id": "E1", "path": "tests/test_a.py", "locator": "text:test_a"}]
}
```

规则：`goals` 至少一个；`parent` 缺省/`null` 表示根；**必须有且只有一个根**；
每个子目标的父链必须落到这个根，不能断开或成环。需求可以挂到任意子目标，
但需求的父链最终仍必须落到根。`alignment/1` 与 `alignment/2` 不能混用
`goal`/`goals` 字段。

### 证据绑定实际 run：`alignment/3`

`alignment/1`、`alignment/2` 的证据只证明“文件在、locator 命中”，一个陈旧的
测试文件也能当证据。`alignment/3` 把证据再绑到**本次实际成功的 run 命令**：

```json
{
  "schema": "alignment/3",
  "goals": [{"id": "G1", "text": "把输入整理成可验收的产物", "parent": null}],
  "requirements": [
    {"id": "R1", "text": "产物包含目标字段", "priority": "must", "parent": "G1",
     "artifacts": ["A1"], "evidence": ["E1"], "changes": ["src/a.py"]}
  ],
  "artifacts": [{"id": "A1", "path": "src/a.py"}],
  "evidence": [
    {"id": "E1", "path": "tests/test_a.py", "locator": "text:test_a",
     "kind": "run", "command": "python -m pytest tests/test_a.py"}
  ]
}
```

规则：

- `goals` 形状与 `alignment/2` 相同，仍是一棵单根目标树；
- v3 的 evidence 额外接受 `kind`（`file`/`run`）与 `command`；`kind=run` 必须
  声明非空 `command`，且 command 必须点名证据文件路径；
- v3 的 `must` 需求至少要有**一条** `kind=run` 证据；`strict` 下 `should` 同，
  `--lenient` 只把 should 的缺口降为 warning；`may` 只警告；
- 预检验证的是**声明**：命令形状、路径、哈希、locator 都成立，回执记
  `counts.provenance=1`、`counts.provenance_verified=0`；
- `agent_verify` 在 post-run 把本次 `run[]` 中 exit=0 的命令传给 gate，命令
  必须按空白/路径分隔符/大小写归一后**精确匹配**，否则
  `evidence-not-executed`，post 为 `unaligned`。此时回执的
  `provenance_verified` 才变成 1；
- `alignment_gate check --run-command "..."` 可单独复算同一条件；不传
  `--run-command` 只得到 declared 证据，不冒充已执行。

命令匹配只能证明“这条命令真的跑过且成功”，不能证明它真的在测这条需求；
`true # tests/test_a.py` 这种命令仍可能钻空子。语义充分性仍要靠人/测试设计，
这是本门明确的诚实边界。

### 生成骨架：`scaffold`

`scaffold` 从目标与需求文本生成一份**故意不完整**的 `alignment/2` 清单：
`artifacts`、`evidence`、`changes` 留空，所以 gate 仍会拒绝。它只帮人把
“目标树/需求清单”先摆出来，不代替补证据：

```bash
python tools/alignment_gate.py scaffold --root . --manifest .agent/alignment.json \
  --goal "把输入整理成可验收的产物" \
  --requirement "产物包含目标字段" --requirement "补齐验收证据"
```

已存在清单时默认不覆盖；需要重写才加 `--force`。输出路径必须在项目 root 内。

### 字段约束

| 字段 | 约束 |
|---|---|
| `goal`（v1） | 恰好一个；`id`/`text` 非空 |
| `goals`（v2/v3） | 至少一个；恰好一个 `parent=null` 的根；子目标父链必须落到根，不能断/环 |
| `requirements` | 至少一条；`text` 非空；`priority` 为 `must/should/may`，缺省按 `must` |
| `requirements.parent` | 必须存在，且父链最终落到根 `goal`；不能成环 |
| `requirements.artifacts/evidence` | 必须引用已存在的 id；`must` 至少各一条有效引用 |
| `evidence.kind`（v3） | `file` 或 `run`；`run` 额外要求 `command`，且 command 必须点名证据路径 |
| `evidence.command`（v3） | `run` 的声明命令；post-run 必须在本次 exit=0 的 run 命令里精确匹配 |
| `artifacts/evidence.path` | 仓库相对路径；不能绝对、`..` 逃逸或经符号链接逃出 root |
| `sha256` | 可选；64 位十六进制，必须与磁盘内容一致 |
| `locator` | 可选；`Lx`、`Lx-Ly` 或 `text:<片段>`，必须命中 |
| `non_goals.paths` | 仓库相对 glob；命中本次变更即拒绝 |
| `allow_unlinked` | 允许不挂需求的变更 glob（锁文件等），仍需仓库相对 |
| `manifest` 自身 | 必须留在项目 root 内；`..`、盘符绝对路径、经符号链接逃出 root 的清单按 unknown 拒绝 |
| `contradicts` | 引用的需求 id 必须存在；出现即成对拒绝 |

## 检查与退出码

`alignment_gate.py` 检查：

1. schema、字段类型、id 唯一性、未知字段；
2. 每条需求的 parent 链是否落到唯一根 goal；v2 还检查每个子目标是否落到同一个根，是否有环/断链/多个根；
3. `must` 需求是否同时有有效产物与有效证据；`strict` 下 `should` 也一样；v3 还要求一条 run-bound 证据；
4. artifact/evidence 的 id 是否存在、文件是否存在、sha256、locator 是否命中；
5. v3 的 run 证据 command 是否点名证据路径；post-run 是否在本次成功 run 命令里精确出现；
6. 每条变更是否被某条需求的 `changes` 或某个 artifact 路径覆盖；
7. 变更是否落进 `non_goals`；
8. 需求之间是否声明了矛盾；
9. 所有路径是否在 root 内（含 realpath 检查）。

退出码：

- `0` aligned：无失败；
- `1` unaligned：有断链/矛盾/未对齐变更；
- `2` 坏调用；
- `3` unknown：清单缺失、坏 JSON、schema 不对。**unknown 不是通行证**。

## 接入 `agent_verify`

- 项目存在 `.agent/alignment.json` 且 `settings.alignment.enabled=true` 时自动运行；没有清单的项目不会被凭空判定为 aligned，也不会被自动拒绝。CI 要强制时显式传 `--alignment`。
- 清单是**任务级工作状态**：任务结束后应更新或删除；把过期清单长期提交在仓库里，会让后续所有 run 被旧 `changes`/旧证据挡住。
- `--alignment <manifest>` 显式覆盖路径；文件缺失会以 unknown 拒绝整链。
- `--alignment-lenient` 只把 `should` 的缺口降为 warning；`must` 缺口永远拒绝。
- 门在 `constraints` 之后、`claims`/`journal-begin` 之前执行；拒绝时不会写盘。
- 回执增加 `alignment` 块：manifest、status、score、must_score、counts、failures、warnings。
- 设置里 `alignment.enabled=false` 可显式退出；不会静默跳过已有清单。

### run 后复核 `alignment-post`

预检只证明“动手前的声明成立”；执行器仍可能在 run 里写出清单没声明的文件，或改掉
带 `sha256` 的产物、让 evidence 的 locator 失效。`agent_verify` 在 `journal-end`
拿到**实际落盘路径**后，再用同一份清单和同一套 gate 复核：

- `journal-end` 的 `added/deleted/modified` 路径作为 `--changed` 重跑 gate；
- 产物/证据重新按磁盘内容验哈希与 locator；
- v3 的 `kind=run` 证据命令必须出现在本次 exit=0 的 `run[]` 里，否则
  `evidence-not-executed`；只声明“会跑”不算跑过；
- 实际变更必须挂到 requirement 或 artifact；落进 `non_goal` 仍拒绝；
- 清单被 run 改坏、journal 失败导致无法证明实际落盘，一律按 drift/unknown 处理；
- 失败 verdict 是 `unaligned after run`，exit 1；journal 已保留，成本/记忆仍会记账，
  不会用“拒绝”抹掉已经发生的写入；
- 若 `Store` 显式放在项目 root 内，thin bookkeeping 的 store 路径会从“产品变更”
  中过滤，避免把 journal 自己的副本当成业务产物。

回执里的 `alignment.post` 记录 post-run 的 `status/score/must_score/counts/failures/warnings`。
它仍在同一个 `alignment` 稳定字段内，不新增顶层波动键。

## 测试读数

- `python tools/alignment_gate.py self-test`：**79/79**
- `python tools/alignment_gate.py stress --cases 100`：**200 用例，valid 100 aligned，mutated 100 rejected，0 errors**
- `python tools/agent_settings.py --self-test`：**30/30**
- `tools/selftest.ps1` 的 AL27–AL37：预检五种路径（通过、缺证据、清单缺失、lenient、自动发现）、v2 子目标链、run 后写出未挂变更拒绝、声明内变更仍 aligned、run 改带 sha256 产物后哈希漂移拒绝、v3 证据命令实际跑过才升级 provenance、命令没跑则 post 拒绝
- 全仓 `tools/selftest.ps1`：**707 passed / 0 failed / 0 skipped**

## 诚实边界

- 本门验证的是**声明关系与磁盘事实**，不判断需求文本的语义真假；需求写得对
  不对，仍要靠人/评审。
- `locator` 只证明片段存在，不证明该片段就是充分证据；它把“有证据”降级为
  “可定位证据”，不替代 claim audit 与测试。
- v3 的 command 匹配只证明“这条命令真的跑过且成功”，不证明它真的在测这条需求；
  `true # tests/test_a.py` 仍可钻空子。语义充分性要靠人/测试设计。
- v3 预检的 `alignment.status=aligned` 是**声明对齐**；`provenance_verified` 只有
  post-run 或显式 `--run-command` 才非零，不要把两者混读。
- `alignment-post` 只看 journal 覆盖范围内的实际差异；journal 明确跳过的目录
  （`target`/`.git`/`.journal`/`node_modules` 等）不会被它发现，这是 journal 的
  既有作用域，不是对齐门在放水。
- 没有 LLM 评审；`scaffold` 只生成空骨架，不自动补产物/证据/变更，因此骨架本身
  仍会被拒。清单必须由执行者显式写出，写不出就应该拒绝。
- 不替代 `security_gate`、`claim_audit`、`handoff`、`constraints`：它只管
  goal/requirement/artifact/evidence/change 这一条轴。

## 参考

- RTADev: Intention Aligned Multi-Agent Framework for Software Development
  (ACL 2025 Findings), <https://aclanthology.org/2025.findings-acl.80/>
- TraceDev: A Traceability-Driven Multi-agent Framework for Requirement-to-Code
  Development, <https://arxiv.org/pdf/2607.18886>
- From Agent Traces to Trust: A Survey of Evidence Tracing and Execution
  Provenance in LLM Agents, <https://arxiv.org/html/2606.04990>
- Towards Understanding Specification Gaming in Reasoning Models,
  <https://arxiv.org/pdf/2605.02269>
- A Sober Look at Agentic Misalignment in Automated Workflows,
  <https://arxiv.org/html/2605.24197>
- Asymmetric Goal Drift in Coding Agents Under Value Conflict,
  <https://arxiv.org/html/2603.03456v1>
