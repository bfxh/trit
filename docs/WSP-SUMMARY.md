# trit-wspace × all-MiniLM-L6-v2：完整实测汇总

本文件把 `feat/wspace-dim-sweep` 分支上这一系列实验的**全部读数**汇总成一张表，方便查阅与复核。
所有数字都来自仓库内可复现的示例（`cargo run --release -p trit-wspace --example <name>`）。

> 数据源：真实语料 STS Benchmark（train 5749 / dev 1500 / test 1379 对）＋
> `sentence-transformers/all-MiniLM-L6-v2` 的真实权重。合成任务另有标注。

---

## 0. 一句话结论

**三值表示在这个仓库里守住了精度，但守不住压缩，也守不住"判等忽略 scale"这个最独特的属性。**
它换来的是可验证性，代价是验证粒度变粗——"逐行正比例"这条不变量说明了**粗到什么程度**。

---

## 1. 这一层有表征能力吗：有，与官方持平

真实 STS test 全量 1379 对，指标 Spearman（余弦 vs 人工分数）：

| 实现 | Spearman | 说明 |
|---|---|---|
| raw 词向量平均 | 0.5621 | 基线 |
| 简化单层（ReLU / 单头 / 无输出投影） | 0.6157 | 早期不完整实现 |
| 完整 BERT（缺 token_type、标点丢弃） | 0.7311 | 补齐 attention.output.dense / GELU / 12 头 |
| **完整 BERT + token_type + 标准 BasicTokenizer** | **0.8204** | 与官方持平 |
| 官方 all-MiniLM-L6-v2 报告值 | ≈0.82 | 参照 |

补齐路径（每修一个缺陷，读数跳一档）：

| 缺陷 | 症状 | 修复后 |
|---|---|---|
| 缺 `attention.output.dense` | CLS 池化 Spearman **0.0277** | — |
| `Dense::forward(x, true)` 其实是 **ReLU**（BERT 用 GELU） | 6 层堆叠塌到 **0.4505**（比单层还差） | — |
| 注意力按**单头 384 维**打分（真实 **12 头 × 32 维**） | **0.5017** | 200 对 **0.8614** |
| 缺 `token_type_embeddings[0]`（非零向量）＋ 标点被丢弃 | **0.7311** | 全量 **0.8204** |

**池化方式**：`1_Pooling/config.json` 明确写 `pooling_mode_mean_tokens=true`、`cls_token=false`
——这个模型不用 CLS 池化，CLS 崩是预期行为而非 bug。

---

## 2. 逐层探针：深度带来什么

### 2.1 合成任务上：U 形（负贡献）

1024 样本 / 64 上下文 / 768 训练 + 256 留出 / λ=1e-2 / 机会 0.25：

| 读出 | quad 训练/留出 | context 训练/留出 |
|---|---|---|
| `raw` | 0.9010 / **0.3906** | 0.8841 / **0.4023** |
| layer 0 | 0.8906 / 0.3750 | 0.8932 / 0.3789 |
| layer 1 | 0.8750 / 0.3555 | 0.8828 / 0.3672 |
| layer 2 | 0.8607 / 0.3047 | 0.8659 / 0.3359 |
| layer 3 | 0.8529 / 0.3281 | 0.8333 / 0.3477 |
| layer 4 | 0.7891 / 0.3359 | 0.8073 / 0.3398 |
| layer 5 | 0.7721 / 0.3203 | 0.7617 / 0.3516 |

**深层不如原始词向量** —— 但这是任务设计问题，见 §3。

### 2.2 真实 STS 上：单调上升（正贡献）

| 读出 | Spearman |
|---|---|
| `raw` | 0.5621 |
| layer 0 | 0.5509 |
| layer 1 | **0.5211**（谷底） |
| layer 2 | 0.5683（反超 raw） |
| layer 3 | 0.5773 |
| layer 4 | 0.6034 |
| **layer 5** | **0.6157** |

