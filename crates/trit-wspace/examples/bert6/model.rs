//! MiniLM BERT 6 层的 f32 / 三值前向（只三值化线性权重，LN/GELU/softmax 保持 f32）。
mod config;
mod forward;
mod loader;
mod parallel;
mod sidecars;
use crate::activation::{ActivationMode, LayerScales, NormKind, quantize_activation};
use crate::gelu::gelu;
use crate::linear::Linear;
pub(crate) use config::BertConfig;
pub(crate) struct BertLayer {
    pub(crate) query: Linear,
    pub(crate) key: Linear,
    pub(crate) value: Linear,
    pub(crate) output: Linear,
    pub(crate) norm1: NormKind,
    pub(crate) intermediate: Linear,
    pub(crate) ffn_output: Linear,
    pub(crate) norm2: NormKind,
    pub(crate) activation: ActivationMode,
}
pub(crate) struct Bert {
    pub(crate) hidden: usize,
    pub(crate) heads: usize,
    pub(crate) head_dim: usize,
    pub(crate) layers: Vec<BertLayer>,
    pub(crate) scales: Option<Vec<LayerScales>>,
}
impl Bert {
    pub(crate) fn set_scales(&mut self, scales: Vec<LayerScales>) {
        self.scales = Some(scales);
    }
    /// 前向；`capture` 为真时返回每层 FFN 输出段（GELU 后）的输入快照。
    pub(crate) fn forward(
        &self,
        input: &[f32],
        mask: &[u8],
        batch: usize,
        seq: usize,
        capture: bool,
    ) -> (Vec<f32>, Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let threads = parallel::threads();
        if !capture && threads > 1 {
            return parallel::forward(self, input, mask, batch, seq, threads);
        }
        forward::run(self, input, mask, batch, seq, capture)
    }
}
impl BertLayer {
    #[allow(clippy::too_many_arguments)]
    fn forward(
        &self,
        x: &mut [f32],
        mask: &[u8],
        seq: usize,
        hidden: usize,
        heads: usize,
        head_dim: usize,
        scales: Option<&LayerScales>,
    ) -> Vec<f32> {
        let xq = crate::quantize_input::quantize(x, hidden, self.activation, None);
        let mut q = vec![0.0f32; seq * hidden];
        let mut k = vec![0.0f32; seq * hidden];
        let mut v = vec![0.0f32; seq * hidden];
        self.query.forward_seq(&xq, seq, &mut q);
        self.key.forward_seq(&xq, seq, &mut k);
        self.value.forward_seq(&xq, seq, &mut v);
        let scale = 1.0 / (head_dim as f32).sqrt();
        let mut context = vec![0.0f32; seq * hidden];
        let mut scores = vec![0.0f32; seq];
        for head in 0..heads {
            let offset = head * head_dim;
            for row in 0..seq {
                for col in 0..seq {
                    if mask[col] == 0 {
                        scores[col] = f32::NEG_INFINITY;
                    } else {
                        let mut dot = 0.0f32;
                        for d in 0..head_dim {
                            dot += q[row * hidden + offset + d] * k[col * hidden + offset + d];
                        }
                        scores[col] = dot * scale;
                    }
                }
                softmax(&mut scores);
                for d in 0..head_dim {
                    let mut sum = 0.0f32;
                    for col in 0..seq {
                        sum += scores[col] * v[col * hidden + offset + d];
                    }
                    context[row * hidden + offset + d] = sum;
                }
            }
        }
        let mut projected = vec![0.0f32; seq * hidden];
        self.output.forward_seq(&context, seq, &mut projected);
        for index in 0..seq * hidden {
            x[index] += projected[index];
        }
        self.norm1
            .forward_seq(x, seq, self.activation, scales.map(|s| s.norm1.as_slice()));
        let intermediate = hidden * 4;
        let xq_ffn = crate::quantize_input::quantize(
            x,
            hidden,
            self.activation,
            scales.map(|s| s.norm1.as_slice()),
        );
        let mut ffn = vec![0.0f32; seq * intermediate];
        self.intermediate.forward_seq(&xq_ffn, seq, &mut ffn);
        for value in &mut ffn {
            *value = gelu(*value);
        }
        if self.activation.ternary_activation() {
            quantize_activation(&mut ffn, self.activation, scales.map(|s| s.ffn.as_slice()));
        }
        let snapshot = ffn.clone();
        let ffnq = crate::quantize_input::quantize(
            &ffn,
            intermediate,
            self.activation,
            scales.map(|s| s.ffn.as_slice()),
        );
        let mut out = vec![0.0f32; seq * hidden];
        self.ffn_output.forward_seq(&ffnq, seq, &mut out);
        for index in 0..seq * hidden {
            x[index] += out[index];
        }
        self.norm2
            .forward_seq(x, seq, self.activation, scales.map(|s| s.norm2.as_slice()));
        snapshot
    }
}
fn softmax(values: &mut [f32]) {
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for value in values.iter_mut() {
        *value = (*value - max).exp();
        sum += *value;
    }
    if sum > 0.0 {
        for value in values.iter_mut() {
            *value /= sum;
        }
    }
}
