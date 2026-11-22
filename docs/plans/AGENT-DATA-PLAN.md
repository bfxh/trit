# 智能体数据 × 语言增强：可复算的高质量数据门（2026-11-23）

> 计划与判据，不是结果。定位与 `DATA-CLEANING-PLAN.md` 一致：**用可解释、可复算的规则与三值/三支决策做数据质量，不把唯一的裁判权交给另一个模型**。
> 论文只取能落成判据的部分；引用见 §1。

## 0. 结论（先给判断）

现有开源做法已经证明三件事：

1. **合成/轨迹数据的价值在“生成流程 + 过滤”，不在堆量**。AgentInstruct 用 agentic flows 生成 25M pairs，并明确说合成数据质量与多样性参差、需要大量人工策展；SWE-Gym 的关键是“可执行环境 + 单测 + 轨迹”，不是纯文本模仿。
2. **过程信号比只看最终答案更强**。PRM800K 证明 step-level 监督显著优于 outcome-only；Agent-as-a-Judge 把中间过程纳入评价，和人工基线一样可靠；Agent Q 进一步说明**失败轨迹也能用于学习**（MCTS + DPO），所以“拒绝”不应等于“丢弃”。
3. **模型过滤有效但会过量删除**。DCLM 的模型过滤把 7B/2.6T token 推到 64% MMLU；FineWeb-Edu 用教育性分类器拿到 1.3T token；Nemotron-CC 指出激进过滤会删掉约 90% 数据，改用**分类器集成 + 合成重写 + 少用启发式**，在长 token horizon 上更好。

因此本仓的主线不是“再训一个 judge 给每条数据打分”，而是：

- **硬门可复算**：可执行性、验证通过、证据哈希、预算、循环、去重——全部确定性规则；
- **软门显式不确定**：过程完整性、恢复、语言质量——用三值/三支决策，**不确定就 defer，不硬删**；
- **负样本入册**：reject 的轨迹带原因码保留，服务 DPO/偏好数据；
- **语言增强走重写闭环**：低质但信息量高的样本重写后再过同一门，原始与重写都保留 provenance；
- **数据卡可复现**：清单指纹、来源、许可证、原因分布、去污染结果。

## 1. 论文依据与“采用/不采用”

| 论文 | 关键结论 | 本仓怎么用 |
|---|---|---|
| AgentInstruct (arXiv:2407.03502) | agentic flows 生成 prompts+responses；25M pairs；质量/多样性需策展 | 生成与门控分离；门控输出 accept/reject/defer 与原因分布 |
| Agent-as-a-Judge (arXiv:2410.10934) | 过程级评价远好于 LLM-as-a-Judge；DevAI 55 任务/365 需求 | 每条轨迹记录 step 级证据与过程分；judge 只做对拍，不做唯一裁判 |
| SWE-Gym (arXiv:2412.21139) | 可执行环境 + 单测 + 真实轨迹；resolve rate +19% | 硬门要求 verifier 与 evidence；拒绝“无法执行/无法验证”的轨迹 |
| Agent Q (arXiv:2408.07199) | MCTS + 自 critique + 失败轨迹 DPO；booking 18.6%→81.7% | reject/defer 的轨迹进负样本池，不丢 |
| PRM800K (arXiv:2305.20050) | step-level 监督优于 outcome-only | 软门记录每步 observation/thought/错误恢复 |
| DCLM (arXiv:2406.11794) | 模型过滤是关键；7B/2.6T → 64% MMLU | 分类器可做 M1 软门，但不替代硬门 |
| FineWeb / FineWeb-Edu (arXiv:2406.17557) | 15T token；教育性过滤 1.3T，MMLU/ARC 显著提升 | 语言门的教育性分数作为软门特征，保留原分数与阈值 |
| Nemotron-CC (arXiv:2412.02595) | 激进过滤删 90%；分类器集成 + 合成重写更优 | 三支决策 + 重写闭环；defer 不做硬删除 |
| Tulu 3 (arXiv:2411.15124) | SFT/DPO/RLVR；开放数据配方；去污染 | 数据卡、去污染、可验证奖励；发布配方而不是只发数字 |

不采用：把某个 LLM judge 的分数当唯一标签；用 benchmark 题目参与训练；没有任何 provenance 的“清洗后数据”。

## 2. 统一数据 schema（JSONL，一行一条轨迹）

