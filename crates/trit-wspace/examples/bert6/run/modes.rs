//! 各激活模式的前向与 STS 汇总（从 `run.rs` 拆出守住行数预算）。
use crate::activation::{ActivationMode, LayerScales};
use crate::model::{Bert, BertConfig};
use crate::report::compare;
use crate::sts::{Pair, metrics, sentence_vectors};
use serde_json::{Map, Value, json};

/// `TernaryAll` 的逐层快照（real, random）。
pub(crate) type Stages = (Vec<Vec<f32>>, Vec<Vec<f32>>);
pub(crate) struct Request<'a> {
    pub(crate) file: &'a str,
    pub(crate) gptq_dir: Option<String>,
    pub(crate) gptq_group: usize,
    pub(crate) hidden: usize,
    pub(crate) eps: f32,
    pub(crate) requested: Option<ActivationMode>,
    pub(crate) input: &'a [f32],
    pub(crate) random_input: &'a [f32],
    pub(crate) mask: &'a [u8],
    pub(crate) batch: usize,
    pub(crate) seq: usize,
    pub(crate) pairs: &'a [Pair],
    pub(crate) f32_out: &'a [f32],
    pub(crate) reference: &'a [f32],
    pub(crate) f32_random: &'a [f32],
    pub(crate) scales: &'a [LayerScales],
}

/// 逐档跑前向，返回 `modes` 表与 `TernaryAll` 的逐层快照（给 `output_stage`）。
pub(crate) fn evaluate(request: &Request) -> (Map<String, Value>, Option<Stages>) {
    let mut modes = Map::new();
    let mut all_stages = None;
    for mode in [
        ActivationMode::F32,
        ActivationMode::TernaryLn,
        ActivationMode::TernaryAll,
        ActivationMode::TernaryAllScale,
        ActivationMode::TernaryAllChannel,
        ActivationMode::Mixed,
        ActivationMode::Int8,
    ] {
        if request.requested.is_some_and(|wanted| wanted != mode) {
            continue;
        }
        let mut quant = Bert::load(
            request.file,
            BertConfig::new(request.hidden, request.eps, true, mode)
                .with_gptq(request.gptq_dir.clone(), request.gptq_group),
        );
        if mode == ActivationMode::TernaryAllChannel {
            quant.set_scales(request.scales.to_vec());
        }
        let capture = mode == ActivationMode::TernaryAll;
        let (real_out, real_stages, _) = quant.forward(
            request.input,
            request.mask,
            request.batch,
            request.seq,
            capture,
        );
        let (random_out, random_stages, _) = quant.forward(
            request.random_input,
            request.mask,
            request.batch,
            request.seq,
            true,
        );
        modes.insert(
            mode.key().to_string(),
            json!({
                "real_vs_f32": compare(&real_out, request.f32_out, request.mask, request.hidden),
                "real_vs_reference": compare(&real_out, request.reference, request.mask, request.hidden),
                "random_vs_f32": compare(&random_out, request.f32_random, request.mask, request.hidden),
                "sts": metrics(&sentence_vectors(&real_out, request.mask, request.batch, request.seq, request.hidden), request.pairs),
            }),
        );
        if mode == ActivationMode::TernaryAll {
            all_stages = Some((real_stages, random_stages));
        }
    }
    (modes, all_stages)
}
