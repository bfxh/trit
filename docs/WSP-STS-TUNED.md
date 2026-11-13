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

## 多 seed + held-out

训练脚本支持 `--seeds 7,13,29 --valid-ratio 0.2`：STSb train 切成 2,699 训练 / 674 验证。

| seed | train 准确率 | valid 准确率 | base valid |
|---|---:|---:|---:|
| 7 | 0.9485 | 0.8961 | 0.7819 |
| 13 | 0.9492 | 0.8961 | 0.7626 |
| 29 | 0.9511 | 0.8798 | 0.7626 |

三个 seed 的 valid 准确率 0.8798–0.8961（base 0.7626–0.7819），说明训练本身稳定。

门禁读数（400 对 benchmark，3 seed × 3 阈值）：

| threshold | edit_fixed 均值（范围） | keep_broken 均值（范围） | 补丁后正确 均值 |
|---|---|---|---|
| 0.3 | **11.0**（9–13，sd 1.63） | 0 | 380.0（378–382） |
| 0.5 | 0 | **17.3**（15–20，sd 2.06） | 354.7（352–357） |
| 0.8 | 0 | 0.33（0–1，sd 0.47） | 269.7（269–270） |

读法：

1. 0.3 阈值下 `edit_fixed` 的 9–13 区间（sd 1.63）是真实收益的置信范围，`keep_broken` 三个 seed 都是 0；
2. 0.5 阈值下 `keep_broken` 15–20（sd 2.06）稳定为负收益——不是单 seed 的偶然；
3. 0.8 阈值下两个量都接近 0，high-threshold 区间对 query 层改动不敏感。

## 标准证书

pair 判定不是单输出，`certify_nets` 的 `Oracle` 不适用。新增
`KeepReport::from_verdicts(want, before, after, rows)`，把逐对判定组装成与
`trit-wspace.certificate.v1` 同一套结构：

- `edit` 集 = 基线判错的对（`after_loss` 是仍未修好的数）；
- `keep` 集 = 基线判对的对（`regressed` 是被破坏的数）；
- `editset_sts --strict`：`certified=false` 时非零退出，可作 CI 门禁。

证书的 `input` 字段为空（pair 判定没有单输入），`want` / `before` / `after` 用单元素三值表示。

## 未证栏

- 训练目标是**协议级**（句向量 → 384 维余弦），不是 HF 端到端微调；产物不能当 HF checkpoint 用；
- 只训练 `query` 层，其余权重与句向量编码器全部冻结；
- 3 个 seed 只覆盖训练随机性；held-out 是 STSb train 的 20% 子集，不是独立测试集；
- 测试仍取 benchmark 前 400 对（未随机化），与之前读数同分布，不能当独立测试集；
- `keep_broken` 的绝对值很小（0/4/16），单次运行的统计波动未评估。

## 接口

- `scripts/train_query_layer.py`：真实梯度微调 `query` 层并保存 tuned safetensors；
- `examples/editset_sts.rs --tuned <safetensors>`：同一份门禁协议，换 tuned 来源即可。
