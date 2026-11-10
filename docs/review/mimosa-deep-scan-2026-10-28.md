# Mimosa 深度安全扫描复核（2026-10-28）

- 对象：本仓工作区（含 PR #169 的 trit-learn / trit-attn 新增代码）
- 扫描：Mimosa `deep`，static-only（不执行任何项目代码）
- scanId：`scan-2026-10-27T18-40-12.710Z-0634a35295e2`
- 封印：`sha256:b17e5eb84118226d09dcf09e854ad069be9e067e9a8fdc67d98e3ab19301d54a`
- 运行状态：**inconclusive**（调用图部分不完整：动态派发/超出分析规模；
  部分分析阶段未完整覆盖）→ 不构成"通过"或"不通过"的结论
- 发现：6 条，**逐条人工复核后全部为误报或构建产物**，明细如下

## 逐条复核

| # | 级别 | 位置 | 扫描器说法 | 复核结论 |
|---|---|---|---|---|
| 1 | HIGH | scripts/mutation_check.py:2350 | 外部数据进入 eval，可执行任意 Python | **误报**。该行是 MUTANTS 列表里一个**字符串字面量**（Rust 变异锚点 `eval(&lhs, &empty, alg)`），不是 Python `eval()` 调用；全文件无 `eval(` 的真实调用（含引号过滤的机检复核为空）。 |
| 2-5 | MEDIUM | module_budget.py / mutation_check.py / pr_hygiene.py / publish_graph.py | 命令行参数 → examples/ternary_corpus.rs:118 的文件操作 | **误报**。`examples/ternary_corpus.rs` 文件**不存在**（examples/ 下只有 .net/.json 资产），四个脚本也没有任何指向它的引用；跨文件污点是匹配器把不相关的文件名拼到了一起。 |
| 6 | MEDIUM | target/doc/static.files/search-*.js | MongoDB 动态排序字段注入 | **构建产物**。该文件是 rustdoc 生成的静态搜索 JS（rustc 工具链产物，非本仓源码）；扫描器应排除 `target/`。 |

## 边界与后续

- 本次扫描**不宣称项目安全**：状态 inconclusive 且只覆盖静态证据面；
  依赖面 218 个包离线匹配 0 条已知 advisory（1 个未知源）。
- 已从这次扫描沉淀的两个工具改进（都在本 PR）：
  - `trit-unsafe`：RISKY_APIS 补 MaybeUninit / mem::forget / write_bytes，
    `unsafe extern`（edition 2024）声明块纳入扫描面，并修掉
    `extern "C"` 这个因 scrub 抹字符串而**从未匹配过**的死模式（改为 `extern`）；
  - `trit-lsp`：编辑期深层三值诊断（见 CHANGELOG），把"语义混用"这类
    三进制特有问题提前到写代码时暴露。
- 建议：给 Mimosa 配置排除 `target/`（构建产物不进扫描面）。

## 追加：第二轮重扫（自审同日，覆盖 #169 合并后全部代码）

- scanId：`scan-2026-10-27T20-58-44.330Z-b6872d75a56e`
- 封印：`sha256:062c76e7987ca52556bfa78dd5988e97ed37e2a590e1f8880441f5965c8008b4`
- 同样 **inconclusive**、6 条发现，与第一轮同类：`eval` HIGH 仍是变异锚点字符串、
  4 条跨文件污点仍指向不存在的 `ternary_corpus.rs`、MongoDB 仍在 rustdoc 产物。
  复核结论不变（全部误报/构建产物）；依赖面 218 包离线 0 条已知 advisory。
