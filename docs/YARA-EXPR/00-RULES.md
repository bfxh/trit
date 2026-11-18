# 00 红线与工作方式

> 开工前读完这一份。它比任何任务卡都优先：任务卡和这里冲突时，以这里为准并停下来报告。

---

## 1. 七条红线

1. **不猜。** 文档和向量里都查不到的语义，让它弃权：求值给 `Unknown`（`Expr::Abstain(原因)` 或不把字段放进 pe 树），解析给 `Unsupported`（`Err(原因)`）。弃权永远是安全的，猜错不是。
2. **`Unknown` 不许变成假。** 只有“另一侧已经决定结果”时它才会消失（`false and 未知`、`true or 未知`）。任何地方出现 `Unknown => false`、`unwrap_or(false)`、`unwrap_or(0)` 这类写法，都要停下来重看。
3. **错判必须是 0。** `tests/yara_expr_vectors.rs` 打印三个数：一致、弃权、错判。错判只要不是 0，就不许提交。
4. **不改向量，不改样本。** `tests/data/yara_expr/` 下的东西是官方引擎的读数。向量和你的代码对不上，错的是代码。
5. **不抬预算台账。** 见第 3 节。
6. **不删测试，不弱化断言。** 要改既有断言只有一种情况：`tasks/T30` 里列出的那几条（官方读数与旧断言矛盾），改的时候在提交说明里写上对应的向量 ID。
7. **不推 `main`，不碰共享工作树，不执行任何样本。**

## 2. 每张任务卡的做法

```
1. 读任务卡，读它指向的规范章节。
2. 先写卡上要求的单测（会失败）。
3. 写实现，直到单测通过。
4. 跑向量回放，看三个数：
     cargo test -p trit-re --test yara_expr_vectors -- --nocapture
   错判必须是 0。把 tests/yara_expr_vectors.rs 里 GAP_CEILING 的数字调低到这次打印的弃权数。
5. 跑全套门禁（第 4 节）。
6. 提交。一张卡一个提交。
```

第 4 步的上限**只许调低**。如果这次的弃权数比上限还高，说明你把原来能算的东西弄坏了。

## 3. 预算台账（最容易卡住的地方）

仓库里每个 `.rs` 文件都有一个行数上限，等于它登记时的行数。`scripts/module_budget.py` 检查的是“每个文件 ≤ 自己的上限”，单文件硬上限 800 行。

**既有文件**（起点提交 `e0f37d8` 里就有的）：上限已经定死，而且全部贴着上限，余量是 0。要往里加一行，就得先从里面拿走一行。

**新文件**：在这个分支合并之前，可以重新登记。每次提交前固定跑这两条：

```bash
git checkout e0f37d8 -- scripts/module_budget_baseline.json
python -I scripts/module_budget.py --update
```

第一条把台账恢复成起点的样子，第二条把所有新文件按当前行数登记进去，并把变短的旧文件收紧。这样新文件在分支内可以增长，旧文件永远不会超过起点的上限。

`--update` 报 `REFUSED … GREW` 时：

| 情况 | 怎么办 |
|---|---|
| 报的是旧文件 | 你让它变长了。把多出来的逻辑搬到新文件里，或者合并几行注释腾位置。**不要手改 JSON** |
| 报的是新文件超过 800 行 | 拆成两个文件 |

接缝补丁已经把需要动的旧文件处理好了，当前行数与上限：

| 文件 | 行数 | 上限 |
|---|---|---|
| `sigscan.rs` | 280 | 280 |
| `sigscan/eval.rs` | 213 | 213 |
| `sigscan/rule.rs` | 87 | 89 |
| `sigscan/scan.rs` | 43 | 101 |
| `yaraparse.rs` | 285 | 286 |

之后只有一处还要动旧文件：`T30` 的切换（旧前端文件整个删除，`yaraparse.rs` 和 `main.rs` 只会变短或持平，卡上给了逐行的改法）。在那之前，**不要碰任何旧文件**。

