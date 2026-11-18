//! 三值训练原语与神经组件（可发布）。- [`quant`]：absmean 三值量化、STE 三变体（Identity / Clipped / Soft）、scale 梯度；
//! - [`dense`]：批量三值 QAT 全连接层（STE 反向 + 外部优化器接口）；
//! - [`activation`]：激活函数（tanh / ReLU / sigmoid）及其导数；
//! - [`loss`]：MSE / hinge / softmax 交叉熵与解析梯度；
//! - [`opt`]：平面向量优化器（SGD 动量 / Adam）与梯度裁剪；
//! - [`forecast`]：SMA / EWMA / Holt 预测与误差指标。
//!
//! 这些组件同时服务多个实验层：`trit-train`（三值线性层 QAT，用 [`quant`]）、`trit-graph`（TriMPNN 三态转移表）、`trit-attn`（注意力训练集成，用全套）。
//!
//! 放在可发布 crate 里，是为了让实验层不必互相依赖（发布图约束）。

pub mod activation;
pub mod dense;
pub mod forecast;
pub mod loss;
pub mod metrics;
pub mod opt;
pub mod quant;

pub use activation::Activation;
pub use dense::TernaryDense;
pub use forecast::{Ewma, Holt, mae, mape, rmse, sma};
pub use loss::{cross_entropy, mse, softmax, softmax_backward};
pub use metrics::Confusion;
pub use opt::{Adam, Optimizer, Sgd, clip_global_norm};
pub use quant::{Quantized, Ste, quantize_with_scale, scale_grad, ternarize};