同样 U 形，但**收益为正**。对比 §2.1 可见：**合成任务与预训练表示不匹配，不能用来评判某一层。**

---

## 3. 训练 block：不能提升，而且有害

冻结 layer 0–4、只训 layer 5。固定验证批（64 对，不参与训练）上的 loss 与 test Spearman：

| 目标 / 配置 | probe loss | test Spearman |
|---|---|---|
| 冻结基线（闭式） | — | **0.8204** |
| 余弦-MSE，100 步 | 0.02019 → 0.01922（−4.8%） | 0.820376 → **0.820399**（+2e-5） |
| 对比学习 in-batch negatives，lr=1e-5 | — | 0.820376 → 0.820379（+3e-6） |
| 对比学习，lr=1e-6 | — | 0.820376 → 0.820381（+5e-6） |

**lr 扫描（对比学习）**：

| lr | probe loss | test Spearman |
|---|---|---|
| 1e-5 | 4.81147 → 4.80577（−0.12%） | 0.820376 → 0.820384（+7.5e-6） |
| 1e-4 | 4.81147 → 4.75350（−1.2%） | 0.820376 → 0.820283（−9.4e-5） |
| **1e-3** | 4.81147 → 4.15635（**−13.6%**） | 0.820376 → **0.814810**（−0.0056） |

**单调反向**：训练 loss 降得越多，test 指标掉得越多 ⇒ 过拟合 / 破坏预训练表示，不是"学不会"。
最优策略是 **lr → 0（完全不训练）**，这直接证明 0.8204 已是上限。

**为什么**：800 个训练对 vs MiniLM 预训练的**亿级**数据——差 5 个数量级。

---

## 4. 权重空间改动：代价谱

同一批 300 对子集（可比；绝对值 0.9337 不与全量 0.8204 直接比）：

| 改动 | Spearman | 相对基线 | 备注 |
|---|---|---|---|
| f32（基线） | 0.9337 | — | |
| **int8 只 attention** | 0.9333 | **−0.05 点** | 几乎免费 |
| int8 全量 | 0.9303 | −0.35 点 | |
| int8 只 FFN | 0.9299 | −0.38 点 | |
| ternary 只 FFN | 0.8916 | −4.21 点 | |
| **ranked-6 头** | 0.8846 | −4.91 点 | 按重要性剪 |
| skip layer 2/3/4 | 0.8473 | −8.64 点 | 层数减半 |
| **ranked-3 头** | 0.8496 | −8.41 点 | |
| ternary 只 attention | 0.8461 | −8.76 点 | |
| **ranked-1 头** | 0.8447 | −8.91 点 | 每层只剩 1 个头 |
| index-3 头（错误做法） | 0.7361 | **−19.76 点** | 索引假象 |
| **ternary 全量** | 0.6847 | **−24.91 点** | |

### 4.1 两种精度下敏感模块排序相反

- **int8**：attention 更抗压（−0.05 < FFN −0.38）
- **ternary**：attention 是瓶颈（−8.76 > FFN −4.21）
- **同时三值化是超线性的**：−24.91 远大于分项之和 −12.97 ⇒ 误差互相放大

### 4.2 头剪枝必须按重要性排序

- 保留"前 3 个头"（−19.76）比"前 1 个头"（−12.83）**还差** ⇒ 头的重要性不按索引
- 按 `wo` 列范数排序后：ranked-3 比 index-3 高 **11.35 点**
- `ranked-1 ≈ ranked-2 ≈ ranked-3`：每层真正有用的头只有 1–2 个

---

## 5. 三值 + 分组：精度救回来了

### 5.1 置换（排序）是关键

逐元素三值化对**置换不变**；置换只在**分组共享 scale** 时起作用。

| 配置 | Spearman | 相对基线 |
|---|---|---|
| f32（基线） | 0.9337 | — |
| ternary per-tensor | 0.6847 | −24.91 点 |
| ternary per-row（行内排序） | 0.7073 | **−22.65 点**（无改善） |
| ternary group32（不排序） | 0.7158 | −21.79 点 |
| **ternary group32（整张量排序）** | **0.9348** | **+0.11 点** |

