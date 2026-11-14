# 分组三值的判等证书（设计稿）

> 状态：设计已定，实现见 `crates/trit-wspace/src/grouped_certify.rs`
> 与 `tests/grouped_certify.rs`。本文先钉死判据，再谈实现——判据错了，实现再快也是错的。

## 1. 问题：分组把「scale 自由度」从行上打散了

`Layer` 每行一个正 scale，因此有两条免费的不变量：

1. **正 scale 不改行为**：整行乘正数，符号输出不变 ⇒ 判等可以忽略 scale；
2. **隐藏单元置换**：交换两行 + 对应列，行为不变。

`GroupedLayer` 按**跨行**分组（`docs/WSP-SUMMARY.md` §7.1），同一行内不同列可以落到不同
scale。于是：

- 单独缩放某一组会改变行内各项的相对权重 ⇒ **符号输出会变**；
- `Layer` 的「忽略 scale」判等在分组结构下**直接失效**；
- 现有兜底只有 `row_proportional()`（最粗的充分条件），没有分级、没有证据字段，
  也没有和 `certify` 的「可复核证书」口径对齐。

## 2. 判据分级（L0–L4）：每一级都是可复核的离散证据

| 级 | 判据 | 证据字段 | 强度 | 成本 |
|---|---|---|---|---|
| **L0** | 逐位相同（trits / groups / scales / bias_f32） | `witness: "identical"` | 同一工件 | `O(rows×cols)` |
| **L1** | canonical 相等（全体 scale 同除 `scales[0]`） | `scale_ratio` | 等价（全体正缩放不改行为） | `O(rows×cols)` |
| **L2** | 逐行正比例 `B[r,:] = k_r·A[r,:]`，`k_r > 0` | `row_ratios[]` | **充分**（⇒ 行为相同） | `O(rows×cols)` |
| **L3** | 穷举行为签名相同（`3^vars` 个输入，逐行符号） | `vars`、`samples`、`rows`、`signature_fnv` | **严格**（有界域上完全判定） | `O(3^vars × rows × cols)` |
| **L4** | 以上都不成立 | `witness: "inconclusive"` | **不是**「行为不同」 | — |

另有一级不在 L0–L4 里：**形状不同** → `witness: "shape_mismatch"`。定义域不同（行/列不一致）
⇒ **确定不等价**，不需要穷举，也不该混进 `inconclusive`。

**L4 的措辞是硬要求**：穷举有域上限（`MAX_VARS = 8`），超域时无法判定。证书只能写
`inconclusive`，不许写 `different`——后者是把「没证明出来」冒充成「已否证」，
与本仓 `docs/STATUS.md` 的口径冲突。

## 3. 每级的证明

### L1：全体正缩放

若存在 `a > 0` 使 `B = a·A`（逐元素，含 trits、groups 与 scale 的对应关系），则对任意 `x`：
`y_B[r] = Σ_c a·A[r,c]·x[c] = a·y_A[r]`，而 `sign(a·y) = sign(y)`（`a > 0`）。
所以行为签名逐位相同。**`canonical()` 就是把 `a` 除掉**，因此 canonical 相等 ⇔ L1。

### L2：逐行正比例

若 `B[r,:] = k_r·A[r,:]`（`k_r > 0`），则 `y_B[r] = k_r·y_A[r]`，逐行符号相同 ⇒ 行为相同。
这是 `Layer` 的「改某行 scale」在分组结构下的推广：分组只是让 `k_r` **一般不等于全体同一值**。

### L3：穷举签名

在有界域（`vars ≤ 8`，`3^8 = 6561` 个 trit 输入）上逐输入、逐行比较符号。
域内没有抽样，因此这是**判定**而不是检验。

### 反例（为什么 L1/L2 不能反过来）

`docs/WSP-SUMMARY.md` §7.1 的最小反例：trits `[1,1,1,-1]`、scales `[0.1, 10.0]`、输入 `[1,1]`：

- per-row 分组 `[0,0,1,1]`：第 1 行 = `10 − 10 = 0`（符号 `0`）
- 跨行分组 `[0,1,0,1]`：第 1 行 = `0.1 − 10 = −9.9`（符号 `−`）

两层 `trits` 相同、只有组号不同 ⇒ **L1/L2 都不成立，但 L3 会判「不同」**。这说明：
分组结构下「同 trits」不代表同行为，判等必须带上 groups 与 scales。

## 4. 与 `certify` 的口径对齐

两套证书回答的是不同问题，**不许混用**：

