//! SVD 残差侧车的 rank 扫描（从 `run.rs` 拆出守住行数预算）。

use crate::activation::{ActivationMode, LayerScales};
use crate::model::{Bert, BertConfig};
use crate::report::compare;
use crate::sts::{Pair, metrics, sentence_vectors};
use serde_json::{Value, json};

pub(crate) struct Request<'a> {
    pub(crate) file: &'a str,
    pub(crate) gptq_dir: Option<String>,
    pub(crate) gptq_group: usize,
    pub(crate) mode: ActivationMode,
    pub(crate) hidden: usize,
    pub(crate) eps: f32,
    pub(crate) input: &'a [f32],
    pub(crate) mask: &'a [u8],
    pub(crate) batch: usize,
    pub(crate) seq: usize,
    pub(crate) pairs: &'a [Pair],
    pub(crate) f32_out: &'a [f32],
    pub(crate) scales: &'a [LayerScales],
}

/// 一次构建最大 rank 的侧车，再按前缀收缩逐个 rank 跑前向；`None` 返回 `null`。
pub(crate) fn run(ranks: Option<Vec<usize>>, request: &Request) -> Value {
    let Some(ranks) = ranks.filter(|ranks| !ranks.is_empty()) else {
        return json!(null);
    };
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let started = std::time::Instant::now();
    let mut quant = Bert::load(
        request.file,
        BertConfig::new(request.hidden, request.eps, true, request.mode)
            .with_gptq(request.gptq_dir.clone(), request.gptq_group)
            .with_sidecar_svd(max_rank),
    );
    if request.mode == ActivationMode::TernaryAllChannel {
        quant.set_scales(request.scales.to_vec());
    }
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let dense_params = quant.weight_params();
    let mut entries = Vec::with_capacity(ranks.len());
    for rank in ranks {
        quant.set_sidecar_rank(rank);
        let (real_out, _, _) = quant.forward(
            request.input,
            request.mask,
            request.batch,
            request.seq,
            false,
        );
        let params = quant.sidecar_params();
        entries.push(json!({
            "rank": rank,
            "sidecar_params": params,
            "dense_params": dense_params,
            "param_ratio": params as f64 / dense_params as f64,
            "build_ms": build_ms,
            "real_vs_f32": compare(&real_out, request.f32_out, request.mask, request.hidden),
            "sts": metrics(
                &sentence_vectors(&real_out, request.mask, request.batch, request.seq, request.hidden),
                request.pairs,
            ),
        }));
    }
    json!(entries)
}
