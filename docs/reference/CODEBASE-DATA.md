# 大型代码库数据：切分、质量门与上下文（2026-11-23）

> 计划与判据。大代码库（monorepo / 前后端仓库）不是“一条轨迹”，
> 不能只按 JSONL 行处理；要按仓库 → 文件 → 依赖图 → 测试单元切分。

## 0. 结论

- **上下文靠检索，不靠把整个仓库塞进 prompt**：RepoCoder（arXiv:2303.12570）用相似检索 + 迭代生成，
  在仓库级补全上显著优于只看当前文件；我们的数据管线也应保存“检索到的上下文”与“依赖边”。
- **质量门要分两级**：
  - 文件级：语言、大小、生成文件、许可证、重复、代码质量规则（已有）；
  - 仓库级：可解析/可编译/可测试（能跑就给可执行证据），依赖是否闭合，测试是否覆盖改动。
- **可执行验证优先**：SWE-bench（arXiv:2310.06770）用真实仓库 + issue + 测试；
  我们沿用“verifier + evidence”硬门，仓库级证据是测试命令与输出哈希。
- **许可证与归因**：The Stack v2（arXiv:2402.19173）给 SWHID 与许可过滤；StarCoder（arXiv:2305.06161）
  做 PII 与归因追踪；我们的数据卡必须记录 source/license/commit。

## 1. 仓库扫描（RepoUnit）

每个文件一条 `RepoUnit`：

```json
{
  "repo": "bfxh/trit",
  "commit": "....",
  "path": "crates/trit-learn/src/manifest.rs",
  "lang": "rust",
  "bytes": 5231,
  "imports": ["trit_core::fingerprint", "trit_learn::json"],
  "generated": false,
  "license": "MIT OR Apache-2.0",
  "tests": ["crates/trit-learn/tests/manifest.rs"]
}
```

规则：

- 语言用 `code_langs` 的同一套特征 + 扩展名；
- `generated` 用代码质量规则里的标记；
- `imports` 按语言解析（`use`/`import`/`require`/`#include`），解析不了就留空并标记 `imports_unknown`；
- `tests` 按路径/命名约定匹配（`tests/`、`*_test.*`、`*.test.ts`）。

## 2. 切分与采样

- **单元**：文件 → 函数/类（可选）→ 目录；保留导入闭包与测试文件。
- **分层采样**：按语言、目录、文件大小分桶；巨型文件与生成文件降权；每仓库设上限。
- **去重**：文件级 MinHash/SimHash；跨 fork 重复；同一内容不同路径只留一份。
- **上下文预算**：检索（BM25/embedding）+ 依赖闭包，按 token 截断；记录实际送入的上下文哈希。

## 3. 质量门（仓库级）

| 门 | 判据 | 失败动作 |
|---|---|---|
| `repo_schema` | repo/commit/path/lang 非空 | reject |
| `license` | 有许可证或明确允许 | defer/reject（可配） |
| `generated` | 自动生成标记 | reject |
| `dup` | 文件内容指纹重复 | reject |
| `imports` | 依赖可解析或显式标记 unknown | defer |
| `build` | 可选：编译/测试通过 | reject/defer |
| `tests` | 可选：改动有对应测试 | defer |

## 4. 上下文与评估

- **检索**：BM25 + 依赖边；按 RepoCoder 的迭代检索思路，先检索再生成，再检索。
- **评估**：RepoEval/RepoBench 的补全指标 + SWE-bench 式端到端解决率；同时报告检索命中率与上下文 token。
- **负结果**：检索不到依赖、测试跑不起来、许可证缺失都要入册，不静默丢弃。

## 5. 与 `trit-data` 的落点

- 新增 `repo` 模式（`--repo <dir>`）：扫描仓库生成 `RepoUnit` JSONL，再走现有 gate/去重/配比；
- 数据卡增加 `repo` 统计：语言分布、文件大小、生成文件、许可证、依赖解析率；
- 现有 `--jsonl <dir>` 只处理轨迹 JSONL；`repo` 模式处理源码树，两者共用质量门与报告。

## 6. 实施顺序

- **R0（下一步）**：`repo` 扫描 + RepoUnit + 文件级质量门 + 数据卡；
- **R1**：import 解析 + 依赖闭包 + 测试匹配；
- **R2**：检索上下文构建 + RepoEval 式评估 + 去重（MinHash）。

## 7. 边界

- 不把整个仓库当一条样本；不省略许可证/来源；
- 构建/测试是可选门（环境可能不具备），但一旦声称“可执行”，必须给命令与输出哈希；
- 生成文件、vendor、第三方副本默认排除，除非显式要求。
