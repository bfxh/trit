//! 三值量化与 STE 的**再导出**：实现已下沉到 `trit-core::quant`（base 层）。
//!
//! 保留 `trit_learn::quant::*` 与 `trit_learn::Ste` 这条路径，是为了让实验层
//! （`trit-train` / `trit-attn` / `trit-parts`）和外部 git pin 不必同批改导入。

pub use trit_core::quant::{Quantized, Ste, quantize_with_scale, scale_grad, ternarize};
