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

## 三值化实验（2026-11-17）

环境变量：`WSP_QUANT=sign|sign_row|threshold`、`WSP_QUANT_ROW`（二阶表单独覆盖）、
`WSP_QUANT_TARGET=a|row|both`、`WSP_QUANT_TAU=<τ>`。ARC test 3,530 题，未量化基线 **52.4008%**。

**粗糙的整表符号三值化会崩：**

| 方案 | accuracy | 相对基线 |
|---|---:|---:|
| 只 `A`，整表 sign | 46.6856 | −5.72 |
| 只 `A`，每行 sign | 47.0822 | −5.32 |
| 只二阶表 `row_w`，每行 sign | 52.0042 | −0.40 |
| 两者都符号三值化 | 46.7705 | −5.63 |

**但阈值三值化能收回大部分损失**（只量化 `A`，阈值 `τ·max|row|`，低于阈值的先置 0）：

| τ | 0.0 | 0.05 | 0.1 | 0.2 | 0.4 | 0.6 |
|---|---:|---:|---:|---:|---:|---:|
| accuracy | 46.2748 | 51.4164 | 51.9122 | **52.2592** | 51.1898 | 49.4051 |

τ=0.2 只掉 **0.14 点**；answered 从 97.99% 降到 94.14%（未答按 1/4 猜的论文口径）。

**可用组合**：`A` 阈值三值化（τ=0.2）+ 二阶表每行三值化 → **51.8697%（−0.53 点）**。
二阶表是权重的大头（4,194,304×16 f32 ≈ 268 MB），三值化后每行只需符号位 + 一个 scale。

**结论**：三值化不是"能不能"的问题，是"阈值放哪"的问题。`A` 的幅度信息承重——
先把弱关联置 0 再符号化，损失从 5.3 点降到 0.14 点；二阶表几乎不承重，可以直接符号化。

**未证**：τ 只扫了 6 个点；scale 用每行均值、没有学；`A` 置 0 后的实际稀疏度与显存占用未统计；
没有把这套三值表接到 `trit-gpu` 的双导轨内核上做真实 GPU 读取器对照。

## 计时（CPU / GPU）

Rust release，3530 题（不含读 npz 与构建）：

| 版本 | 时间 | accuracy | 说明 |
|---|---:|---:|---|
| CPU（`select_nth_unstable_by`） | 136.70 ms | 52.4008% | top-16 只做选择，不全排序 |
| GPU 预处理（CPU 侧，旧版） | 30.41 ms | — | 已被下面的 GPU 预处理取代 |
| **GPU 端到端（GPU 预处理 + 读取器）** | **14.53 ms** | **52.4292%** | 全部在 GPU |
| GPU 内核（线程 0 串行二阶，f32） | 7.34 ms | 52.3725% | 上一版 |
| **GPU 内核（120 线程并行二阶，f32）** | **3.34 ms** | **52.4292%** | 5 次中位 |
| **GPU 内核（并行 + 三值 `row_w`）** | **3.09 ms** | **52.1176%** | `row_w` 268 MB → 48 MB |
| GPU 内核（u16 `row_ids`，f32） | 3.33 ms | 52.4292% | `row_ids` 268 → 134 MB |
| GPU 内核（u16 `row_ids` + 三值 `row_w`） | 3.77 ms | 52.1176% | 三值路径慢 22% |

`--gpu` 用 wgpu compute 跑读取器：CPU 先算好 top-16、pair 行号、qb 位图、选项概念表，
GPU 内核只做二阶/一阶读取与打分。数据上传（约 500 MB）不计入时间。

- **并行二阶把 GPU 内核从 7.34 ms 压到 3.31 ms（2.2×）**；比论文的 18 ms（RTX 3090）
  快 **5.4×**。论文口径未拆 topk，我们的 3.31 ms 也不含 CPU 预处理与上传。
