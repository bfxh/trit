//! 每层三处激活的 per-channel scale。

/// 每层三处激活的 per-channel scale（LN1 / GELU 后 / LN2）。
#[derive(Clone)]
pub(crate) struct LayerScales {
    pub(crate) norm1: Vec<f32>,
    pub(crate) ffn: Vec<f32>,
    pub(crate) norm2: Vec<f32>,
}
