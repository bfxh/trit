//! softplus（从 `energy.rs` 拆出，控制单文件行数）。

pub(super) fn softplus(value: f32) -> f32 {
    if value > 20.0 {
        value
    } else {
        value.exp().ln_1p()
    }
}
