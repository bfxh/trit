//! 三值训练原语与神经组件（可发布）。
//!
//! - [`quant`]：absmean 三值量化、STE 三变体（Identity / Clipped / Soft）、scale 梯度；
//! - [`dense`]：批量三值 QAT 全连接层（STE 反向 + 外部优化器接口）；
//! - [`activation`]：激活函数（tanh / ReLU / sigmoid）及其导数；
//! - [`loss`]：MSE / hinge / softmax 交叉熵与解析梯度；
//! - [`opt`]：平面向量优化器（SGD 动量 / Adam）与梯度裁剪。
//!
//! 这些组件同时服务多个实验层：
//! - `trit-train`：三值线性层的 QAT（用 [`quant`]）；
//! - `trit-graph`：TriMPNN 的可学习三态转移表（用 [`quant`]）；
//! - `trit-attn`：注意力 + 残差 + FFN 的训练集成（用全套）。
//!
//! 放在可发布 crate 里，是为了让实验层不必互相依赖（发布图约束）。

pub mod activation;
pub mod dense;
pub mod loss;
pub mod opt;
pub mod quant;

pub use activation::Activation;
pub use dense::TernaryDense;
pub use loss::{cross_entropy, mse, softmax, softmax_backward};
pub use opt::{Adam, Optimizer, Sgd, clip_global_norm};
pub use quant::{Quantized, Ste, quantize_with_scale, scale_grad, ternarize};
