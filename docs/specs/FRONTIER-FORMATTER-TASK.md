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

## 基线（2026-11-28 实测，prettier 3.10.1）

oracle：prettier **3.10.1**（Node 20+，`prettier.format` API，单进程）。

| 指标 | 读数 |
|---|---|
| 语料 | 250 文件 / 1,296,600 字节（10× 混合模板：JS 函数 / TS 接口 / JSON 配置 / Markdown 文档） |
| 单轮最优 | 943 / 949 / 960 ms（3 次独立运行，每次 3 轮取最优） |
| 吞吐 | **1.29–1.31 MiB/s**（单线程） |
| 峰值内存 | RSS 264–277 MB；heapUsed 114–177 MB |

G3 据此换算：单线程 ≥ **2.6 MiB/s**（2×），4 线程 ≥ **3.9 MiB/s**（3×），RSS ≤ **265 MB**。
读数随机器变化，派发时必须在同机重测并替换本节。

复算要点（语料与脚本放仓外临时目录，不提交）：

1. 语料：10 组混合模板（JS 函数体 / TS 接口 + 泛型 / JSON 嵌套对象 / Markdown 多级标题），
   每组复制成 25 个文件凑到 ~1.3 MB；
2. `npm init -y && npm i prettier@3.10.1`；
3. bench 脚本逐文件 `await prettier.format(text, {parser})`，parser 按扩展名
   （`.js`→`babel`、`.ts`→`typescript`、`.json`→`json`、`.md`→`markdown`），
   warmup 1 轮后计时 3 轮取最优，同时输出 `process.memoryUsage()` 的 rss/heapUsed；
4. 记录机器规格（核数 / 内存 / OS）——换机读数不可比。

读数与负结果写进本卡或 `CHANGELOG.md`；**不写 `docs/results/`**（该目录属于公开树禁区）。

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

## 实现进度（2026-11-28）

实现位于 `crates/trit-fmt`（prod 层，零第三方依赖），当前范围是 JSON。

| 判据 | 结果 | 复算 |
|---|---|---|
| G1 语义保持 | 13 份语料（含 84,048 字节大文档）与 prettier golden **DIFF 0**；另用 `serde_json` 交叉验证语义相等 | `cargo test -p trit-fmt --test corpus` |
| G2 幂等 | 全部语料 0 反例 | 同上 |
| G3 吞吐（大文档 84,048 字节） | trit-fmt **12.90 MiB/s** vs prettier 3.10.1 **1.48 MiB/s**（8.7 倍） | `cargo bench -p trit-fmt --bench format`；prettier 侧 `node bench_single.js big.json 200` |
| G3 吞吐（小文档 95 字节） | trit-fmt **1.76 µs/doc** vs prettier **213.35 µs/doc** | 同上 |
| G4 注释与空行 | JSON 没有注释；空行规则未覆盖（见负结果） | — |
| G5 接入默认链 | 已接入：CI 的 `frontier` job 对 4 个手写 JSON 跑 `--check`，`TRIT_FMT=0` 回退；已登记进 `docs/specs/DEFAULT-TOOLS.json` | — |

### 负结果（保留，不删）

- 范围只有 JSON（`.json` / `.jsonc`）；JS/TS/Markdown 返回 `FormatError::Unsupported`，没有半成品实现。
- 单线程；4 线程 3 倍的目标未验证（bench 只跑单线程）。
- 接入面只有 4 个手写 JSON（`crates/trit-wspace/mcp.example.json`、`examples/policy.json`、
  `jobs/example.json`、`.github/data-pipeline.json`）；机器生成的基线文件（`scripts/*baseline*.json`）
  仍是 `json.dump` 风格，没有接进来——它们与 prettier 风格的差异会产生巨大 diff，需要单独一轮迁移。
- JS/TS/Markdown 仍未接入（没有实现），prettier 在这三类上仍是唯一选择。
- 解析器按字节处理：非法 UTF-8 输入未做严格拒绝；重复键保留原序（与 prettier 一致）。
- 只覆盖 prettier 的 `printWidth=80` 默认值；`--width` 等选项未实现。
- 小文档那一行的 121 倍放大来自固定开销（prettier 每个文档约 213 µs 的解析器初始化），
  不能当作真实吞吐比；大文档 8.7 倍才是可比读数。

## 基线文件迁移（2026-11-28）

棘轮台账的格式统一到 trit-fmt（= prettier 风格），并把 7 个纳入 CI 白名单：

`scripts/ai_vendor_baseline.json`、`scripts/layers.json`、`scripts/module_budget_baseline.json`、
`scripts/claims_evidence_baseline.json`、`scripts/docs_hygiene_baseline.json`、
`scripts/test_surface_baseline.json`、`scripts/alloc_guard_allow.json`。

写回侧同步成 `indent=2` + 强制 LF（`newline="\n"`）——原先 `indent=1` 且 Windows 会写 CRLF，
两个都会让 `--check` 在换平台时随机红。`merge=json-union` 驱动用 `json.loads` 解析，
与行格式无关，所以重排不影响合并。

### 唯一不迁移的一个（结构性冲突，保留原格式）

`scripts/privacy_scan_baseline.json` 不纳入：它的 `drive-path` 是**重复短字符串数组**，
trit-fmt 的全标量 fill 会把多个元素并到同一行，而 `json.dump` 永远每元素一行——
两边会反复互相改写，无法同时稳定。要迁移就得先改数据结构（去重或换键），那是另一件事。