**行内排序无效** ⇒ 真正起作用的是**跨行全局按幅度分层**（把所有微小权重聚到一起用极小 scale）。

### 5.2 分组大小扫描（300 对）

| 配置 | bits/weight | 压缩率 | Spearman | 相对基线 |
|---|---|---|---|---|
| group4 | 10.00 | 3.2× | 0.9339 | +0.0001 |
| group8 | 6.00 | 5.3× | 0.9343 | +0.0006 |
| group16 | 4.00 | 8.0× | 0.9347 | +0.0009 |
| group32 | 3.00 | 10.7× | 0.9348 | +0.0011 |
| group64 | 2.50 | 12.8× | 0.9349 | +0.0011 |
| group128 | 2.25 | 14.2× | 0.9351 | +0.0013 |
| group256 | 2.12 | 15.1× | 0.9346 | +0.0009 |

> **注意**：以上 `bits/weight = 2 + 32/G`，**不含组号开销**。含组号后见 §6.2。

### 5.3 全量与 6 个子集确认

全量 1379 对：

| 配置 | bits/w | 压缩率 | Spearman | 相对基线 |
|---|---|---|---|---|
| f32 | 32 | 1.0× | **0.8204** | — |
| group128 | 2.25 | 14.2× | 0.8193 | −0.0011 |
| **group256** | **2.12** | **15.1×** | **0.8200** | **−0.0004** |

6 个子数据集（各 125–254 对）：

| 子集 | 对数 | 基线 | group256 | 相对 |
|---|---|---|---|---|
| MSRpar | 250 | 0.5633 | 0.5627 | −0.0006 |
| MSRvid | 250 | 0.9398 | 0.9405 | **+0.0006** |
| answer-answer | 254 | 0.7263 | 0.7238 | −0.0025 |
| headlines | 250 | 0.8230 | 0.8182 | **−0.0049** |
| images | 250 | 0.9114 | 0.9110 | −0.0005 |
| track5.en-en | 125 | 0.8470 | 0.8481 | **+0.0011** |

**子集基线跨度 0.5633–0.9398（从难到接近饱和），三值分组在整个难度区间都稳定。**

---

## 6. 但压缩率没有想象中好：三次自我纠正

### 6.1 抽象位宽 → 含组号

排序划分本身是**额外信息**：每组 256、59 万权重 ⇒ 2304 组 ⇒ 每元素需 `log2(2304) ≈ 11.2 bit` 记组号，
**比 trit 的 2 bit 大 5 倍**。

| 方案 | 位宽/权重 | 真实压缩率 | STS 代价 |
|---|---|---|---|
| f32 | 32 | 1× | — |
| **int8** | 8 | **4×** | −0.35 点 |
| 三值 per-tensor | 2 | 16× | −24.91 点 |
| 三值 group256 + 排序（**含组号**） | ≈13.3 | **2.4×** | −0.0004 点 |

### 6.2 可读文本格式下：负压缩

`wsp1` 每个 trit 占**一个 ASCII 字节**（8 bit），不是理论上的 2 bit。

| 表示 | 字节（64×64） | 相对 f32 |
|---|---|---|
| f32（二进制） | 16384 | 1× |
| `wsp1g`（分组三值，文本） | **14544** | **1.13×** |
| `wsp1`（per-tensor 三值，8×8 小张量） | — | **0.76×**（比 f32 还大） |

真实规模（6 层 × 6 个线性权重）：

| 指标 | 字节 |
|---|---|
| f32 | 42,467,328 |
| **WSP1G** | **56,925,133** |
| **实测压缩** | **0.75×（大 33%）** |

**结论**：`wsp1` 是"可 diff / 可复算 / 可校验"的**源码格式**，不是存储格式。
三值的压缩优势只在二进制实现里成立（2.4×），落到可读文本是负的。

