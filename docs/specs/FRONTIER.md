# 前沿设计宪章

> 一句话：主库与编程模型只用**前沿做法**；老旧设计默认不沿用，除非能给出它**今天仍然成立**的判据。
> 用户口径（2026-11-28）：要的是"前沿、新东西、全面比他们强"；很多老旧设计已经从各方面不堪用，
> 不需要再沿用——不是"照抄主流就算对"。

## 1. 五条硬约束

### 1.1 默认 MCP 与默认工具

主库与编程模型**默认**走这两条链，而不是每个项目各写一份：

- 策略执行面：[tech-stack-mcp](../../tools/tech-stack-mcp/README.md)（MCP 工具 `validate_path` / `policy`）；
- 验收编排面：`tools/agent_verify.py`（任一闸拒绝则整体拒绝）。

新增"默认件"必须登记到 `DEFAULT-TOOLS.json`（同目录），并写清它替代了什么、凭什么更强。

### 1.2 反老旧设计

每次改设计必须同时写清三件事：

1. **旧做法是什么**（具体到结构/接口，不写"以前比较乱"）；
2. **为什么不沿用**（可观测理由：性能、错误面、并发、可判性）；
3. **新做法的判据**（命令 + 读数或性质断言）。

没有对照说明的改动按"旧设计延续"处理，不进 main。判据落在评审与 `CHANGELOG.md` 条目里。

### 1.3 禁上帝对象（上游与下游都禁）

- 单文件 ≤800 行（`scripts/module_budget.py` 棘轮，只减不增）；
- 单类型职责面（`pub` 方法 / 字段数量）扩张必须同 PR 拆分，或在 PR 里写明为什么不能拆；
- **下游依赖方同样适用**：不因为"只是消费方"就放行巨型结构或隐式全局状态。

### 1.4 质量不变的超越

任何"更强 / 更快 / 更省"必须同时给三件东西，缺一不算：

| 要素 | 要求 |
|---|---|
| 基线 | 对标的实现与版本（官方实现、第二实现或朴素 oracle） |
| 读数 | 可复算命令 + 数字（吞吐、内存、并发扩展、延迟分位） |
| 负结果 | 哪些维度**没有**超越、为什么，写进文档而不是删掉 |

性能、并发、内存任一维度退化超过 10% 即视为未达成；"质量不变的超越"优先于"功能更多"。

### 1.5 数学验证与数值模板

- 数值路径默认走既定模板口径（量化 / STE 口径统一在 `crates/trit-train`），不各写一套；
- 关键性质必须有 Verus、Lean 或独立 oracle 的对拍，不接受"自我一致性"（见
  [AGENT-TASK-STANDARD](../AGENT-TASK-STANDARD.md) §2 的 S 级四门槛）；
- 无法形式化的部分（浮点内核、外部格式）必须明确写成"差分证据"，不得表述为定理。

## 2. 门禁映射（机器可判）

| 约束 | 判据 | 命令 |
|---|---|---|
| 默认工具登记 | 清单字段完整 + 路径存在 | `python3 scripts/frontier_gate.py` |
| 上帝对象 | 行数棘轮 | `python3 scripts/module_budget.py` |
| 能力句有据 | 复算指针或读数 | `python3 scripts/claims_evidence.py` |
| S 级四门槛 | 任务包字段齐全 | `python3 scripts/frontier_gate.py --tasks` |
| 数学验证 | Verus/Lean 报告 | `python3 scripts/verify.py` |

## 3. 边界（如实）

- 门禁判的是"有没有登记 / 有没有读数"，判不了"设计是否真的更前沿"；后者靠评审与负结果。
- 存量不追溯：新规则只约束新增与覆盖（与语言策略同口径）。
- 工具重写（如 prettier 类格式化器）按 S 级任务包派发，见
  [FRONTIER-FORMATTER-TASK.md](FRONTIER-FORMATTER-TASK.md)；重写后必须先测试、再替换默认件。

## 4. 相关文档

- S 级门槛：[AGENT-TASK-STANDARD.md](../AGENT-TASK-STANDARD.md)
- 技术栈（Python 只准测试代码）：[tech-stack-policy.md](../tech-stack-policy.md)
- 新代码归位：[LAYERS.md](../positioning/LAYERS.md)
- 状态口径（能写什么词）：[STATUS.md](../positioning/STATUS.md)
