# 复现《Weight-Space Programming Doesn't Need Transformers》的稀疏事实表读取器

论文：[alphaXiv:2611.weight-space-programming-associative-memory](https://www.alphaxiv.org/abs/2611.weight-space-programming-associative-memory)
（Hyunjong Ok, Jaeho Lee / POSTECH）
代码：[effl-lab/Weight-Space-Programming](https://github.com/effl-lab/Weight-Space-Programming)

论文的核心主张：LLM 直接手写权重、不走梯度时，Transformer 的两个前提（适合梯度训练、
适合 GPU 稠密矩阵乘）都不再成立；三个前沿 LLM 在自由设计时都收敛到**稀疏事实表 + 固定读取器**，
在 ARC 上最高 54.4%，参数量比同一 LLM 写的最优 Transformer 少约 1,800×。

本工程只复现其中**读取器**的一段：把论文仓库里 Claude Opus 5.5 第 5 代的
`mem_assoc2` 模型（二阶联想记忆）用 Rust 重新实现，在 ARC 的 3,530 道测试题上跑出
与论文一致的读数，并把不一致的部分定位到具体原因。

## 复现结果

模型：`models/opus/mem_assoc2_g5-arc_blind_K0_r0`，ARC-Easy + ARC-Challenge test，3,530 题。

| 实现 | accuracy | answered |
|---|---:|---:|
| 论文 `result.json` | 0.5237252124645893 | 0.9798866855524079 |
| **Rust 复现（本工程）** | **0.5240084985835695** | **0.9798866855524079** |
| Python 确定性 tie-breaking 参考 | **0.5240084985835695** | 0.9798866855524079 |

**Rust 与确定性 Python 参考逐位一致；与论文差 3 题（3530 题中的 3 题，0.085 个百分点）。**

差异原因已定位，不是实现错误：论文的 `arch.py` 用 `torch.topk(qb, 16)` 选问题最强的 16 个
概念，而 `torch.topk` 对**并列值**不保证顺序。这个模型里很多通用词的 idf 被截在 **0.8**，
所以第 13–16 名经常是一组 0.8 的并列概念；`torch.topk` 在这些并列概念里选哪几个，取决于
PyTorch 的内部实现，不是可移植语义。

- 题 47 / 64 / 946 是全部 3 题差异；
- 三题的第 13–16 名都落在 0.8 的并列组；
- 把 `torch.topk` 换成「值降序、值相等按概念 id 升序」的确定性规则后，Python 参考的
  accuracy 变成 **0.5240084985835695**，与 Rust 完全相同。

也就是说：**论文的 0.5237 依赖 `torch.topk` 的一个未定义 tie 顺序；确定性规则下是 0.5240。**
两者差 3 题，不影响「稀疏表 + 固定读取器能达到 ~52%」的结论，但说明论文读数不是逐位可复现的。

## 复现步骤

### 1. 取论文仓库与 ARC 数据

```bash
git clone --depth 1 https://github.com/effl-lab/Weight-Space-Programming
# ARC 的 parquet 放进 HF 缓存布局：
#   $HF_HOME/hub/datasets--allenai--ai2_arc/snapshots/<rev>/ARC-Easy/{test,validation}-*.parquet
#   $HF_HOME/hub/datasets--allenai--ai2_arc/snapshots/<rev>/ARC-Challenge/{test,validation}-*.parquet
```

### 2. 用 Python 导出权重（对齐基线，不参与交付）

```bash
cd Weight-Space-Programming
HF_HOME=<你的 HF_HOME> PYTHONPATH=. \
  python <本工程>/python/export_baseline.py \
  models/opus/mem_assoc2_g5-arc_blind_K0_r0 <输出目录>
```

产物：`weights.npz`（concept / idf / A / row_ids / row_w / mix，与 `build.build()` 逐值相同）
与 `arc_test.npz`（q / o / y）。

### 3. 跑 Rust 复现

```bash
cd <本工程>
cargo run --release -- <输出目录>/weights.npz <输出目录>/arc_test.npz
```

预期输出：

```text
概念 4324｜词表 46165｜二阶表 4194304×16｜测试题 3530
accuracy 0.5240084985835695｜answered 0.9798866855524079｜a n=3530
```

第一代 `mem_assoc2` 的二阶表是 4,194,304×**40**，用环境变量 `WSP_R=40` 跑同一个程序即可。

### 4. 对照论文与确定性参考

```bash
# 论文原始实现（torch.topk）
python <本工程>/python/dump_logits.py models/opus/mem_assoc2_g5-arc_blind_K0_r0 <输出目录>
# 确定性 tie-breaking 的 Python 参考
python <本工程>/python/eval_det_topk.py models/opus/mem_assoc2_g5-arc_blind_K0_r0 <输出目录>
```

## 工程边界

- **Rust 只实现 forward + 评测**；`kb/*.txt` → 权重的编译仍在 Python 的 `build.py` 里。
  这是刻意的：`build.py` 是论文的对齐基线（词形还原、idf、一阶/二阶表、否定规则约 1,000 行），
  先钉住读取器的逐题一致，再决定是否把编译也搬进 Rust。
- 只复现了 `mem_assoc2`（给定架构的一支），没有复现 `free*` 三个 writer 的自设计架构；
  论文的 54.4% 来自 `free5`，它的 `arch.py` 是另一套 forward，需要单独移植，本工程未做。
- 没有复现论文的 **17× GPU 推理速度**：那需要与同尺寸 Transformer 的同口径对照，
  论文里 Transformer 侧用的是随机权重计时；本工程没有接这部分。
- 没有做三值化。论文的稀疏表是 f32/文本概念；把它变成 trit 的双导轨三值表示
  （以及删一行 = 清一位的全量局部性穷举）是下一步，不在本工程内。
- 本工程是独立 workspace，不进主 workspace、不进 CI；依赖本机论文仓库检出与 ARC parquet。
