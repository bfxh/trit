# trit-learn：三值训练原语（可发布）

`publish = true` 的小 crate，只放两个实验层都需要的东西，避免它们互相依赖：

| 模块 | 内容 |
|---|---|
| `quant` | absmean 三值量化；**STE 三变体**（Identity / Clipped / Soft）；scale 梯度 |

```rust
use trit_learn::{Ste, ternarize};

let q = ternarize(&[0.9, -0.4, 0.05]);
assert_eq!(q.q, vec![1, -1, 0]);
let grad = Ste::Soft.backward(0.9, q.scale, 0.1);
```

## 谁在用

- `trit-train`：三值线性层的 QAT（per-row scale）；
- `trit-graph`：TriMPNN 的可学习三态转移表（前向硬三票、反向 STE）。

## 边界

- 只有量化/STE/scale 梯度，没有优化器状态、没有自动微分。
- 算法侧的收敛对照在 `trit-train`；这里只保证原语一致。
