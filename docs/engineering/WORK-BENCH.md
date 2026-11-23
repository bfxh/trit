# WORK-BENCH：工作任务产物基准

**一句话**：`agent_bench` 证明“代码补丁能执行、错误补丁能被拦住”；`work_bench`
证明“办公任务的产物能被确定性检查”。它不跑 GitHub PR，也不跑编译器，而是把
文档、表格、JSON、CSV、XLSX 和文件树作为交付物，用参考产物 + 负例产物 + 检查器
做可复算的工作任务矩阵。

## 1. 为什么单独做一套

代码基准看的是 patch 和测试命令；工作任务看的是**产物**：

- 文档：标题、章节、关键事实、最小篇幅、禁用占位词；
- 表格：列名、行数、空值、按主键查询、预算公式；
- JSON 记录：字段、精确值、列表长度、嵌套 bullet 数；
- 日历/计划：ISO 时间、区间不重叠、依赖无环；
- 文件整理：文件被移动而不是复制，映射表可核对；
- 合规/抽取/翻译：风险编号、发票字段、术语命中。

这些检查不需要 LLM 判分，也不依赖外部服务；同一份 `tasks.json` 在任何装有
Python 3.10+ 的机器上给出同一读数。

外部谱系：OfficeBench、WorkBench、OdysseyBench、APEX-Agents、OfficeEditBench、
EVOCowork、OmegaUse-OfficeVal、EnterpriseClawBench，以及 TheAgentCompany、
OSWorld 的办公/长程工作流切片。本仓库只实现一个**本地、合成数据、可复算**的
切片，不声称复现这些榜单的规模或真实企业数据。

- OfficeBench: https://arxiv.org/abs/2407.19056
- WorkBench: https://arxiv.org/pdf/2405.00823v2.pdf
- OdysseyBench: https://arxiv.org/html/2508.09124
- APEX-Agents: https://arxiv.org/pdf/2601.14242v2
- OfficeEditBench: https://arxiv.org/pdf/2609.34469
- EVOCowork: https://arxiv.org/pdf/2610.14828
- OmegaUse-OfficeVal: https://arxiv.org/pdf/2607.27155v2
- EnterpriseClawBench: https://arxiv.org/pdf/2606.23654

## 2. 当前矩阵

20 个任务、15 个类别、81 条检查、22 个产物路径（部分任务产出多个文件）。

| # | 任务 | 类别 | 产物 | 检查重点 |
|---|---|---|---|---|
| 1 | `doc-project-brief` | document | `brief.md` | 标题/目标/范围/里程碑/预算/风险/负责人、关键事实、最小篇幅、无 TODO |
| 2 | `doc-meeting-notes` | document | `meeting.md` | 结论/决策/行动项表格、负责人、截止日期、背景 |
| 3 | `doc-research-brief` | research | `research.md` | 三份来源引用、对比表、建议、最小篇幅、无绝对化断言 |
| 4 | `doc-release-notes` | document | `release.md` | 版本号、新增/修复/兼容性/风险、工单号 |
| 5 | `doc-handoff` | handoff | `handoff.md` | 当前状态/已变更/下一步/阻塞/接手人/验证记录 |
| 6 | `data-clean-csv` | data | `clean.csv` | 固定列、10 行、无空值、按 id 查询 score |
| 7 | `data-weekly-metrics` | data | `metrics.json` | week/orders/revenue/aov/growth 精确值 |
| 8 | `data-budget-xlsx` | finance | `budget.xlsx` | 原生 XLSX、`Budget` 表、`B3` 必须是 `=SUM(B2:B3)` |
| 9 | `data-variance` | data | `variance.csv` | 列、4 行、正负差异可复算 |
| 10 | `email-status` | email | `email.json` | 收件人/主题/正文、三项事实、正文长度、无占位 |
| 11 | `slides-outline` | slides | `outline.json` | 7 页、每页 3 条 bullet、风险/下一步 |
| 12 | `calendar-plan` | calendar | `schedule.json` | 4 个时间块、ISO 时间、无重叠、任务齐全 |
| 13 | `file-triage` | files | `triage.json` + `sorted/**` | 文件被移动、映射正确、原 inbox 不再残留 |
| 14 | `form-fill` | form | `form.json` | 姓名/邮箱/金额/日期，金额为数字 |
| 15 | `extract-invoice` | extraction | `invoice.json` | vendor/date/total/tax 四字段 |
| 16 | `kb-answer` | research | `answer.json` | 30 分钟/72 小时、两份来源、禁止“不知道/立即” |
| 17 | `compliance-review` | compliance | `flags.json` | PII-01、RET-02、high/medium 风险 |
| 18 | `translation-glossary` | translation | `translated.md` | 译文、术语数、关键术语按术语表 |
| 19 | `task-plan` | planning | `plan.json` | 6 个任务、负责人、依赖无环 |
| 20 | `risk-register` | risk | `risks.csv` | 列、5 行、high 风险、负责人 |

## 3. 一个任务由什么组成

```json
{
  "id": "data-budget-xlsx",
  "category": "finance",
  "prompt": "读取 budget.csv，生成 budget.xlsx……",
  "inputs": {"budget.csv": "..."},
  "artifacts": {"budget.xlsx": {"format": "xlsx", "sheets": [...]}},
  "negative": {"budget.xlsx": {"format": "xlsx", "sheets": [...]}},
  "checks": [
    {"artifact": "budget.xlsx", "type": "xlsx_sheet", "values": ["Budget"]},
    {"artifact": "budget.xlsx", "type": "xlsx_cell", "sheet": "Budget", "cell": "B3", "equals": "=SUM(B2:B3)"}
  ]
}
```