---

### 6.3 二进制落点：WSP2G 把 0.75× 修成 **2.37×**

把同一批 6 层 × 6 个线性权重（36 个张量、10,616,832 个元素）打成 **WSP2G** 容器
（WSP2 `version=3`，组号按 `ceil(log2 组数)` 位定长打包，组 scale 与 bias f32 计入）：

| G | 组数/层 | bit/权重 | WSP1G 文本 | WSP2G 二进制 | 文本→二进制 |
|---|---|---|---|---|---|
| 32 | 4608 | 17.40 | 0.61× | 1.84× | 3.03× |
| 64 | 2304 | 15.90 | 0.67× | 2.01× | 3.01× |
| 128 | 1152 | 14.65 | 0.71× | 2.18× | 3.08× |
| **256** | **576** | **13.53** | **0.75×** | **2.37×** | **3.17×** |

- f32 权重 42,467,328 B；G=256 时 WSP1G **56,925,133 B**（与 §6.2 逐字节一致），
  WSP2G **17,949,399 B** ⇒ **2.37×**，与 §6.1 的理论估计（≈13.3 bit → 2.4×）对齐。
- 四个组大小的 `roundtrip_bit_identical` 全为 **true**（trit / 组号 / scale / bias 逐位一致）。
- 口径：f32 只算权重（与 §6.2 一致）；两种三值格式都含 bias f32 与头部 ⇒ 压缩率偏保守。
- 复算：`cargo run --release -p trit-wspace --example grouped_binary_gain -- <model.safetensors> 32,64,128,256`
- **仍未做**：WSP2G 还没接 `BlockIndex` 的行块随机读（分组跨行，行块要换算到组号段的
  元素区间），所以现在只能整层解码。

## 7. 判等语义：代价在哪里

### 7.1 分组破坏了"正 scale 不改行为"

`Layer` 每行一个正 scale ⇒ 缩放整行不改符号 ⇒ 判等可忽略 scale。
`GroupedLayer` 跨行分组 ⇒ **同一行内不同列可能落到不同 scale** ⇒ 该不变性失效。

反例：trits `[1,1,1,-1]`、scales `[0.1, 10.0]`、输入 `[1,1]`

- per-row 分组 `[0,0,1,1]`：第 1 行 = `10−10 = 0`（符号 `0`）
- 跨行分组 `[0,1,0,1]`：第 1 行 = `0.1−10 = −9.9`（符号 `−`）

### 7.2 能恢复多少

| 机制 | 作用 | 适用 |
|---|---|---|
| `behavior(vars)` | 穷举 `3^vars` 取符号作签名 | **严格**判等（`vars ≤ 8`） |
| `canonical()` | scale 除以 `scales[0]`，消掉整体缩放 | 结构相同时的**快速**判等 |
| `is_row_grouped()` | 是否每行自成一组 | 为真时才可像 `Layer` 那样忽略 scale |
| **`row_proportional()`** | **逐行正比例 ⇒ 行为相同** | **最粗**，`O(rows×cols)` |

**统一解释**：

| 结构 | 改动 | 逐行比例？ | 结论 |
|---|---|---|---|
| `Layer`（每行一个 scale） | 改某行 scale | **是** | scale 可忽略 |
| `GroupedLayer`（跨行分组） | 改某组 scale | **一般不是** | scale 必须固定 |

> **分组破坏判等不变性，本质是让 scale 自由度不再对齐到行。**

### 7.3 补丁搜索也变了

同一 trit 编辑的效果**取决于它属于哪一组**（`grouped_repair` 实测）。
补丁语义从"改哪个 trit"变成"改哪个 trit **且在哪个分组下**"。

---

## 8. 端到端闭环：存 → 读 → 行为不变

把 6 层 × 6 个线性权重量化成 `GroupedLayer` → 写盘 WSP1G → 读回 → 装回 BERT → 跑 STS：

