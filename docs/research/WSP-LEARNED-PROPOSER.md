# 学习式单点编辑提议器

状态：**已实现**（`learned` 模块 + CLI `train-proposer` / `propose-learned`）。
这是“模型提议、库复核”里的模型一侧：**不调用外部模型**，只用工件本身与 oracle 输入
统计训练一个 f32 线性打分器；调用方再用 `analyze` / `repair` 实测复核。

## 1. 提议空间与特征

候选 = `repair::candidates` 的全部单点编辑（改一个 trit 或一个 bias）。特征 16 维、
顺序固定：

| 特征 | 含义 |
|---|---|
| `layer_frac` / `row_frac` / `col_frac` | 层/行/列位置（归一化） |
| `to` / `current` | 目标 trit 与当前 trit（-1/0/+1） |
| `is_bias` | 是否 bias 编辑 |
| `row_nonzero` / `row_absmean` | 该行非零 trit 比例 / 平均绝对权重 |
| `row_scale` | 该行 scale / 全模型平均 scale |
| `row_bias` | 该行三值 bias |
| `row_activation_mean` / `row_activation_active` | 该行在 oracle 输入上的平均 pre-activation 幅值 / 非零比例 |
| `col_input_mean` / `col_input_nonzero` | 该列在该层输入上的均值 / 非零比例 |
| `to_matches_bias` | 目标值与该行 bias 同号 |
| `row_is_output` | 是否输出层 |

打分器是 `score = w·x + b`；训练标签是 `analyze` 的实测
`gain / (before_loss + after_loss)`，正数表示降低失配。特征不读取 `after_loss`，
避免标签泄漏。

## 2. CLI

```bash
trit-wspace train-proposer model.wsp --oracle oracle.json --out model.json
trit-wspace propose-learned model.wsp --oracle oracle.json --model model.json \
  --limit 10 --verify
```

- `train-proposer`：`analyze` 全量单点候选 → 样本；按 `--holdout` 随机留出**编辑**，
  SGD 训练线性模型；报告 train/holdout MSE、留出 top-16 命中与正样本基线。
- `propose-learned`：模型打分排序取 top-K；`--verify` 对 top-K 逐个 apply + oracle
  计算真实 `after_loss` / `gain`（库复核）。

## 3. 真实读数（4→8→1 随机网络 + 采样 oracle）

- 网络 `Net::random([4,8,1])`，oracle `sampled_behavior(vars=4, samples=64)`；
  打坏 8 个 trit 后 `before_loss > 0`。
- 单点候选 98 条；训练 98 样本，49/49 划分，300 epochs，lr 0.05，L2 1e-4。
- train MSE **0.00359**，holdout MSE **0.00681**。
- 留出正样本 **23/49（46.9%）**；模型 top-16 命中 **10（62.5%）**，比基线 **+15.6 pp**。
- 模型 top-10 在完整工件上 `--verify` 实测：**8/10** 降低 loss。
- 成本：训练标签需要 `analyze` 跑 98 条候选的实测；推理只对 98 条候选做 16 维线性
  打分，不跑前向。适合“离线训练一次、多次提议”。

## 4. 边界（未证栏）

- 只在单工件内随机留出**编辑**，不是跨工件/跨任务泛化；没有训练/测试任务分离。
- 特征是手工静态/行为统计，没有用梯度、反向传播或输入-输出敏感性。
- 线性 f32 模型；“三值”指提议空间是三值编辑，不是模型权重三值。
- 只在 4→8→1 随机网络 + 采样 oracle 上测；真实 MiniLM 没有任务 oracle，未测。
- `train-proposer` 的标签来自 `analyze` 全量实测，训练成本高于一次推理。
- 没有接 MCP；没有与 `repair` 自动串联（模型只输出候选，复核/应用由调用方做）。
- 没有特征标准化/正则调参；不同尺寸网络的最优学习率未扫。
