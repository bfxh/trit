# trit-learn：三值训练原语（可发布）

`publish = true` 的小 crate，只依赖 `trit-core`。它放的是**会改参数**的共用原语：
量化 / STE、批量 QAT 全连接层、激活、损失、优化器。放在可发布 crate 里，是为了让
实验层（`trit-train` / `trit-attn` / `trit-graph`）不必互相依赖
（发布图约束见 `scripts/publish_graph.py`）。

## 模块

| 模块 | 内容 | 主要导出 |
|---|---|---|
| `quant` | absmean 三值量化、**STE 三变体**（Identity / `Clipped` / Soft）、scale 梯度。实现下沉在 `trit-core::quant`，这里保留同名再导出，既有导入路径与外部 git pin 不受影响 | `Quantized`、`Ste`、`ternarize`、`quantize_with_scale`、`scale_grad` |
| `dense` | 批量三值 QAT 全连接层 `Y[t,r] = scale[r]·Σ_c q[r,c]·X[t,c]`；前向恒用三值权重，反向经 STE 回 latent，梯度在层内累积 | `TernaryDense` |
| `activation` | `tanh` / `ReLU` / `sigmoid` 及其导数；导数由**激活后输出**给出 | `Activation` |
| `loss` | MSE / hinge / softmax 交叉熵与解析梯度；超越函数在 f64 内部求值再落回 f32，避免各平台 libm 的 1 ulp 差异放大成不同训练轨迹 | `mse`、`hinge`、`softmax`、`cross_entropy`、`softmax_backward` |
| `opt` | 平面向量优化器（带动量 SGD / Adam）与全局范数裁剪；每个参数张量各配一个实例，无全局状态 | `Optimizer`、`Sgd`、`Adam`、`clip_global_norm` |

crate 根再导出 `Activation`、`TernaryDense`、`mse`、`softmax`、`cross_entropy`、
`softmax_backward`、`Optimizer`、`Sgd`、`Adam`、`clip_global_norm`、
`Quantized`、`Ste`、`ternarize`、`quantize_with_scale`、`scale_grad`
（清单见 `crates/trit-learn/src/lib.rs`）。`hinge` 未提到根，用 `trit_learn::loss::hinge`。

## 用法

量化与 STE：

```rust
use trit_learn::{Ste, ternarize};

let q = ternarize(&[0.9, -0.4, 0.05]);
assert_eq!(q.q, vec![1, -1, 0]);
let grad = Ste::Soft.backward(0.9, q.scale, 0.1);
assert!(grad.is_finite());
```

一步 QAT（`T = 2` 个样本、`cols = 3`、`rows = 2`）：

```rust
use trit_learn::{Adam, Ste, TernaryDense, mse};

let mut layer = TernaryDense::new(
    2, 3,
    |r, c| ((r * 3 + c) as f32 - 2.0) * 0.1,
    |_| 0.5,
    Ste::Identity,
);
let mut latent_opt = Adam::new(0.01);
let mut scale_opt = Adam::new(0.01);

layer.zero_grad();
let batch = [1.0f32, 0.5, -0.5, 0.0, 1.0, -1.0]; // T×cols，行主序
let y = layer.forward(&batch);                    // T×rows
let (_loss, dy) = mse(&y, &[1.0, -1.0, -1.0, 1.0]);
layer.backward(&dy);
layer.update(&mut latent_opt, &mut scale_opt);
```

`forward` 缓存输入批，`backward` 必须紧跟同形状的 `forward`；`update` **不清梯度**，
下一步开始前要自己调 `zero_grad`。形状与梯度口径见 `crates/trit-learn/src/dense.rs`。

## 谁在用

- `trit-train`：三值线性层的 QAT（`quant`）；
- `trit-graph`：TriMPNN 的可学习三态转移表（`quant`）；
- `trit-attn`：注意力 + 残差 + FFN 的训练集成（全套）。

## 边界

- 没有自动微分、没有计算图：反向由各层自己写，`backward` 只处理本层。
- 没有优化器全局状态与参数组；`TernaryDense` 不带偏置（M0 边界，见 `crates/trit-learn/src/dense.rs`）。
- 没有评测清单 / 内容指纹 / 独立复核器：`trit-learn` 只保证训练原语一致，不声称模型质量。

## 复现

```bash
cargo test -p trit-learn
python scripts/publish_graph.py     # 发布图门
```
