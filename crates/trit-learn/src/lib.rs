//! 三值训练原语（可发布）。
//!
//! - [`quant`]：absmean 三值量化、STE 三变体（Identity / Clipped / Soft）、scale 梯度。
//!
//! 这些原语同时服务：
//! - `trit-train`：三值线性层的 QAT；
//! - `trit-graph`：TriMPNN 的可学习三态转移表。
//!
//! 放在可发布 crate 里，是为了让两个实验层不必互相依赖（发布图约束）。

pub mod quant;

pub use quant::{Quantized, Ste, quantize_with_scale, scale_grad, ternarize};
