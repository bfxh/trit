# 阈值对齐后的单层 / 两层对照

## 为什么对齐

固定阈值下两个网络不可比：单层的有利区间在 0.3，两层在 0.5。按“valid 上 baseline 准确率最高”
选阈值，再在 test 上比较 `edit_fixed` / `keep_broken`——把两个网络放到同一把尺子上。

## 协议

- 872 对按索引切分：前 30% 为 valid（262 对），后 70% 为 test（610 对）；
- 阈值网格 0.01–0.95，选 valid 上 baseline 准确率最高的阈值；
- 用该阈值在 test 上报告 `edit_fixed` / `keep_broken`；
- 单层 = 三值 `query`（384→384），两层 = 三值 `intermediate.dense` + `output.dense`。

```bash
cargo run --release -p trit-wspace --example editset_aligned -- \
  --pairs sts-bench-full.json --safetensors model.safetensors \
  --tuned tuned.safetensors --arch ffn --valid-ratio 0.3
```

## 读数（seed 7）

| arch | valid 最佳阈值 | valid 准确率 | test baseline | test after | edit_fixed | keep_broken |
|---|---|---:|---|---|---:|---:|
| single（query） | 0.35 | 95.8% | 510 / 100 | 532 / 78 | **22** | **0** |
| ffn（两层） | 0.27 | 92.7% | 521 / 89 | 464 / 146 | 0 | **57** |

`changed`：单层 32,838 trits，两层 397,523 trits（约 12 倍）。

## 结论：对齐后反转

| 口径 | 单层 | 两层 |
|---|---|---|
| 固定阈值 0.5 | 修 0 / 破坏 31 | **修 127 / 破坏 0** |
| 阈值对齐（valid 选） | **修 22 / 破坏 0** | 修 0 / 破坏 57 |

**“两层更好”是阈值选择偏差**：两层的 127 个修复用的是 test 上的 0.5 阈值，而它自己的
valid 最优阈值是 0.27；在 0.27 上它反而破坏 57 个 keep。

这条说明：网络规模的对比必须先把阈值对齐，否则结论会随阈值翻转。

## 多 seed 复核（5 单层 + 3 两层）

上一轮对齐只用 seed 7。用已有的 tuned 权重每个 seed 各跑一次对齐：

| arch | seeds | threshold | valid 准确率 | test baseline | edit_fixed | keep_broken |
|---|---|---|---|---|---|---|
| single | 5 | 0.35（全部一致） | 95.8% | 510 / 100 | **20.6**（18–22，sd 1.50） | **0**（全 0） |
| ffn | 3 | 0.27（全部一致） | 92.7% | 521 / 89 | **0**（全 0） | **43.33**（29–57，sd 11.44） |

读法：

1. 阈值在 seed 间**完全一致**（0.35 / 0.27）——valid 选阈值不随 seed 波动；
2. 单层 5 个 seed 全部修 18–22、破坏 0，稳定正向；
3. 两层 3 个 seed 全部修 0、破坏 29–57，方向一致但幅度波动较大（sd 11.44）；
4. **“单层更好”不是 seed 7 的偶然**。

## 随机化划分（句级分组，5 次重复）

之前按 pairs 索引切分，同一句可能跨 valid/test。这里按句向量 hash 聚组（并查集），
组随机分到 valid/test，重复 5 次（split-seed 1..5）：

| arch | edit_fixed（5 次） | keep_broken（5 次） | threshold |
|---|---|---|---|
| single | 8 / 0 / 7 / 0 / 4（mean **3.8**，sd 3.37） | 0 / 10 / 0 / 1 / 0（mean **2.2**，sd 3.92） | 0.38–0.43 |
| ffn | 0 / 0 / 0 / 0 / 0 | 29 / 46 / 50 / 37 / 47（mean **41.8**，sd 7.73） | 0.24–0.28 |

读法：

1. 单层的“修 20.6、破坏 0”在随机划分下**降到修 3.8（0–8）、破坏 2.2（0–10）**——
   按索引切分确实放大了单层的收益（主题/分布泄漏）；
2. 两层的方向不变：修 0、破坏 29–50，“两层在 valid 最优阈值下有害”是稳的；
3. 所以“单层更好”只能弱化为“单层在随机划分下略好于两层，但优势很小且不稳定”。

## 未证栏

- 阈值网格 0.01、valid 262 对，选出的阈值有 ±0.01 级噪声；
- valid/test 按索引切分（非随机），句对之间有主题相关性；
- 已用 5 单层 + 3 两层 seed 复核；两层 `keep_broken` 的 sd 11.44 仍偏大；
- 两层的 `changed` 是单层的 12 倍，编辑幅度本身不同；
- 训练仍是协议级（句向量 → 三值前向），不是 HF 端到端微调。

## 接口

- `examples/editset_aligned.rs`：`--arch single|ffn`、`--valid-ratio`、自动选阈值，
  输出 `threshold` / `valid_accuracy` / `delta.test_*` 与两个 `KeepReport`。