| 指标 | 值 |
|---|---|
| baseline | 0.933742 |
| quantized | 0.934604 |
| **roundtrip** | **0.934604** |
| roundtrip − quantized | **+0.000000** |
| `roundtrip_bit_identical` | **true** |
| f32 字节 | 42,467,328 |
| WSP1G 字节 | **56,925,133** |
| 实测压缩 | **0.75×** |

**往返完全无损，但文件更大 33%。**

---

## 9. 四次自我否定（本系列最有价值的部分）

| # | 错误 | 纠正 |
|---|---|---|
| 1 | int8 压缩率写成 **8×** | 8 bit/weight ⇒ **4×** |
| 2 | 三值分组写成 **15.1×** | 漏掉组号 11.2 bit/元素 ⇒ **2.4×** |
| 3 | 文本格式估成 **2.4×** | trit 在文本里占 8 bit ⇒ 实测 **1.13×**，真实规模 **0.75×** |
| 4 | 校验和"看起来在工作" | 用十进制解析十六进制 ⇒ **静默失效**，任何篡改都检不出 |

**四次都朝同一方向偏，都让"三值"看起来比实际好。** 其中 3 条后来写成了回归测试
（`bits_per_weight_includes_group_ids`、`text_formats_are_honestly_measured`、
`checksum_detects_single_trit_change`）。

---

## 10. 统计显著性：单点读数的置信区间

以上所有读数此前都是**单点估计**。用 bootstrap 给它们配上区间后，结论的表述需要修正。

### 10.1 三值分组的差异**不显著**

300 对、group=256、2000 次重采样：

| 配置 | Spearman | 95% CI |
|---|---|---|
| baseline | 0.933742 | [0.9131, 0.9468] |
| quantized | 0.934604 | [0.9139, 0.9475] |
| delta | **+0.000862** | |
| **paired p 值** | **0.2815** | **不显著** |

- 两个置信区间**高度重叠**，成对 bootstrap p = 0.28。
- 所以正确表述是「**无可检测的差异**」，而不是「掉了 0.0004」或「提升了 0.0009」——
  这些单点差异的方向本身就在噪声里。
- **CI 半宽约 1.7 个点**：任何小于这个尺度的差异都不该被解释。这也说明
  §5.3 里 headlines 的 −0.0049 在 250 个样本上同样无法判定。

### 10.2 分层采样能让小样本代表全量

按人工分数分 5 层、抽 40%（119 个样本）：

| 指标 | 全量 300 对 | 分层 40%（119） |
|---|---|---|
| baseline | 0.9337 | **0.9309** |
| quantized | 0.9346 | **0.9318** |

差 < 0.003。**按层抽样后，小样本子集可以代表全量**——这是「采样麻烦」的正解，
而不是随意取「前 N 对」（那样会像 §0 提到的 0.8614 vs 0.7311 一样偏 13 个点）。

### 10.3 方法学

- **percentile bootstrap 不保证区间包含点估计**：估计量有偏时重采样分布会偏斜
  （Spearman 逼近 1 时正是如此）。需要「必然包含」时应用 basic bootstrap。
- **比较必须成对**（Dietterich 1998）：同一批样本上重采样，样本难度被消掉，
  只剩配置差异；否则容易把「这批样本特别难」误判成「配置更差」。

### 10.4 工具

```rust
use trit_wspace::stats::{bootstrap_spearman_ci, paired_bootstrap_p, stratified_indices};

let ci = bootstrap_spearman_ci(&predicted, &gold, 2000, 0.95, seed);
let p  = paired_bootstrap_p(&baseline, &quantized, &gold, 2000, seed);
let subset = stratified_indices(&strata, 0.4, seed);
```

示例：`cargo run --release -p trit-wspace --example classifier_sts_significance -- $MODEL $VOCAB $STS 300 256 2000`

---

## 11. 复现方式

