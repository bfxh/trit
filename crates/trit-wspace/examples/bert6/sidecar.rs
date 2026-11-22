//! top-k 列 f32 侧车：扫 rank 对 STS 的影响。
use crate::activation::{ActivationMode, LayerScales};
use crate::model::{Bert, BertConfig};
use crate::report::compare;
use crate::sts::{Pair, metrics, sentence_vectors};
use serde_json::{Value, json};

/// 对每个 rank 构建三值模型（全三值激活 + per-channel scale + 侧车），输出 STS。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    file: &str,
    input: &[f32],
    mask: &[u8],
    f32_out: &[f32],
    scales: &[LayerScales],
    pairs: &[Pair],
    batch: usize,
    seq: usize,
    hidden: usize,
    eps: f32,
    ranks: &[usize],
) -> Value {
    let mut out = Vec::with_capacity(ranks.len());
    for &rank in ranks {
        let config = BertConfig::new(hidden, eps, true, ActivationMode::TernaryAllChannel)
            .with_sidecar(rank);
        let mut bert = Bert::load(file, config);
        bert.set_scales(scales.to_vec());
        let (real, _, _) = bert.forward(input, mask, batch, seq, false);
        out.push(json!({
            "rank": rank,
            "real_vs_f32": compare(&real, f32_out, mask, hidden),
            "sts": metrics(&sentence_vectors(&real, mask, batch, seq, hidden), pairs),
        }));
    }
    json!(out)
}
