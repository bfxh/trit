# 真实 tuned 权重上的编辑集门禁

## 为什么换真实 tuned

此前所有 `edit_fixed` / `keep_broken` 的 `--tuned` 都是“每 10 个元素取反”的合成 Δ，
结论不能外推。这一层用**真实梯度微调**的 `query` 层替代它。

## 训练

`scripts/train_query_layer.py`：在 STSb **train** split 上对 `query` 层（384×384，f32）做
Adam 微调，输入是 MiniLM 编码的 384 维句向量，目标函数是
`BCE(cos(W·a, W·b) × 4, label)`（score ≥4 正、≤2 负、=3 丢弃）。

```bash
HF_ENDPOINT=https://hf-mirror.com python scripts/train_query_layer.py \
  --model <本地 MiniLM 快照> --out tuned-query.safetensors --epochs 12 --lr 1e-3
```

训练读数：

| 项 | 值 |
|---|---:|
| 训练对 / 句子 | 3,373 / 6,295 |
| 准确率（阈值 0.5） | 0.7735 → **0.9457** |
| 权重变化 | 146,685 / 147,456 个元素，`|Δ|` 均值 0.0287 |

产物张量名与 base 一致，可直接作为 `editset_sts --tuned` 的输入。

## 真实读数

400 对 benchmark，`--tuned` = 上表的真实微调权重（`delta` 三值变化 34,225 个）：

| threshold | 基线 对/错 | edit 集 | edit_fixed | keep_broken | 补丁后 对/错 |
|---|---:|---:|---:|---:|---|
| 0.3 | 369 / 31 | 31 | **9** | 0 | **378 / 22** |
| 0.5 | 372 / 28 | 28 | 0 | **16** | 356 / 44 |
| 0.8 | 270 / 130 | 130 | 0 | 1 | 269 / 131 |

## 与合成 Δ 的对照

| threshold | 合成 Δ（10,128 trits） | 真实 tuned（34,225 trits） |
|---|---|---|
| 0.3 | 修 0 / 破坏 0 | **修 9 / 破坏 0**（369→378） |
| 0.5 | 修 0 / 破坏 4 | 修 0 / **破坏 16**（372→356） |
| 0.8 | 修 3 / 破坏 0 | 修 0 / 破坏 1 |

读法：

1. **真实 tuned 第一次给出正向收益**：0.3 阈值下修好 9 个 edit 样本、keep 0 破坏，
   总正确 369→378——这是合成 Δ 从未做到的；
2. 同一份 tuned 在 0.5 阈值下 **破坏 16 个 keep**（372→356）：编辑的方向对低阈值有利、
   对中阈值有害，说明 `—tuned` 的效果强依赖判定阈值，不能只报一个数字；
3. 0.8 阈值下几乎不动（修 0 / 破坏 1）：高阈值区间的样本对 query 层的局部改动不敏感。

## 未证栏

- 训练目标是**协议级**（句向量 → 384 维余弦），不是 HF 端到端微调；产物不能当 HF checkpoint 用；
- 只训练 `query` 层，其余权重与句向量编码器全部冻结；
- CPU 12 epoch、单 seed、STSb train 全量（3,373 对，未划分验证集），有过拟合可能；
- 测试仍取 benchmark 前 400 对（未随机化），与之前读数同分布，不能当独立测试集；
- `keep_broken` 的绝对值很小（0/4/16），单次运行的统计波动未评估。

## 接口

- `scripts/train_query_layer.py`：真实梯度微调 `query` 层并保存 tuned safetensors；
- `examples/editset_sts.rs --tuned <safetensors>`：同一份门禁协议，换 tuned 来源即可。
