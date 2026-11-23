# 上下文压缩、搜索评分与自动推理强度

> 状态：**已落地并自测**（`context_window.py` 23/23、`search_policy.py` 22/22；
> `agent_settings.py` 默认档新增 `context/search/reasoning` 三段，23/23。命令见 §5）。
> 这些读数是本机 Python 自测，不等于真实模型 tokenizer 的精确计数。

## 1. 为什么先做“预算计划”，而不是直接摘要

1M 窗口不等于可以无脑塞满。用户的口径是 **1M 窗口、850K 自动触发、压缩到
700K 附近**。如果只做“超了就把中间消息删掉”，证据链会断，模型也会在不知情
的情况下丢掉早期约束。因此本实现把压缩拆成可审计的三步：

1. 用确定性估算器算每条消息的 token（ASCII ≈ 4 字符/token，非 ASCII ≈ 1.5
   字符/token；消息可显式给 `tokens`）。
2. 保锚点：`system`、第一条 user、最后一条 user、`pinned`、
   `priority >= 9`、以及 `evidence` / pinned 的 tool 结果。
3. 只从中间候选里按“低优先级 + 较旧”先丢；每丢一段就把它的索引、角色和
   关键行写进一条 `compressed_summary` 摘要。若锚点 + 摘要仍压不到目标，返回
   `refused`，不返回半截 transcript。

摘要不是模型生成的语义总结，而是抽取式证据索引：原始消息序号、角色、含数字/
URL/路径/引文的行。这样模型可以看到“删了什么”，审计也能复算。

## 2. 默认 envelope 与设置

```json
{
  "version": 1,
  "context": {
    "window_tokens": 1000000,
    "auto_compact_at": 850000,
    "target_tokens": 700000,
    "recent_keep": 6,
    "preserve_tool_evidence": true
  },
  "search": {
    "max_results": 10,
    "budget_tokens": 12000,
    "min_relevance": 0.15,
    "max_result_tokens": 2000,
    "dedupe": true,
    "freshness_half_life_days": 30,
    "trusted_domains": [],
    "safety_block": ["critical", "high"]
  },
  "reasoning": {
    "mode": "auto",
    "effort": "medium",
    "min_effort": "low",
    "max_effort": "high"
  }
}
```

优先级仍是：内置 < 用户级 < 项目 `.agent/settings.json` < `--settings` < CLI。
`receipt_echo` 只回显数值/模式，不带本机绝对路径。

## 3. 搜索评分

每条结果先过工作台既有的文字门（`context_guard.scan_text`），再做：

| 维度 | 计算 | 作用 |
|---|---|---|
| relevance | 查询词覆盖率 0.55 + BM25 0.30 + 精确短语 0.15 | 先保证“答得上” |
| safety | `clean/low/medium/high` → 1/0.7/0.4/0 | high/critical 直接不选 |
| authority | trusted 0.5 + official/docs 0.2/0.1 + https 0.05 | 不把匿名转发当权威 |
| freshness | 半衰期 30 天，缺失按 0.5 | 旧文不掩盖新规 |
| cost | 标题+摘要+URL 的 token 估算 | 硬预算，不超发 |

最终 `score = 0.55*relevance + 0.15*authority + 0.10*freshness +
0.10*safety + 0.10*clean`。选择阶段用 MMR/标题摘要 Jaccard 去重，按 token
预算和 `max_results` 双硬帽；每条落选结果都带原因：低于相关度、被安全门拦截、
超大、超预算、重复、达到上限。

## 4. 自动推理强度

`search_policy.choose_effort` 只看可解释信号，不接模型自评：

- 查询长/多跳（同时、分别、以及）加分；
- 对比、因果、高 stakes（安全/合规/医疗/法律/投资/跨境/漏洞）、时效、歧义、
  结果冲突加分；
- 极短定义型查询减分；
- 阈值：`<0.36` → low，`<0.62` → medium，否则 high。

`params` 同时给出 `search_depth`、`max_results`、`verify`、`budget_tokens`。
用户可用 `reasoning.mode=fixed` 强制档位；`min_effort/max_effort` 做夹取。
阈值由 22 条自测钉住，改阈值必须先改测试和文档。

## 5. 命令

```bash
python tools/context_window.py plan --transcript transcript.json --out context-plan.json
python tools/context_window.py self-test

python tools/search_policy.py plan --query "2026 跨境数据合规政策变化" \
  --results results.json --budget-tokens 12000 --out search-plan.json
python tools/search_policy.py self-test
```

`context_window` 的 `status` 为 `noop|compacted|refused`；`refused` 退出码 3，
调用方必须换策略（提高目标、拆会话或人工删减），不得把空 transcript 当成功。
`search_policy` 的 `status` 为 `ok|empty|no_safe_results|disabled`；后两者
不会伪造“已检索”。

## 6. 测试与 CI

- `context_window.py`：默认 1M/850K/700K、noop、压缩后 ≤ 目标、首末 user/
  system/pinned 保留、摘要存在、锚点溢出拒绝、非法配置拒绝、CJK 估算、
  确定性哈希、保留消息不被改写，共 23 条。
- `search_policy.py`：相关结果排序、无关/注入/重复/超预算/超上限落选理由、
  低/中/高档位、强制档位、空查询/空结果拒绝、设置禁用、相关度地板、
  新鲜度排序、确定性，共 20 条。
- `context_window.py stress --cases 100` 与 `search_policy.py stress --cases 100`：
  随机 transcript / 随机搜索结果，钉住“不超目标、不超预算、锚点不丢、拒绝不
  返回半截、排除有理由、输入不被改写、输出可复现”。
- `context_search_cross.py`：搜索选中的证据进入 transcript 后仍能被压缩保留；
  同一项目 `.agent/settings.json` 同时驱动两个 CLI；锚点溢出 CLI 退出 3 且不
  返回 messages；14/14。
- 注册表：`C074`（上下文压缩）、`C075`（搜索评分）、`C076`（交叉接缝）进入
  fast tier；CI 的 `agent checks (registry)` 每次跑。`selftest.ps1` 增加
  AV9/AV10/AV11。

## 7. 诚实边界

- 这是**预算规划器 + 抽取式摘要**，不是模型语义摘要；摘要质量不承诺。
- token 数是估算，不是厂商 tokenizer 的精确值；真实调用前应保留安全余量。
- 安全分复用 `context_guard` 的启发式规则，不能替代对齐、权限、沙盒和人工
  复核；它会漏掉语义改写与多轮稀释攻击。
- relevance 是词法分，不是语义 reranker；同义词/跨语言召回不足时，应扩大
  检索或接语义模型，而不是偷偷降低 `min_relevance`。

## 8. 未落地

- 与具体模型厂商 tokenizer 的对齐；目前只有可复算估算。
- 真实对话运行时的自动调用点；本仓库提供 CLI/库与设置，宿主需在发送模型前
  调用 `context_window.plan`。
- 语义摘要与跨会话记忆压缩；当前只做抽取式证据索引。