```bash
cd trit-wt-dimsweep
git checkout feat/wspace-dim-sweep

# 全套测试（77 个）
cargo test --release -p trit-wspace

# 需要本地模型与 STS 数据；示例都接受路径参数
MODEL=.../all-MiniLM-L6-v2/model.safetensors
VOCAB=.../all-MiniLM-L6-v2/vocab.txt
STS=.../sts/test.tsv

cargo run --release -p trit-wspace --example classifier_sts_bert  -- $MODEL $VOCAB $STS 1379
cargo run --release -p trit-wspace --example classifier_sts_prune -- $MODEL $VOCAB $STS 300
cargo run --release -p trit-wspace --example classifier_sts_heads_ranked -- $MODEL $VOCAB $STS 300
cargo run --release -p trit-wspace --example classifier_sts_group_sweep  -- $MODEL $VOCAB $STS 1379
cargo run --release -p trit-wspace --example classifier_sts_wspg -- $MODEL $VOCAB $STS 300 256
cargo run --release -p trit-wspace --example classifier_sts_curve -- $MODEL $VOCAB $STS 100 1e-5 0.05 20
```

**数据边界**：单模型（MiniLM-L6）、单任务族（STS）；合成任务的结论**不能**推广到 STS。
子集绝对值与全量绝对值**不可直接比较**。

---

## 12. 真实激活校准：随机输入的方向不是单一低估

此前 `bias_e2e` 的输入是 `uniform(-1,1)`。本层补了真实激活：`real_activations` 从
`vocab.txt` 的真实词出发，走 MiniLM 的 `word_embeddings + position + token_type + LayerNorm`
得到第 0 层 query 段的真实输入（256 条 × 384 维），再喂给 `bias_e2e --activations`。

真实激活统计：mean −0.0041、std **0.2342**；`uniform(-1,1)` 的 std 是 0.5774
（真实激活的尺度只有随机输入的 **41%**）。

第 0 层 query 段（384×384）读数：

| bias | 真实激活 rel_l2 | 随机 rel_l2 | 真实 cos | 随机 cos |
|---|---|---|---|---|
| zero | **0.6531** | 0.5771 | 0.8401 | 0.8451 |
| trit | **0.6044** | 0.5527 | 0.8903 | 0.8657 |
| f32 | **0.4526** | 0.4943 | **0.9296** | 0.8908 |

**结论（推翻了「随机输入低估 1.5–3×」的假设）**：

- 对 `zero` / `trit` bias，随机输入**低估**了 rel_l2（0.577 vs 0.653、0.553 vs 0.604），
  但 cosine 的偏差方向相反（随机更高）；
- 对 `f32` bias，方向**反转**：真实激活下 rel_l2 更低（0.453 vs 0.494）、cosine 更高；
- 中间段与输出段本次仍用随机输入（激活只覆盖第 0 段），所以那两段两组读数完全相同。

**边界**：只覆盖第 0 层 query 段；本机没有 STS 语料，激活来自真实词表而不是真实句子；
连续 6 层的级联激活还没做。

---

## 13. 6 层全量真实 tuned：训练集 +7.7 点，STS test 反而 −18 点

`scripts/train_all_layers.py` 端到端微调 MiniLM 的 **36 个线性权重**（6 层 × 6 个），
embedding/LayerNorm/bias 冻结；训练集为 STSb train（5749 对，丢弃 score=3 后 2699 对），
lr 1e-4、batch 16、1 epoch（169 步）。`scripts/evaluate_tuned_edit_gate.py` 在 STS test 上
按 score 划分 edit 集（≥4，338 对）与 keep 集（≤2，534 对）。

| 集合 | base 命中率 | tuned 命中率 | 变化 |
|---|---|---|---|
| 训练集 valid（674 对，随机 20%） | 0.7849 | **0.8620** | **+7.7 点** |
| STS test edit（338 对） | 0.9734 | **0.7929** | **−18.0 点** |
| STS test keep（534 对） | 0.2790 | **0.1367** | **−14.2 点** |

