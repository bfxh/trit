# trit-wspace — 三值权重空间编程

把权重当**源码**：读出行为、写出补丁、验证后提交。不做梯度，不改 f32 latent，
只编辑 `{-1,0,+1}` 的权重本身。

> 外部背景（**未复现**）：2026-11-02 MIT CSAIL 报道 GPT-6 Astra 直接读写
> Qwen3.5-0.8B 的原始权重，改约 **0.016%** 的权重把 MATH-500 从 **7.9% 提到 48.1%**，
> 不走梯度（报道署名 Takehi 等）。本 crate 不复现该结果，只实现它暴露出的接口形态：
> **读 / 写 / 验证**，并把三值的离散性变成可审计性。

## 为什么三值适合权重空间编程

| 性质 | 三值权重 | f32 权重 |
|---|---|---|
| 一次编辑 | 离散、精确（三选一） | 连续值，误差不可复现 |
| 判等 | 逐位相等 | 要容差 |
| 回滚 | 存旧值即可逐位还原 | 要快照 |
| 行为验证 | 有界域可**穷举**，给出完全等价 | 只能抽样/留出 |
| 工件 | `WSP1` 文本可 diff、可校验和 | 二进制 blob |

代价：没有 f32 预训练生态；表达力受限；大规模穷举不可行。M0 的边界就是
**小网络 + 有界域**。

## 技术树（做到哪、还缺什么）

| 层 | 需要什么 | 本 crate | 缺口 |
|---|---|---|---|
| L0 表示 | 规范文本、逐 trit 可读、校验和、可 diff | `codec`（WSP1） | 大模型分片/压缩 |
| L1 原语 | 改 trit/偏置/scale、换行、取反 | `edit`（`Edit`） | 子张量/跨层批量操作 |
| L2 事务 | 整体校验、失败零改动、逐位回滚 | `Patch` + `Journal` | 并发/多版本事务 |
| L3 程序语义 | 前置/后置行为条件、失败原样回滚 | `program` | 程序组合、条件跳转 |
| L4 读取 | 行为穷举、指纹、f32 裕量读数 | `oracle` + `readout` | 大模型探针/解释器 |
| L5 搜索 | 无梯度精确修补、最小编辑数 | `repair`（小网络完备 BFS；大网络半径 1/2 + 束搜索） | 大规模目标、学习式提议器 |
| L6 代数 | 消置换对称性、合并 | `merge`（贪心对齐 + 符号投票） | TIES/DARE 的三值对应物、任务向量 |
| L7 规模 | GPU、外部 LLM 读写协议、模型库 | — | 全部 |

## 三值的两条规范自由度（本库钉死）

1. **正 scale 不改行为**：符号判决只看累加符号，scale 只影响 f32 读数；
2. **隐藏单元置换**：交换第 `l` 层两行并交换第 `l+1` 层两列，行为不变。

所以行为判等必须走 `Oracle` 穷举，不能比较权重。`merge` 先对齐再投票，
`Patch` 允许改 scale 但不把它算进行为。

## 快速验证

```bash
cargo test -p trit-wspace
cargo run -p trit-wspace --example program_xor
python scripts/mutation_check.py --pkg trit-wspace
python scripts/module_budget.py
```

## 论文与来源

- **Weight-space programming（外部报道，未复现）**：MIT CSAIL，2026-11-02，
  Astra 读写 Qwen3.5-0.8B 原始权重；0.016% 权重、MATH-500 7.9%→48.1%。
- A Survey of Weight Space Learning: Understanding, Representation, and Generation,
  arXiv:2603.10090（2026）——把 WSL 分成理解/表示/生成三支。
- HyperNetworks, Ha/Dai/Le, ICLR 2017, arXiv:1609.09106。
- Learning to Control Fast-Weight Memories, Schmidhuber, *Neural Computation* 1992。
- Using Fast Weights to Attend to the Recent Past, Ba et al., NeurIPS 2016。
- Linear Transformers Are Secretly Fast Weight Programmers, Schlag et al.,
  ICML 2021, arXiv:2102.11174。
- Git Re-Basin: Merging Models modulo Permutation Symmetries, Ainsworth et al.,
  ICLR 2023, arXiv:2209.04836。
- Editing Models with Task Arithmetic, Ilharco et al., ICLR 2023, arXiv:2212.04089；
  TIES-Merging, Yadav et al., NeurIPS 2023, arXiv:2306.01708。
- Ternary Weight Networks, Li/Liu/Wang, arXiv:1605.04711；
  BitNet b1.58（The Era of 1-bit LLMs）, Ma et al., arXiv:2402.17764。
- Estimating or Propagating Gradients Through Stochastic Neurons,
  Bengio/Léonard/Courville, arXiv:1308.3432（STE）。

## 边界（没做的部分）

- 只有符号激活的前馈网络；没有注意力、残差、卷积；
- 验证是**有界穷举**，不是形式化证明；域外没有保证；
- 修补在状态空间 ≤20 万时完备（最小编辑数），超过后是启发式；
- 没有自然语言 → 补丁的模型；本库提供的是供外部程序/模型调用的编辑原语与验证闭环。