写新文件时给自己留余地：**每个新文件目标 400 行以内**。

## 4. 门禁

每次提交前全部跑一遍，全部通过才提交：

```bash
cargo fmt --all -- --check
cargo clippy -p trit-re --all-targets --locked -- -D warnings
cargo test -p trit-re --locked
git checkout e0f37d8 -- scripts/module_budget_baseline.json
python -I scripts/module_budget.py --update
python -I scripts/module_budget.py
python -I scripts/hazard_gate.py
python -I scripts/secret_scan.py
python -I scripts/docs_hygiene.py
python -I scripts/test_surface.py
```

常见的失败：

| 现象 | 原因与处理 |
|---|---|
| clippy 报 `dead_code` | 你写了还没被用到的函数。要么接上它，要么这张卡先别写它。接缝补丁里有三处带说明的 `#[allow(dead_code)]`，卡上写了什么时候删 |
| clippy 报 `too_many_arguments` | 把参数收进一个结构体。实在拆不开再加 `#[allow(clippy::too_many_arguments)]`（仓库里有先例） |
| `test_surface` 报“无断言测试” | 每个 `#[test]` 里至少要有一个 `assert` |
| `hazard_gate` 报警 | 你用了 `unsafe`、`transmute`、`static mut` 之类。这个任务不需要它们，删掉 |
| `fmt --check` 失败 | 跑 `cargo fmt --all` |

## 5. 代码上的约定

- 产品代码里不用 `unwrap()`、`expect()`、`panic!`、直接下标 `a[i]`（`i` 来自输入时）。用 `get`、`?`、`let … else`。测试代码里随意。
- 整数运算：来自输入的数一律用 `checked_*` 或 `wrapping_*`，按规范写的是哪种就用哪种。
- 注释和文档用中文，写“为什么”，不写“做了什么”。引用官方行为时带上向量 ID。
- 错误原因（`Err(String)`）写清楚是什么不支持，带上出问题的 token。这些原因会原样出现在 CLI 的 `UNSUPPORTED` 行里。
- `CHANGELOG.md` 只在 `## [Unreleased]` 之后追加，不改动相邻条目。它在最后一张文档卡里统一写，前面的卡不动它。

## 6. 卡住了怎么办

| 情况 | 做法 |
|---|---|
| 某条向量始终是“错判” | 用 `python3 tools/cite.py find '<条件>'` 找同类向量，对照 `01-SEMANTICS.md` 的对应小节。还是不明白，就让这类表达式弃权（解析期 `Err`），在提交说明里写清楚哪些 ID 因此是弃权 |
| 规范里两处说法矛盾 | 以向量为准。没有向量覆盖 → 弃权，并报告 |
| 旧文件无论如何腾不出行数 | 停下来报告，不要抬台账 |
| pe 的某个字段在某个样本上对不上 | 见 `03-PE-MODEL.md` 第 10 节：那种情况下不把字段放进树 |
| 需要一个规范没提到的新公开接口 | 停下来报告 |

“报告”的意思是：在提交说明或 PR 描述里写明卡在哪、试过什么、现在的状态是什么。不要用一个看起来能跑的猜测盖过去。

## 7. 词汇表

| 词 | 意思 |
|---|---|
| Undef | 官方的 undefined，确定没有值 |
| Unknown | 证据缺失，trit 的弃权 |
| Unsupported | 这条规则解析不了或官方编译不过，带原因；扫描时不出裁决 |
| 弃权 / 缺口 | 这边给了 Unknown 或 Unsupported，而官方有裁决。不算错，但要登记原因 |
| 错判 | 这边给了与官方相反的裁决，或官方编译不过而这边给了命中 / 不命中 |
| 向量 | `tests/data/yara_expr/*.tsv` 里的一行，一条冻结的官方读数 |
| 接缝 | `ref/0001-seam.patch`：新旧两套前端并存的那个起点 |
