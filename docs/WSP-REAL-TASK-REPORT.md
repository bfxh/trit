# 权重空间编辑在真实任务上的实验报告（M1 阶段）

> 范围：`docs/WSP-SUMMARY.md` §13–§18 六个实验的协议、读数、复算命令与负结果。
> 口径遵循 `docs/SCOPE.md` §7.1：**只有「可复算命令 + 读数（含单位与规模档）」才算完成**；
> 负结果同样入册。统计显著性按 §10 的 bootstrap 口径（844 样本 CI 半宽约 1.7 点）。

## 0. 一句话结论

**方法有效，但幅度落在噪声里；瓶颈在任务难度，不在编辑算法。**

- 梯度微调（1505 trits）在真实任务上**破坏表示**：训练 +7.7 点，test −18.0 点；
- 推理式编辑（1–14 trits）**不破坏表示**：pair 判定只修 1 条，分类头可复现 +0.47 点（不显著）；
- 三值深度堆叠（6 层）**连 baseline 都保不住**（143/200 vs 单层 184/200）。

## 1. 实验矩阵

数据：`mteb/stsbenchmark-sts` 的 `test.jsonl.gz` / `train.jsonl.gz`（1379 / 5749 对），
本地 MiniLM `all-MiniLM-L6-v2`（384 维，6 层）。所有实验共享同一批句向量与标签口径
（score ≥ 4 正、≤ 2 负、= 3 丢弃）。

| # | 实验 | 学生表示 | 编辑方式 | 数据 | 复算入口 |
|---|---|---|---|---|---|
| E1 | 梯度微调 6 层 | 完整 MiniLM（f32） | Adam 36 个线性权重 | STSb train 2699 对 | `scripts/train_all_layers.py` + `scripts/evaluate_tuned_edit_gate.py` |
| E2 | 推理式·单层 pair | 单层三值 384→384 | 行级探针 + 行内贪心 | STS test 200 对 | `examples/editset_sts_repair.rs` |
| E3 | 推理式·6 层 trit 堆叠 | 6 层 384→384 三值线性堆叠 | 搜索最后一层 | STS test 200 对 | `examples/editset_sts_repair6.rs` |
| E4 | 推理式·grouped pair | 单层 `GroupedLayer`（f32 输入） | 同 E2 | STS test 200 对 | `examples/editset_sts_grouped_repair.rs` |
| E5 | 推理式·genre 分类头 | 单层 `GroupedLayer` 3×384 | 类质心初始化 + 训练集贪心 | genre 3 类，train 1708 / test 844 | `examples/classifier_genre_repair.rs` |
| E6 | 多 seed + 预算扫描 | 同 E5 | 同 E5，3 seed / 4 档预算 | 同 E5 | 同上 + `--seed` / `--max-edits` |

辅助脚本：`scripts/export_sts_pairs.py`（E2–E4 的句向量 pairs）、
`scripts/export_genre_samples.py`（E5–E6 的 genre 分类样本）。

## 2. 逐实验读数

### E1 梯度微调 6 层（负结果）

训练：36 个线性权重，169 步，lr 1e-4，batch 16；embedding/LayerNorm/bias 冻结。

| 集合 | base | tuned | 变化 |
|---|---|---|---|
| 训练 valid（674 对） | 0.7849 | **0.8620** | **+7.7 点** |
| STS test edit（338 对） | 0.9734 | **0.7929** | **−18.0 点** |
| STS test keep（534 对） | 0.2790 | **0.1367** | **−14.2 点** |

`regressions 119 / fixes 2`。**训练集提升完全不泛化**，且大面积破坏预训练表示。

### E2 推理式·单层 pair

| 路线 | 改动 trits | right/200 | edit_fixed | keep_broken |
|---|---|---|---|---|
| baseline | 0 | 184 | — | — |
| 推理式 | **1** | **185** | 1 | 0 |
| 梯度 tuned（layer0 query） | 1505 | 185 | 1 | 0 |

**1 个 trit 抵 1505 个 trit 的任务收益**；但两者都只修好 1 条。

### E3 推理式·6 层 trit 堆叠（负结果）

| 配置 | baseline right/200 | repair 步数 |
|---|---|---|
| 单层（E2） | 184 | 1 |
| **6 层堆叠** | **143** | **0** |

所有 384 行的行级探针净收益都是 0。**深度反而更差**：每层 trit 化都丢信息。

### E4 推理式·grouped pair

| 配置 | baseline right/200 | repair 步数 | edit_fixed |
|---|---|---|---|
| 单层纯 trit（E2） | 184 | 1 | 1 |
| **单层 grouped（group=256）** | **180** | **1** | **1** |

grouped 的 baseline 反而低 4 条——单层小张量的 group=256 太粗（576 组）。

### E5 genre 三分类头（第一个正结果）