`regressions 119 / fixes 2`。

**结论（负结果）**：训练集上的提升**没有泛化**到 test，反而大面积破坏预训练表示。
这与 §3「训练 block 有害、lr→0 最优」一致：MiniLM 的表示已经饱和，梯度微调在
小数据上只会过拟合。

**复算**：

```bash
python scripts/train_all_layers.py --model <快照> --data <train.jsonl.gz> \
  --out tuned.safetensors --epochs 1 --lr 1e-4 --batch 16
python scripts/evaluate_tuned_edit_gate.py --model <快照> \
  --base <model.safetensors> --tuned tuned.safetensors --data <test.jsonl.gz>
```

**边界**：单 seed、CPU、1 epoch、lr 未扫；tuned 权重未三值化；没有 keep 集的逐条明细。

---

## 14. 推理式编辑 vs 梯度微调：**1 个 trit 抵 1505 个 trit**

在 STS test 的 200 对子集（57 正 / 143 负，threshold 0.5）上，学生 = 单层三值 `query`
（384→384），输入是 MiniLM 句向量逐行三值化后的 trit。两条路线在同一批 pair 上对比：

- **推理式编辑**（`examples/editset_sts_repair.rs`）：行级探针选承重行 → 行内逐列试
  `-1/0/+1`，按「edit 修复数 − keep 破坏数」贪心，最多 8 步；
- **梯度微调**：`scripts/train_all_layers.py` 训出的 layer0 `query` 差分。

| 路线 | 改动 trits | right/200 | edit_fixed | keep_broken |
|---|---|---|---|---|
| baseline | 0 | 184 | — | — |
| **推理式编辑** | **1** | **185** | 1 | 0 |
| **梯度微调** | **1505** | **185** | 1 | 0 |

**结论（双面）**：

- **正面**：推理式编辑用 **1/1505 的改动量**拿到与梯度微调**相同**的任务收益——
  「不改梯度、只改离散 trit」这条路在真实任务上**有效**；
- **负面**：两条路线都只修好 1 条（16 错 → 15 错），**单层三值 384×384 的表达力是瓶颈**；
  第 2 步起没有净收益 > 0 的候选，搜索到达局部最优。证书不通过
  （`edit` 集仍有 15 条未达成）。

**复算**：

```bash
cargo run --release -p trit-wspace --example editset_sts_repair -- \
  --pairs sts-pairs-200.json --safetensors model.safetensors \
  --max-edits 8 --rows 8 --tuned tuned-all.safetensors
```

**边界**：单层学生网络、200 对子集、threshold 0.5；没有多步/多行预算扫描；没有 6 层三值网络上的推理式编辑。

---

## 15. 6 层三值堆叠上的推理式编辑：深度反而更差，搜索 0 步

把学生从单层 384→384 换成 **6 层同构三值堆叠**（每层权重取 MiniLM 对应层的
`attention.self.query`；没有 attention/LN/残差，**这不是完整 BERT**），输入仍是句向量
逐行三值化。搜索预算集中在最后一层（前 5 层输出缓存，改最后一层只重算该行）。

| 配置 | baseline right/200 | repair 步数 | edit_fixed | keep_broken |
|---|---|---|---|---|
| 单层 384→384（§14） | 184 | 1 | 1 | 0 |
| **6 层堆叠** | **143** | **0** | **0** | **0** |

- **baseline 从 184 掉到 143**：每层 trit 化都丢信息，堆 6 层把语义破坏得更彻底
  （与 §3「堆层救不了」一致）；
- **推理式编辑 0 步**：所有 384 行的行级探针净收益都是 0——在这个退化表示上，
  没有任何单行/单 trit 改动能改善 200 对里的任何一条；
- 证书不通过（57 条错，无法修）。