- **参考产物**必须让全部检查通过；
- **负例产物**至少让一条检查失败，否则该任务不算有效；
- 检查器按产物路径读取文件，不支持“只检查 prompt”或“让人打分”；
- `remove_inputs` 用于“移动文件”类任务；参考产物会删除原输入，负例保留，
  从而让 `files_absent` 检查真正区分完成与未完成。

## 4. 检查器

| 组 | 类型 |
|---|---|
| Markdown | `md_sections`、`md_phrases`、`md_min_chars`、`md_forbidden` |
| JSON | `json_keys`、`json_value`、`json_min_chars`、`json_contains`、`json_forbidden`、`json_list_len`、`json_list_field_min`、`json_no_overlap`、`json_dag` |
| CSV | `csv_columns`、`csv_row_count`、`csv_no_nulls`、`csv_lookup` |
| XLSX | `xlsx_sheet`、`xlsx_cell`、`xlsx_formula` |
| 文件 | `files_exist`、`files_absent` |

`xlsx_formula` 用 `openpyxl` 以 `data_only=False` 读取公式字符串，不把缓存值
当成公式；因此 `B3=540` 不会冒充 `=SUM(B2:B3)`。

## 5. 读数

本机 2026-11-23，Windows / Python 3.13 / openpyxl 3.1.5：

```text
python tools/work_bench.py -SelfTest
work-bench self-test: 182/182

python tools/work_bench.py -Run -Out target/agent-quality/work-bench-report.json
work-bench: 20 tasks / 81 checks / reference 81/81 / negative 20/20 / wall 0.428s

python tools/work_bench.py -Run -Repeat 2 -Out target/agent-quality/work-bench-repeat2.json
两次运行：reference 81/81、negative 20/20、repeat_all_ok=true，wall 0.552s
```

`-SelfTest` 的 182 项由三部分组成：1 项 suite 结构校验、81 项参考产物检查、
20 项负例检出检查，再加每个任务的 4 项结构不变量（20×4=80）。完整自测还会由
`tools/selftest.ps1` 的 `AT1/AT2` 再验一次。

`-Candidates <dir>` 只复制候选目录里**真实存在**的 `<task-id>/...` 产物；缺失文件
直接失败，不会回退到参考产物。候选不会因为“什么都不做”而通过。完整候选产物
读数 81/81；空候选目录只得到 0/5，`-Strict` 返回 1。

完整仓库自测（`pwsh tools/selftest.ps1`）读数 **681 passed / 0 failed / 0 skipped**，
其中 `AT1`/`AT2` 是工作任务断言，`AU1`/`AU2` 是 PTC 命令行降错断言。

## 6. 一条命令跑完（PTC 降错）

`testdata/work-bench/ptc-plan.json` 把原来的多条手工命令收成 4 个步骤：

1. preflight：Python/openpyxl；
2. `work_bench.py -SelfTest`，核验 `182/182`；
3. `work_bench.py -Run`，核验报告里 `metrics.failed=0`、reference 81/81、negative 20/20；
4. `work_bench.py -Run -Repeat 2`，核验 `repeat_all_ok=true`。

```powershell
python tools/cmd_driver.py -Plan testdata/work-bench/ptc-plan.json -Out target/agent-quality/ptc-receipt.json
```

实测：

```text
cmd-driver: 4/4 steps ok / quality-gates 13/13 / retries 0 / errors 0
            context 117B (local-detail 51172B) / wall 1.92s
```

这不是用“少报错”换“少检查”：13 条核验里包含 work-bench 的参考产物 81/81、负例 20/20 和重复一致性；任何一条失败都会让驱动退出 1。完整报告和 stdout/stderr 留在 receipt，摘要只回 117B。`cmd_driver.py` 自身的 34/34 自测覆盖缺失可执行文件、坏 cwd、低磁盘、非零、超时、核验失败、无盲重试和 dry-run。

## 7. 边界与下一步

**已证明**：

- 20 类工作任务在本地可重复执行，参考产物 81/81、负例 20/20；
- 文档、JSON、CSV、原生 XLSX、文件移动五类产物都进入同一套确定性检查；
- 两次重复执行一致，且不依赖网络、模型或外部数据；
- 以 `-Task` / `-Category` 可切片复算，`-Out` 输出逐检查明细。

**没有证明**：

- 文档语义质量、事实真实性、写作风格——检查只覆盖夹具中编码的显式事实；
- 真实企业数据、真实客户文件、真实邮件/日历账号；
- OfficeBench 规模的多应用长程任务；当前是 20 个合成切片；
- DOCX/PPTX 原生产物尚未接入（当前文档用 Markdown，演示用 JSON 大纲，
  表格用原生 XLSX）。下一步可安装 `python-docx` / `python-pptx` 并扩展
  `docx_*` / `pptx_*` 检查器，但不应把“能读文本”冒充“版式正确”。

**扩展方向**：DOCX/PPTX 原生格式、跨应用工作流（表格→文档→邮件）、
日历与会议邀请、真实附件抽取、更多负例（缺字段、错公式、漏引用、错日期、
重复文件）、以及把 `-Candidates` 接到实际 agent 产物目录做回归。
