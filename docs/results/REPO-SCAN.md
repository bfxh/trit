# 本地代码库全量扫描读数（2026-11-23）

> `trit-data repo` + R1 依赖图在真实仓库上的第一次全量读数。
> 目的不是给"数据质量"打分，而是确认扫描/解析在真实树上不崩、指标可复算，
> 并量出解析率的构成。

## 命令

```bash
cargo run -q -p trit-data --bin trit-data -- repo --repo <目录> --out <输出目录>
```

输出：`repo.units.jsonl`（每文件一条 RepoUnit）、`repo.deps.json`
（nodes + edges + stats）、`repo.report.json`（语言/许可证/覆盖率/依赖统计）。

## 读数

| 仓库 | 文件 | 主要语言 | 生成 | 测试文件 | 依赖边 | 仓库内 / 未解析 | 解析率 | 有测试的源文件 | 测试关联 | 环上 |
|---|---|---|---|---|---|---|---|---|---|---|
| trit-sec-halluc | 1808 | rust 1341 / md 169 / py 124 / toml 65 | 13 | 273 | 1529 | 1529 / 7608 | 16.7% | 107 | 433 | 101 |
| trit-kernel | 175 | rust 148 / md 20 / toml 6 | 0 | 58 | 116 | 116 / 927 | 11.1% | 34 | 86 | 4 |
| trit-fabric | 37 | java 28 / md 5 / py 3 | 0 | 3 | 60 | 60 / 121 | 33.1% | 0 | 0 | 0 |
| trit-mc | 32 | rust 20 / md 6 / toml 3 | 0 | 1 | 44 | 44 / 84 | 34.4% | 2 | 2 | 0 |

许可证：trit-sec-halluc MIT 1808、trit-kernel MIT 175、trit-mc MIT 32、
trit-fabric `missing` 37（仓库根没有 LICENSE/COPYING/NOTICE，扫描不向上层目录猜）。

## 解读

- **解析率不是质量分**：`unresolved` 里绝大多数是外部依赖（`std::*`、`serde`、
  `tokio` 等）与第三方包名，本来就映射不到仓库内文件。要评"依赖闭合"
  应看仓库内边 / 仓库内可解析 import，而不是本文的 16.7%。
- **`imports_unknown` 是语言覆盖缺口**：trit-sec-halluc 的 331 个文件是
  markdown / toml / yaml / protobuf 等没有 import 语法的文件，不是解析失败。
- **环上 101 是文件级环**：Rust 的 `mod` 声明与 `use` 互引（`lib.rs` ↔ 子模块）
  天然成环；Kahn 统计用于发现"重复引用成环"，不是错误信号。
- **测试关联可用**：trit-sec-halluc 107 个源文件有对应测试、433 条关联边；
  trit-kernel 86 条；`trit-fabric` 的 Java 测试按 `*Test.java` 命名，
  当前约定未覆盖（R2 补 JUnit 约定）。
- **生成文件与许可证**：13 个生成文件全部在 `target/` 之外被标记；
  `trit-fabric` 缺许可证是仓库事实，数据卡会如实记录 `license_missing`。

## 已知缺口（R2）

1. `unresolved` 细分：外部依赖 / 解析失败 / 动态导入（`importlib`、`require` 变量）。
2. 测试约定补齐：`*Test.java` / `*IT.java`、`pytest` 的 `conftest.py`、Rust `#[cfg(test)]` 内联。
3. 检索上下文：BM25 + 依赖闭包，记录实际送入的上下文哈希。
4. 去重：文件级 MinHash，跨 fork 重复只留一份。

## R2 实测（2026-11-23）

**去重**（`repo --repo .`，1827 文件，release 二进制 1.2s）

| 类型 | 组数 | 明细 |
|---|---|---|
| 精确（SHA-256） | 2 | `benchmarks/dot_variants/main.rs` = `.../src/main.rs`；`trit-wspace/examples/adaptive_defense/metrics.rs` = `creative_priming/metrics.rs` |
| 近似（MinHash） | 2 | trit-wspace 三份 `metrics.rs` 相似 0.936；`evolution/metrics.rs` 与 `spam_redteam/metrics.rs` 相似 0.801 |

跳过 304 个文件（测试文件 / <200B / >1MB / 非 UTF-8），跳过数入册不静默丢弃。

**检索上下文**（`repo --context "bm25 context bundle"`，5.7s 含扫描）

```json
{"kind": "trit-data-context", "budget_tokens": 8000, "total_tokens": 3732,
 "bundle_sha256": "ff3387f2…", "files": 10}
```

- 8 个 BM25 命中（`context.rs` / `context_tests.rs` / `repo_context.rs` 排前三，
  另有 docs 计划与 repo_opts.rs），2 个依赖闭包补入（`repo.rs`、`code_quality.rs`）。
- `bundle_sha256` 对入选文件的「路径 + 内容」做内容级指纹，同一查询复算可比对。
- 预算按词级 token 数计，超预算的命中/依赖跳过，不截断文件内容。

## 数据管线（2026-11-23）

```bash
cargo run -q --release -p trit-data --bin trit-data -- pipeline --repo <目录> --out <目录>
```

流程：`repo` 扫描 → 仓库级质量门（`repo_gate`）→ `accepted/deferred/rejected.jsonl`
+ `pipeline.report.json` + `dataset_card.json` + `manifest.json`（内容级清单）。

仓库级门：

| 门 | 类型 | 判据 |
|---|---|---|
| `schema` | reject | path/lang 为空或 bytes=0 |
| `unknown_lang` | reject | 无法识别语言 |
| `generated` | reject | 自动生成标记（可 `--allow-generated` 关掉） |
| `duplicate` | reject | 精确/近似重复组内非首份（可 `--allow-duplicates` 关掉） |
| `license` | reject/defer | `--strict-license` 拒绝；`--require-license` 降级为 defer |
| `imports_unknown` | defer | 语言无 import 语法（markdown/toml 等） |
| `no_test` | defer | `--require-tests` 且无测试关联 |
| `too_large` | defer | >1MB |

实测（本仓库，1831 文件，1.1s）：

```json
{"files": 1831, "decisions": {"accept": 680, "defer": 1133, "reject": 18},
 "reasons": {"duplicate": 5, "generated": 13, "imports_unknown": 331, "no_test": 1128}}
```

Rust 的 `#[cfg(test)]` 内联测试已计入覆盖（识别前 no_test 1435、accept 372；
识别后 no_test 1128、accept 680，`coverage.tests` 0.058 → 0.228）。

- `no_test` 占多数：本仓库大量文件没有同名/同 crate 测试关联（R2 的测试匹配只认约定）。
- `duplicate` 5 与 `repo.dup.json` 的 2 组精确 + 2 组近似一致（组内除首份全部拒绝）。
- 数据卡记录 `sources / licenses / languages / coverage / deps / duplicates`；
  `manifest.hash` 对输出文件集合做内容级指纹，重跑可复算。
- `--repo .` 时数据卡的 source 名是 `.`（basename 为空），用目录名或绝对路径可得到可读名。