```json
{
  "schema": 1,
  "task_id": "swe-...",
  "source": "swe-gym",
  "license": "MIT",
  "model": "producer-model@version",
  "harness": "agent-harness@git-sha",
  "steps": [
    {
      "i": 0,
      "role": "assistant",
      "thought": "...",
      "action": {"tool": "bash", "args": {"cmd": "pytest -q"}},
      "observation": "...",
      "error": false
    }
  ],
  "outcome": {
    "verifier": "pytest",
    "passed": true,
    "evidence": ["sha256:..."],
    "final": "..."
  },
  "cost": {"tokens_in": 1200, "tokens_out": 300, "tool_calls": 6, "ms": 4200},
  "labels": ["code", "tool-use"]
}
```

约束：

- `task_id`、`model`、`steps` 必填；`steps[*].action.args` 必须是对象（无参数写 `{}`）。
- `outcome.verifier` 不得为空；`passed` 是布尔；`evidence` 每项是 `sha256:<64 hex>`。
- `source`/`license`/`harness` 缺失时数据可读，但数据卡会把它标成 `provenance_incomplete`（defer，不 reject）。

## 3. 质量门（M0）

### 3.1 硬门（任一失败 → reject，原因可复算）

| 代码 | 判据 |
|---|---|
| `schema` | task_id / model 非空；steps 非空；每步 role 非空 |
| `action_args` | 有 tool 的步必须有 args（对象），tool 名非空 |
| `budget` | steps ≤ 64；tokens_in+tokens_out ≤ 200000；tool_calls ≤ 128 |
| `loop` | 连续 3 步 `(tool, args)` 完全相同 |
| `verified` | 要求验证时：`verifier != "none"` 且 `passed == true` |
| `evidence` | 要求验证时：evidence 非空，且每项匹配 `sha256:[0-9a-f]{64}` |
| `dedup` | 规范化内容指纹与已见集合重复 |

### 3.2 软门（硬门通过后，任一失败 → defer）

| 代码 | 判据 |
|---|---|
| `recovery` | 出现 `error=true` 后，后续没有成功步 |
| `observation` | 工具步缺 observation |
| `thought` | 思考非空率 < 0.5 |
| `length` | 步数 < 2（单步解复杂任务）或 > 32（长尾空转） |
| `language` | 非空白字符比 < 0.6，或 4-gram 重复率 > 0.4，或控制字符比 > 0.01 |
| `unverified_soft` | 未强制验证且 `verifier == "none"` |

三支决策：hard fail → `reject`；否则 soft fail → `defer`；否则 `accept`。reject/defer 都保留轨迹与原因码（负样本池）。

### 3.3 输出

- `gate.report.json`：决策计数、每类原因计数、输入指纹（文件 SHA-256 + 内容级指纹）。
- `accepted.jsonl` / `rejected.jsonl` / `deferred.jsonl`：带 `gate` 字段的原记录。
- `dataset.manifest.json`：`schema/kind/created_unix/commit/input{path,sha256,records}/decisions/reasons/manifest{files,bytes,hash}`。

## 4. 语言增强（M1，重写闭环）

1. 语言门给样本分层：`accept`（直接入池）、`defer`（重写/再采样）、`reject`（只入负样本）。
2. 对 `defer` 中“信息量高但表达差”的样本做重写（WRAP/PRewrite/Magpie 式），**重写版本必须再过同一门**。
3. 保真检查用可复算规则：关键实体、数字、代码块、URL、引用编号必须保留（抽取式比对）；不通过则拒绝重写版本。
4. 原始与重写都保留 `provenance`，数据卡记录重写模型与提示词版本；训练配比按技能标签分层（DoReMi/RegMix 的小规模消融）。

## 5. 验收（机器可判）

- 单测：每条硬门/软门至少一正一反；三支决策覆盖 accept/reject/defer；指纹稳定（同内容不同顺序不影响文件集哈希，序列化顺序固定）。
- 对拍：500 条人工标注样本上，硬门与人工一致率 100%（硬门是定义）；软门与 LLM judge 的 Cohen's κ ≥ 0.6（低于则调阈值，不宣称有效）。
- 训练消融：固定 token/步数预算，raw vs gated vs gated+rewrite；下游 agent 任务与语言基准同时报告，**不许只报提升项**。
- 数据卡：`dataset.manifest.json` 可重建；来源/许可证/去污染（benchmark 题目与答案）逐项列出。

## 6. 实施顺序

- **M0（本轮）**：`crates/trit-data`：schema + 硬门 + 软门 + 三支决策 + 清单；CLI `gate`；单测。
- **M1**：语言门特征（教育性/信息密度/语言纯净度）+ 重写闭环 + judge 对拍脚本。
- **M2**：配比与消融、去污染、数据卡发布；与 `trit-orch` 轨迹（TraceEvent/CAS）对接，直接吃真实运行轨迹。

## 7. 边界与风险

