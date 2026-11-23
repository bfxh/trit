# 智能体对齐门（目标 / 需求 / 产物 / 证据 / 变更）

## 为什么加这一层

现有智能体最常报的是“做完了”，最少报的是“做完的东西还是不是原来那个东西”。
完成度检查只能证明某个命令返回 0、某个文件存在（`tools/alignment_gate.py` 把目标、需求、
产物、证据、变更这条链单独建模）；它不能证明：

- 这条需求仍然挂在最初的目标上；
- 这个产物实现的确实是这条需求，而不是顺手加的另一件事；
- 这条证据能定位到产物/测试里的具体位置；
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
| `goals`（v2） | 至少一个；恰好一个 `parent=null` 的根；子目标父链必须落到根，不能断/环 |
| `requirements` | 至少一条；`text` 非空；`priority` 为 `must/should/may`，缺省按 `must` |
| `requirements.parent` | 必须存在，且父链最终落到根 `goal`；不能成环 |
| `requirements.artifacts/evidence` | 必须引用已存在的 id；`must` 至少各一条有效引用 |
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
3. `must` 需求是否同时有有效产物与有效证据；`strict` 下 `should` 也一样；
4. artifact/evidence 的 id 是否存在、文件是否存在、sha256、locator 是否命中；
5. 每条变更是否被某条需求的 `changes` 或某个 artifact 路径覆盖；
6. 变更是否落进 `non_goals`；
7. 需求之间是否声明了矛盾；
8. 所有路径是否在 root 内（含 realpath 检查）。

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

## 测试读数

- `python tools/alignment_gate.py self-test`：**65/65**
- `python tools/alignment_gate.py stress --cases 100`：**200 用例，valid 100 aligned，mutated 100 rejected，0 errors**
- `python tools/agent_settings.py --self-test`：**30/30**
- `tools/selftest.ps1` 的 AL27–AL32：通过清单继续 run、缺证据拒绝、清单缺失 unknown、lenient 只警告、不传 `--alignment` 时自动发现、v2 子目标链通过并记录 `goals=2`
- 全仓 `tools/selftest.ps1`：**702 passed / 0 failed / 0 skipped**

## 诚实边界

- 本门验证的是**声明关系与磁盘事实**，不判断需求文本的语义真假；需求写得对
  不对，仍要靠人/评审。
- `locator` 只证明片段存在，不证明该片段就是充分证据；它把“有证据”降级为
  “可定位证据”，不替代 claim audit 与测试。
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
