//! 激活侧的三值化选项：LayerNorm 参数三值、LN/GELU 输出三值。
/// 激活侧模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationMode {
    /// LN 参数与激活都保持 f32（只三值化线性权重）。
    F32,
    /// LN 的 weight 三值化；激活保持 f32。
    TernaryLn,
    /// LN 的 weight 三值化，且 LN/GELU 的输出再符号三值化。
    TernaryAll,
    /// 同 `TernaryAll`，但符号化后乘 per-token 的 absmean（幅度 scale）。
    TernaryAllScale,
    /// 同 `TernaryAll`，但用 f32 校准的 per-channel absmean 作为 scale。
    TernaryAllChannel,
    Mixed,
    Int8,
}
#[path = "layer_scales.rs"]
mod layer_scales;
pub(crate) use layer_scales::LayerScales;
impl ActivationMode {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "f32" => Self::F32,
            "ternary-ln" => Self::TernaryLn,
            "ternary-all" => Self::TernaryAll,
            "ternary-all-scale" => Self::TernaryAllScale,
            "ternary-all-channel" => Self::TernaryAllChannel,
            "mixed" => Self::Mixed,
            "int8" => Self::Int8,
            other => panic!(
                "未知 activation mode {other}（f32|ternary-ln|ternary-all|ternary-all-scale|ternary-all-channel|mixed|int8）"
            ),
        }
    }
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::TernaryLn => "ternary-ln",
            Self::TernaryAll => "ternary-all",
            Self::TernaryAllScale => "ternary-all-scale",
            Self::TernaryAllChannel => "ternary-all-channel",
            Self::Mixed => "mixed",
            Self::Int8 => "int8",
        }
    }
    fn ternary_norm(self) -> bool {
        matches!(
            self,
            Self::TernaryLn | Self::TernaryAll | Self::TernaryAllScale | Self::TernaryAllChannel
        )
    }
    pub(crate) fn ternary_activation(self) -> bool {
        matches!(
            self,
            Self::TernaryAll | Self::TernaryAllScale | Self::TernaryAllChannel
        )
    }
    pub(crate) fn matmul_input(self) -> bool {
        matches!(self, Self::Mixed | Self::Int8)
    }
}
#[path = "norm.rs"]
mod norm;
pub(crate) use norm::NormKind;
/// 激活三值化：纯 sign，或 sign × per-token absmean（`TernaryAllScale`）。
pub(crate) fn quantize_activation(
    values: &mut [f32],
    mode: ActivationMode,
    scales: Option<&[f32]>,
) {
    if mode == ActivationMode::TernaryAllChannel
        && let Some(scales) = scales
    {
        for (index, value) in values.iter_mut().enumerate() {
            let scale = scales.get(index).copied().unwrap_or(1.0);
            *value = if *value > 0.0 {
                scale
            } else if *value < 0.0 {
                -scale
            } else {
                0.0
            };
        }
        return;
    }
    let scale = if mode == ActivationMode::TernaryAllScale {
        let mean =
            values.iter().map(|v| f64::from(v.abs())).sum::<f64>() / values.len().max(1) as f64;
        mean as f32
    } else {
        1.0
    };
    for value in values {
        *value = if *value > 0.0 {
            scale
        } else if *value < 0.0 {
            -scale
        } else {
            0.0
        };
    }
}
#[path = "int8.rs"]
mod int8;
pub(crate) use int8::quantize_int8;