- 不训练“唯一裁判”模型；模型分数只能进软门，且必须记录模型版本与阈值。
- 不把 benchmark 题目/答案混入训练；去污染是硬门的一部分。
- 许可证不明或来源不可追溯的数据只能 `defer`，不能 `accept`。
- 长尾/失败轨迹不丢：reject 是训练信号（Agent Q），不是垃圾。

## 8. 实现进度（2026-11-23）

- **M0 已落地**：`crates/trit-data` 的 schema、7 条硬门、6 条软门、三支决策、数据清单；
  CLI `trit-data gate`。
- **M1 已落地（库 + 对拍）**：
  - `language.rs`：可复算文本特征与 `Good / Uncertain / Bad` 三值分类；
  - `rewrite.rs`：数字/URL/代码/引用锚点提取、保真检查（缺失 + 长度比）、重写后送回同一门；
  - `compare.rs`：多类 Cohen's κ；CLI `trit-data compare --a --b`。
- **M1 未接**：`gate` 主决策仍用内嵌语言规则，尚未把 `LangClass` 接入
  accept/defer 映射；CLI 尚无 `rewrite` 子命令（需要外部 rewriter 命令与超时/失败回退）。
- **下一步**：把 `LangClass` 接进 gate（Uncertain → defer，Bad → 可配置 reject/defer），
  实现 `rewrite --cmd`，并用 500 条人工/LLM judge 标注跑 κ 与硬门一致率。

- **M2 CLI 与数据卡已落地（2026-11-23）**：
  - `trit-data decontaminate --jsonl <in> --bench <bench.jsonl> [--ngram 13] [--threshold 0.2] [--out <dir>]`：
    逐条写 `contamination {ratio, bench_index, contaminated}`，并输出报告；
  - `trit-data mix --jsonl <in> [--total 1000] [--weights code=0.7,writing=0.3] [--out <dir>]`：
    按标签配额输出 `mix.json`；
  - `gate.report.json` 新增 `dataset_card`：sources / licenses / labels 分布、
    `provenance_missing`、`rewritten`。

- **真实轨迹投影已落地（2026-11-23）**：
  - `trit-data project --events <trace.jsonl> --out <dir>`：把 `trit-orch` 的
    TraceEvent 按 `trace_id` 分组、按 `chain_seq` 排序，机械投影成轨迹 JSONL；
    事件 → step（actor/kind/payload），`finish_confirmed` → `verifier=trace / passed`，
    `payload_hash`（空则事件内容 SHA-256）→ evidence；**不补写不存在的 thought**。
  - 真实轨迹没有 thought 字段时，用 `gate --min-thought 0`。
  - 端到端：2 条事件 → 1 条轨迹 → `gate` accept 1；数据卡显示 `trit-orch:trace`。

- **语言维度可选（2026-11-23）**：
  - `trit-data lang --text "..."`（单次）或 `--jsonl <in>`（批量）：输出主语言
    （`zh`/`ja`/`ko`/`ru`/`ar`/`en`/`other`）与各文字系统比例；`--deny zh,ja`、
    `--allow zh,en` 标记 `denied`；`--off` 直接关闭检测。
  - `gate` 新增：`--no-language`（整门关闭）、`--lang-allow zh,en`、
    `--lang-deny zh,ja`、`--no-dedup`、`--no-loop`；数据卡新增 `languages` 分布。

## 9. 风格 / 语义筛选与论文映射（2026-11-23）

- **可组合的标签筛选**（默认关闭）：
  - 风格：`formal` / `conversational` / `technical` / `tutorial` / `narrative` / `qa`；
  - 语义：`code` / `math` / `science` / `medical` / `legal` / `news` / `fiction` / `dialogue`；
  - `--style` / `--style-deny` / `--no-style`，`--semantics` / `--semantics-deny` / `--no-semantics`；
  - `gate`、`decontaminate`、`mix` 共用同一组开关；原因码 `style_filtered` / `semantics_filtered`。

- **论文映射**：

| 来源 | 规则 / 结论 | 本仓映射 |
|---|---|---|
| StarCoder / The Stack | 许可、PII、归因追踪、按语言覆盖 | 代码占比 + 语言识别 + 许可证缺失标记 |
| Stack v2 | 619 语言、SWHID、PR/notebook/文档 | 多语言代码特征表；source/license 数据卡 |
| DCLM | 模型过滤显著提升；7B/2.6T 64% MMLU | 保留为软门/外部分数，不替代硬门 |
| FineWeb-Edu | 教育性过滤 1.3T token | language 质量门（教育词/信息密度/重复率） |
| Nemotron-CC | 激进过滤删 90%；分类器集成 + 重写 | 三支决策 + 重写闭环；defer 不硬删 |
| Magpie | 4M 生成 → 300K 高质量筛选 | 生成与筛选分离；质量门 + 数据卡 |
