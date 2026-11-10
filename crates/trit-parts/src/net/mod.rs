//! 整网前向 / 反向 / 更新（QAT：三值权重 + STE）。
//!
//! 图与 `parts-parity` 的前向完全一致（BN 已折叠、无 BN 参数参与训练）：
//! `x → e1 → pool → e2 → pool → e3 → u3 → cat(b) → d2 → u2 → cat(a) → d1 → head`。
//! 反向只对**有效权重 E** 求导（GPU），`E → latent/scale` 用 `backward::ste_apply`（主机侧）。

mod layer;
mod model;

pub use layer::*;
pub use model::*;
