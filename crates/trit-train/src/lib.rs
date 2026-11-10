//! 三进制训练实验层（M0）。
//!
//! 三件事，各自可独立验证：
//! - [`quant`]：三值量化与 **STE**（直通估计器）变体、每行 scale 的梯度；
//! - [`plane`]：**位平面 ALU**（双导轨 1 trit/2 bit，按字并行的 AND/OR/NOT/XOR，
//!   支持 Kleene / Łukasiewicz / Bochvar 三套语义）；
//! - [`layer`]：一个带 STE 的三值线性层（QAT 风格），能真训起来。
//!
//! 边界：
//! - 位平面只做**逻辑/强加法**；三进制**算术加法**不做——进位链在打包表示上是串行的，
//!   此前 `trit-balanced` 的实测是比 i64 慢 2–3 个数量级，属于已证负结果。
//! - 训练是单样本 SGD（demo 规模），不是框架；多线程/预取的读数见 `bench` 与 README。

pub mod layer;
pub mod plane;
pub mod prefetch;

/// 训练原语已下沉到可发布的 [`trit_learn`]；这里保留旧路径 `trit_train::quant`。
pub use trit_learn as quant;
pub use trit_learn::{Quantized, Ste, quantize_with_scale, scale_grad, ternarize};
