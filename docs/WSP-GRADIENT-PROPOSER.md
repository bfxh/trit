# 从手工特征到梯度提议器

状态：**路线 A 已实现**（`learned::gradient_scores` + CLI `propose-gradient`）；
路线 B/C 仍为设计。

## 1. 路线 A：梯度知情排序（已实现）

对每个 oracle 样本做一次前向，再用 STE 反向传播：

- 前向与 `Net::readout` 一致：隐藏层 `sign(acc)`，输出层保留 `scale * acc`；
- 损失是输出层 raw 与目标 trit（-1/0/+1）的平方误差；
- 输出层 `δ = (raw - target) * scale`；
- 隐藏层 `δ = Σ δ_next · w_next`，`sign` 的导数用直通估计器（STE，恒为 1）；
- 权重梯度 `∂L/∂w = δ_row · x_col`，bias 梯度 `∂L/∂b = δ_row`，按样本累加后平均。

候选分数 = `-grad · (to - current)`（下降方向为正），排序取 top-K。与 `analyze` 不同，
这里只跑**一次**前向 + 一次反向，而不是对每个候选各跑一次前向。

```bash
trit-wspace propose-gradient model.wsp --oracle oracle.json --limit 8 --verify
```

`--verify` 对 top-K 逐个 apply + oracle 计算真实 `after_loss`/`gain`，保留库复核。

## 2. 读数（4→8→1 随机网络，采样 oracle 64，打坏 8 trit，98 候选）

| 项 | 值 |
|---|---:|
| `analyze`（98 次前向） | 6.11 ms |
| `gradient_scores`（1 次前向 + 反向） | 0.24 ms |
| 加速比 | ~25×（debug 构建） |
| gradient top-10 真实命中 | 7/10 |
| analyze top-10 真实命中 | 10/10 |
| 两个 top-10 的重叠 | 3/10 |

结论：STE 梯度给出的方向有 70% 的 top-10 能真实降低 loss，成本低约 25 倍；但排序与
离散实测不完全一致（重叠仅 3/10），不能替代 `analyze` 的最终复核。

## 3. 路线 B/C（未实现）

### B. 可微特征 + MLP

- 输入：每层输入 `x`、`w`/`b`/`scale` 的局部统计、`∂L/∂w`；
- 1 隐层 MLP（16→32→1），手写前向/反向；
- 损失：pairwise ranking（正样本 `gain>0` 排在负样本前）或 MSE；
- 只训练打分器，不写回权重。

### C. 端到端可微代理

- 动作 `a ∈ {+1,0,-1}` 的 one-hot 输入；
- 目标：实测 `gain` 或 `Δloss` 的可微代理（用松弛权重算）；
- 反向传播通过特征提取器；
- 推理时对每个候选打分，库复核。

## 4. 下一步

1. 把 `|∂L/∂w|` 加入 `learned` 的 16 特征（第 17 维），重训线性模型，看 top-K 是否提升。
2. 在更多 seed/网络上比较 gradient vs linear vs analyze 的 top-K 命中与重叠。
3. 若线性仍不足，再上 B 的 MLP；C 只在有真实任务 oracle 后再考虑。

## 5. 未证栏

- STE 梯度与离散编辑真实收益的排序不完全一致（top-10 重叠 3/10）。
- 只在 4→8→1 随机网络 + 采样 oracle 上测；真实大模型的梯度尺度/数值稳定性未测。
- 耗时是 debug 构建读数；release 比例未单独测。
- B/C 未实现；端到端代理与真实 `gain` 的偏差未证。
