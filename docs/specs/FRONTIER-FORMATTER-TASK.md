# 任务：前沿格式化器（prettier 类）——Rust 重写且全面更强

## 级别与当量

**S**（跨实现、新抽象、有独立 oracle、允许负结果）；预计 3–10 天；可与其它线并行 1 条。

## 起点

- 仓库：`bfxh/trit`
- 分支：`feat/frontier-formatter`
- 起点 SHA：派发时以 `origin/main` 当时的 SHA 为准
- 环境：Rust 1.89+；Node 20+（**只作 oracle**，不进入交付物）

## 必读（按顺序）

- `docs/specs/FRONTIER.md` §1.4（质量不变的超越：基线 / 读数 / 负结果）
- `docs/AGENT-TASK-STANDARD.md` §2（S 级四门槛）
- `docs/tech-stack-policy.md`（新增 `.py` 只准落测试路径；实现必须是 Rust）
- `docs/positioning/LAYERS.md`（新代码归位判据）

## 目标（G1..G5）

| 编号 | 目标 | 判据（可复算） |
|---|---|---|
| G1 | 语义保持 | 语料 C（≥200 个 JS/TS/JSON/Markdown 文件）格式化后 **token 序列不变**：用 oracle 解析两边并比对 token 流，DIFF 0 |
| G2 | 幂等 | `format(format(x)) == format(x)`，语料上 0 反例 |
| G3 | 吞吐与并发 | 同机同语料：单线程吞吐 ≥ prettier 的 **2×**；4 线程 ≥ **3×**；峰值 RSS ≤ prettier |
| G4 | 注释与空行 | 注释不丢失、不换归属；连续空行折叠规则与 prettier 一致（逐条断言） |
| G5 | 接入默认链 | 登记进 `docs/specs/DEFAULT-TOOLS.json`；替换一个真实调用点并保留回退开关 |

目标值在派发时以本机实测基线为准；读数写进 `docs/results/`，负结果同样入册。

## 非目标

- 不实现 prettier 的完整插件生态（只做 JS/TS/JSON/Markdown 核心）。
- 不改 prettier 的选项语义（`semi` / `singleQuote` / `printWidth` 等保持兼容）。
- 不引入 Node/JS 运行时作为生产依赖（oracle 仅在测试与 bench 里用）。

## 验收（可复算）

```bash
# 语义与幂等
cargo test -p <新 crate> --test formatter_corpus

# 性能读数（输出 JSON 行）
cargo bench -p <新 crate> --bench formatter

# oracle 基线（同机、同语料）
node tools/frontier/bench_prettier.js --corpus <语料目录> --out target/prettier-baseline.json
```

期望：语料 DIFF 0；幂等 0 反例；吞吐与内存三条同时达到 G3 的目标值（达不到则负结果入册，保留 prettier 为默认）。

## 禁区

- 不推 `main`；不抬 `scripts/module_budget_baseline.json` 台账。
- 不降低三值/性质判据的含义；不把"差分为 0"写成"已证明等价"。
- 不新增生产路径的 `.py`（语言策略硬约束）。
- 不碰 `crates/trit-core` 的三值语义（本任务只做格式化层）。

## 失败模式与回退

- 无法覆盖的语法（装饰器、JSX 边缘、TypeScript 类型语法的最深嵌套）→ 记录到负结果表，
  退化为**原文透传 + 显式警告**，不许静默改写。
- 性能未达到 G3 的目标值 → 保留 prettier 为默认件，写清差多少与原因；不许"先替换再优化"。
- oracle 行为不确定（prettier 版本差异）→ 固定 oracle 版本并记录，不许拿不同版本对比。

## 交付物

- Rust 实现（新 crate，按 `LAYERS.md` 归位并登记 `scripts/layers.json`）。
- 语料 + 性质测试 + bench + oracle 脚本。
- 文档：实现说明、读数（`docs/results/`）、负结果、`DEFAULT-TOOLS.json` 登记。
- PR 描述含三条读数（吞吐 / 内存 / 并发扩展）与 oracle 版本。

## 自检清单

- [ ] 语料 DIFF 0 且语料来源可复算
- [ ] 幂等与注释位置逐条断言
- [ ] 性能三条读数齐全，含未达到目标值的项
- [ ] `python scripts/module_budget.py` 绿（新文件已登记）
- [ ] `cargo test -p trit-policy --test frontier_design` 绿（默认件清单完整）