- 二阶用 120 个线程各处理一对，累加走 `atomic<i32>` 定点（现在是 ×2^18，上一版 ×2^16）。
  最大误差 **1.7e-4**（2^16 时 6.4e-4），11–12 题翻转；准确率 52.4292%（f32）/
  52.1176%（三值），与 CPU 参考差 ±0.1 点——翻转的题有对有错，不是单向退化。
- 三值 `row_w` 的 5.6× 内存收益没有变成时间收益（3.31 → 3.34 ms），
  说明并行化后瓶颈已不在表带宽。
- **`row_ids` 压 u16（两个 id 打进一个 u32）：268 → 134 MB。** f32 路径持平（3.33 ms），
  三值路径 **慢 22%**（3.09 → 3.77 ms）——解包位运算的指令成本超过了省下的带宽，
  而三值路径本来就不在带宽瓶颈上。这是一条负结果：省内存可以，提速不行。
- **topk/编码搬上 GPU**：`prepare.wgsl` 从 q/o 直接生成 top-16、pair 行号、位图与选项表，
  端到端 **14.53 ms**（prepare kernel + 读取器），比旧版 CPU 预处理 30 ms + GPU 3.3 ms ≈ 33 ms
  快 **2.3×**。prepare 的 top-16 仍是单线程线性扫描，是剩余时间的大头。
- **并行 top-16 是负结果**：128 线程各扫 34 个概念 + 线程 0 归约 2048 个候选，
  端到端 14.53 → **16.37 ms**（慢 13%）。共享内存从 17 KB 涨到 33 KB，occupancy
  下降抵消了并行收益；tie 顺序也变了（最大误差 11.6，准确率仍 52.4008%）。已回退。
- **两阶段归约也是负结果**：stage 写 120×16 条/问题（54 MB）+ 分段归约，
  端到端 14.53 → **24.87 ms**（慢 71%）。精度略好（6.1e-5 vs 定点 1.7e-4），
  但 54 MB 中间读写 + 866M 次比较远贵于 `atomic<i32>`。已回退。

## 跨模型验证（三个 writer）

同一程序跑三个 writer 的 `mem_assoc2`（`WSP_R` 按模型的 R 设）：

| writer | C | R | 论文 | CPU | GPU（全流程） | GPU 时间 |
|---|---:|---:|---:|---:|---:|---:|
| opus | 4324 | 16 | 52.3725% | 52.4008% | 52.4292% | 14.53 ms |
| sol | 4293 | 64 | 45.5949% | 45.6516% | 45.5666% | 16.41 ms |
| astra | 4962 | 57 | 47.4079% | 47.4363% | 47.4646% | 19.22 ms |

- CPU 与论文差 0.03–0.06 点，全部是 top-16 并列的 tie 顺序；GPU 与 CPU 差 0.03–0.09 点
  （4–6 题翻转，最大 logits 误差 2e-5–5e-5）。
- 读取器没有硬编码形状：H/R 从权重读（`WSP_R`），pair 行号用运行时 `h` 取模；
  共享数组上限 5120，覆盖 4293–4962 的概念数。
- 三值 `row_w` 只支持 R ≤ 32，只有 opus 走三值路径。

## 确定性（串行二阶，`WSP_SERIAL=1`）

原子累加的顺序不确定，是 CPU↔GPU 误差的主要来源。把二阶改成线程 0 串行（固定 pair 顺序）：

| 版本 | 全流程 | 最大误差 | accuracy |
|---|---:|---:|---:|
| 原子定点（默认） | 14.53 ms | 1.7e-4 | 52.4292% |
| **串行 f32（`WSP_SERIAL=1`）** | 19.17 ms | **3.1e-5** | 52.3725% |

误差降 **5×**，代价是 **+32%** 时间。剩余 3.1e-5 来自 top 顺序（CPU 的 `select_nth` vs
GPU 的线性扫描）、选项顺序（概念 id vs 词序）和 f32 语义，不是原子。完全逐位需要统一
这三处 + 定点化——这是「整数确定性」方向的下一步。

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
