//! 激活侧的三值化选项：LayerNorm 参数三值、LN/GELU 输出三值。

use trit_wspace::safetensors::Weights;

/// 激活侧模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationMode {
    /// LN 参数与激活都保持 f32（只三值化线性权重）。
    F32,
    /// LN 的 weight 三值化；激活保持 f32。
    TernaryLn,
    /// LN 的 weight 三值化，且 LN/GELU 的输出再符号三值化。
    TernaryAll,
}

impl ActivationMode {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "f32" => Self::F32,
            "ternary-ln" => Self::TernaryLn,
            "ternary-all" => Self::TernaryAll,
            other => panic!("未知 activation mode {other}（f32|ternary-ln|ternary-all）"),
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::TernaryLn => "ternary-ln",
            Self::TernaryAll => "ternary-all",
        }
    }

    fn ternary_norm(self) -> bool {
        matches!(self, Self::TernaryLn | Self::TernaryAll)
    }

    pub(crate) fn ternary_activation(self) -> bool {
        self == Self::TernaryAll
    }
}

/// f32 LayerNorm。
pub(crate) struct Norm {
    pub(crate) weight: Vec<f32>,
    pub(crate) bias: Vec<f32>,
    pub(crate) eps: f32,
}

impl Norm {
    pub(crate) fn new(weight: Vec<f32>, bias: Vec<f32>, eps: f32) -> Self {
        Self { weight, bias, eps }
    }

    fn forward(&self, values: &mut [f32]) {
        let n = values.len() as f32;
        let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / f64::from(n);
        let var = values
            .iter()
            .map(|&v| {
                let d = f64::from(v) - mean;
                d * d
            })
            .sum::<f64>()
            / f64::from(n);
        let inv = 1.0 / (var + f64::from(self.eps)).sqrt();
        for (index, value) in values.iter_mut().enumerate() {
            *value =
                (((f64::from(*value) - mean) * inv) as f32) * self.weight[index] + self.bias[index];
        }
    }
}

/// LN 的 weight 三值化（一个 per-row scale），bias 保留 f32。
pub(crate) struct TernaryNorm {
    scale: f32,
    trits: Vec<i8>,
    bias: Vec<f32>,
    eps: f32,
}

impl TernaryNorm {
    fn new(weight: &[f32], bias: Vec<f32>, eps: f32) -> Self {
        let weights = Weights {
            rows: 1,
            cols: weight.len(),
            values: weight.to_vec(),
        };
        let layer = weights
            .to_layer_with_bias(Some(&[0.0]))
            .expect("LN weight 三值化失败");
        let scale = layer.scale(0).expect("LN scale");
        let trits = (0..weight.len())
            .map(|col| layer.trit(0, col).expect("LN trit").to_i8())
            .collect();
        Self {
            scale,
            trits,
            bias,
            eps,
        }
    }

    fn forward(&self, values: &mut [f32]) {
        let n = values.len() as f32;
        let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / f64::from(n);
        let var = values
            .iter()
            .map(|&v| {
                let d = f64::from(v) - mean;
                d * d
            })
            .sum::<f64>()
            / f64::from(n);
        let inv = 1.0 / (var + f64::from(self.eps)).sqrt();
        for (index, value) in values.iter_mut().enumerate() {
            let normalized = ((f64::from(*value) - mean) * inv) as f32;
            *value = normalized * self.scale * f32::from(self.trits[index]) + self.bias[index];
        }
    }
}

/// 按模式选择 LN 实现。
pub(crate) enum NormKind {
    F32(Norm),
    Ternary(TernaryNorm),
}

impl NormKind {
    pub(crate) fn new(mode: ActivationMode, weight: Vec<f32>, bias: Vec<f32>, eps: f32) -> Self {
        if mode.ternary_norm() {
            Self::Ternary(TernaryNorm::new(&weight, bias, eps))
        } else {
            Self::F32(Norm::new(weight, bias, eps))
        }
    }

    fn forward(&self, values: &mut [f32]) {
        match self {
            Self::F32(norm) => norm.forward(values),
            Self::Ternary(norm) => norm.forward(values),
        }
    }

    pub(crate) fn forward_seq(&self, values: &mut [f32], seq: usize, ternary_out: bool) {
        let width = values.len() / seq;
        for token in 0..seq {
            let slice = &mut values[token * width..(token + 1) * width];
            self.forward(slice);
            if ternary_out {
                quantize_activation(slice);
            }
        }
    }
}

/// 符号三值化：`>0 → +1`、`<0 → -1`、`0 → 0`。
pub(crate) fn quantize_activation(values: &mut [f32]) {
    for value in values {
        *value = if *value > 0.0 {
            1.0
        } else if *value < 0.0 {
            -1.0
        } else {
            0.0
        };
    }
}