| 集合 | baseline | 8 步编辑后 | 变化 |
|---|---|---|---|
| 训练（1708） | 1562（91.45%） | **1600（93.68%）** | **+38** |
| 测试（844） | 768（90.99%） | **775（91.82%）** | **+7** |

8 步全部落在 row 0/1（captions/news），每步改 1 个 trit；测试集同步提升。

### E6 多 seed + 预算扫描

| seed | train 增益 | test 增益 |
|---|---|---|
| 7 | +51 | +3 |
| 13 | +35 | +7 |
| 29 | +29 | +2 |
| **均值 ± 标准差** | **+38.3 ± 9.3** | **+4.0 ± 2.2** |

| max-edits | 实际步数 | train 增益 | test 增益 |
|---|---|---|---|
| 4 | 4 | +33 | +6 |
| 8 | 8 | +51 | +3 |
| 16 | **14** | **+64** | **+8** |
| 32 | **14** | **+64** | **+8** |

- test 增益 3/3 为正，方向可信；
- **+4/844 ≈ +0.47 点 < CI 半宽 1.7 点** ⇒ 不构成统计显著的证据；
- 14 步后无正收益候选（贪心局部最优）。

## 3. 结论与边界

### 3.1 能说的

1. **推理式编辑在真实任务上有效**：E2 用 1/1505 的改动量拿到与梯度微调相同的任务收益；
   E5 在分类头上可复现地产生正向增益（3/3 seed）。
2. **它不破坏表示**：E1 的梯度微调 test −18 点，而 E2/E4/E5 的 keep_broken 全为 0。
3. **瓶颈在任务**：同一方法在 pair 判定上只修 1 条，在 3 类分类头上修 38 条（训练）——
   pair 判定的错样本需要更复杂的决策边界。

### 3.2 不能说的

1. **「统计显著」**：E6 的 test 增益落在 844 样本的 CI 半宽内；
2. **「深度带来容量」**：E3 的 6 层堆叠 baseline 从 184 崩到 143；
3. **「grouped 表示更好」**：E4 的单层 grouped 比纯 trit 低 4 条（§5 的优势只在 6 层 × 6 权重上成立）；
4. **「推理式编辑能替代梯度微调」**：E5 修的是 3 类分类头（1152 个 trit 的线性模型），
   不代表能在真实 LLM 上工作。

### 3.3 下一步的正确方向

- **扩大测试集**（844 → 数千）或**换更细粒度任务**（多类/多标签），才能把 E6 的幅度变成显著；
- 若要做「编辑生效 + 不破坏」的事前保证，方向是 **AlphaEdit 零空间投影**（f32 空间），
  而不是继续在纯 trit 前馈网络上叠深度。

## 4. 复现清单

```bash
# 数据（一次性）
python scripts/export_sts_pairs.py --model <快照> --data <test.jsonl.gz> --out sts-pairs-200.json --limit 200
python scripts/export_genre_samples.py --model <快照> --data <test.jsonl.gz> --out genre-samples.json

# E1
python scripts/train_all_layers.py --model <快照> --data <train.jsonl.gz> --out tuned.safetensors --epochs 1 --lr 1e-4 --batch 16
python scripts/evaluate_tuned_edit_gate.py --model <快照> --base <model.safetensors> --tuned tuned.safetensors --data <test.jsonl.gz>

# E2 / E4 / E3
cargo run --release -p trit-wspace --example editset_sts_repair -- --pairs sts-pairs-200.json --safetensors <model> --max-edits 8 --rows 8
cargo run --release -p trit-wspace --example editset_sts_grouped_repair -- --pairs sts-pairs-200.json --safetensors <model> --group 256 --max-edits 8 --rows 8
cargo run --release -p trit-wspace --example editset_sts_repair6 -- --pairs sts-pairs-200.json --safetensors <model> --max-edits 8 --rows 8

# E5 / E6
cargo run --release -p trit-wspace --example classifier_genre_repair -- --samples genre-samples.json --group 384 --max-edits 8
for seed in 7 13 29; do
  cargo run --release -p trit-wspace --example classifier_genre_repair -- --samples genre-samples.json --group 384 --max-edits 8 --seed $seed
done
```

## 5. 未证栏（本报告）

| 项 | 现状 | 补证 |
|---|---|---|
| 统计显著的正增益 | E6 的 test 增益在 CI 半宽内 | 扩大测试集到数千 / 更细粒度任务 |
| 多分类头容量 | 只测了 3 类、单层线性头 | 5–10 类、多层头 |
| 真实 LLM 上的推理式编辑 | 只到 MiniLM 的 6 层 | ≥1B 模型 + 真实任务 oracle |
| 编辑可编辑性分层 | 已做（§19）：train 146 错中 **45 可单 trit 修（30.8%）**、test 76 错中 26（34.2%） | 双 trit / 更大预算的分层 |