| | `certify`（`src/certify.rs`） | 本文的 `GroupedCertificate` |
|---|---|---|
| 对象 | 编辑前 / 编辑后两个 `Net` + 保持集 `Oracle` | 两个 `GroupedLayer`（同一层，编辑前后） |
| 问题 | 「保持集上有没有改坏」 | 「两个工件是不是同一个行为函数」 |
| 证据 | `Preserved / Fixed / Regressed / Drifted / Stale` 计数 + 明细 | L0–L4 判据 + 比例/签名证据 |
| 范围 | **样本集**（`scope: "sample_set"`） | **结构**（`scope: "structural"`） |
| 强度 | 只对这批样本成立 | L3 成立 ⇒ **任意**输入成立 |

推论（写进证书的 `implies` 字段，取值只有三种）：

- `all_inputs`（L0–L2）：结构证明，**任意实数输入**行为相同 ⇒ 保持集必然全 `Preserved`；
- `bounded_trit_domain`（L3）：只覆盖有界 trit 域（`vars ≤ 8`，其余维为 0），域外没有保证；
- `none`（L4 / 形状不同）：什么都没证明。

## 5. 实现落点（受模块台账约束）

仓库的 `scripts/module_budget.py` 是**全量棘轮**：每个 `.rs` 文件都顶着自己的行数上限，
台账只许收紧、不许抬高，且当前全仓 `slack = 0`。因此新增代码只能：

- 新建文件（父模块声明要腾行），或
- 在同文件内**先删等量行再加**。

本次采用前者，并把两份判等实现合并成一个模块：

| 动作 | 效果 |
|---|---|
| 删除 `src/grouped_equal.rs`、`src/grouped_invariant.rs` | `lib.rs` 少两个 `pub mod` |
| 新建 `src/grouped_certify.rs` | `lib.rs` 加一个 `pub mod` |
| 净变化 | **−1 行**，不触碰任何上限 |

迁移的方法（`behavior` / `equivalent` / `canonical` / `is_row_grouped` /
`row_proportional` / `globally_proportional`）行为不变；`MAX_VARS` 的公开路径从
`grouped_equal::MAX_VARS` 改为 `grouped_certify::MAX_VARS`。

## 6. 验收判据

**正例（每条都要有）**

1. L0：同一层 ⇒ `witness = "identical"`。
2. L1：把全体 scale 乘 `3.0`（trits/groups 不变）⇒ `witness = "canonical"`，`scale_ratio = 3.0`。
3. L2：把每行整体乘不同的正数（构造每行自成一组）⇒ `witness = "row_proportional"`，
   `row_ratios` 与注入值一致。
4. L3：`trits` 逐位取反 + 全体 scale 取正（行为不变）⇒ `witness = "behavior"`，
   且 `signature_fnv` 相同。

**反例（必须落在正确的位置）**

5. 反例层（第 3 节的 `[1,1,1,-1]`）⇒ `witness = "inconclusive"`，**不是** `"different"`。
6. `vars = 9` ⇒ 报 `DomainTooLarge`，不静默降级。

**性质测试（充分性自检）**

7. 随机生成 100 对层，凡是 L2 成立（`row_proportional().is_some()`），L3 **必须**成立
   （`behavior(vars)` 相等）。这是「充分条件」这个词的实证，不是口头承诺。

**成本**

8. 384×384、`vars = 4`：`3^4 = 81` 次前向 ⇒ 约 `81 × 147456 ≈ 11.9M` 次乘加，
   release 实测应 < 1 s（写进测试的超时断言只做上限，不做精确计时）。

## 7. 边界（没做的部分）

- **域外无保证**：`vars > 8` 只能 `inconclusive`；不做 SAT/BDD 编码（那是 P2-15，
  见 `docs/WSP-RESEARCH-ROADMAP.md`）。
- **不判「不同结构但行为相同」**：L3 能判，但成本随 `vars` 指数；L0–L2 只判「结构上可
  证明相同」。
- **bias 不参与 scale 自由度**：L1/L2 要求 `bias_f32` 逐位相同。分组层的 bias 是每行一个
  f32，没有「分组 scale」这层自由度，所以不把它折进比例。
- **容差只用于报告**：L1/L2 的比例比较用 `1e-6` 判定「同一个比例」，这个常数写进证书的
  `tolerance` 字段，复核者可以自己重算并采用更严的阈值；判定本身不是浮点近似。
- **不做多层的跨层证书**：本证书只覆盖单层；多层组合需要 `Net` 级（见 `certify`）。