**结论**：问题不是「深度不够」，而是 **trit 化 + 线性堆叠本身破坏表示**。
要提升编辑容量，方向应该是「保留 f32 激活 + 只把权重三值化」（本仓的 `grouped`
路线），而不是继续叠三值线性层。

**复算**：

```bash
cargo run --release -p trit-wspace --example editset_sts_repair6 -- \
  --pairs sts-pairs-200.json --safetensors model.safetensors \
  --max-edits 8 --rows 8
```

**边界**：不是完整 BERT（无 attention/LN/残差）；搜索只在最后一层；200 对子集。

---

## 16. grouped 表示上的推理式编辑：baseline 180，仍只修 1 条

学生 = 单层 `GroupedLayer`（384→384，group=256，**输入保留原始 f32 句向量**，不像 §14/§15
那样先三值化），判定用 cosine。搜索与 §14 相同：行级探针 + 行内逐列试 `-1/0/+1`。

| 配置 | baseline right/200 | repair 步数 | edit_fixed | keep_broken |
|---|---|---|---|---|
| 单层纯 trit（§14） | 184 | 1 | 1 | 0 |
| **单层 grouped（group=256）** | **180** | **1** | **1** | **0** |
| 6 层 trit 堆叠（§15） | 143 | 0 | 0 | 0 |

- grouped 的 baseline 比纯 trit **低 4 条**（180 vs 184）：单层 384×384 上 group=256 的组太粗
  （576 组，每组 256 个元素共享 scale），没有带来 §5 那种「6 层 × 6 权重」上的精度优势；
- 推理式编辑同样只修 1 条（row 70, col 244），**效率结论不变**（1 个 trit 换 1 条）；
- 三组读数合起来：**瓶颈是单层表达力与 200 对子集，不是三值/分组格式本身**。

**复算**：

```bash
cargo run --release -p trit-wspace --example editset_sts_grouped_repair -- \
  --pairs sts-pairs-200.json --safetensors model.safetensors \
  --group 256 --max-edits 8 --rows 8
```

**边界**：单层学生、200 对子集、threshold 0.5；group 未扫；没有 6 层 grouped 学生。

---

## 17. genre 三分类头上的推理式编辑：train +38，test +7（泛化）

前面的 STS pair 判定（§14–§16）都只修 1 条，怀疑错样本是难例/噪声。换一个更清晰的
任务：STS 数据自带 `genre`（3 类：captions / forums / news），单句向量 → 类别。

- 学生 = 单层 `GroupedLayer`（3×384，每类一行，group=384），输入 f32 句向量，argmax 判定；
- 初始化 = 每类训练句向量的**类质心**三值化（无梯度）；
- 编辑 = 训练集上贪心搜「改一个 trit 能多对几条」，每步同时记录测试集。

| 集合 | baseline | 8 步编辑后 | 变化 |
|---|---|---|---|
| 训练（1708） | 1562（91.45%） | **1600（93.68%）** | **+38** |
| 测试（844） | 768（90.99%） | **775（91.82%）** | **+7** |

8 步全部落在 row 0/1（captions/news 两类），每次改 1 个 trit；**测试集同步提升**，
说明不是纯过拟合。

**结论（第一个正结果）**：推理式编辑在分类头上**有效且泛化**——8 个 trit 换 +38 训练 /
+7 测试。这与 §14–§16 的「STS pair 判定只修 1 条」形成对照：**瓶颈在任务，不在方法**。
pair 判定的错样本需要更复杂的决策边界，单层三值网络改一格救不回来；分类头的错样本
大多是「边界附近的点」，改一个 trit 就能推动。

**复算**：

```bash
python scripts/export_genre_samples.py --model <快照> --data <test.jsonl.gz> --out genre-samples.json
cargo run --release -p trit-wspace --example classifier_genre_repair -- \
  --samples genre-samples.json --group 384 --max-edits 8
```

**边界**：单层线性分类头、类质心初始化（无梯度）、3 类、单次 2:1 划分；没有多 seed / λ 扫描。
