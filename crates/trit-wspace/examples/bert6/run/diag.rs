//! 诊断前向：`TRIT_SKIP_DIAG=1` 时整段跳过，不影响 STS/字节账。
use crate::model::Bert;
use crate::report::compare;
use crate::stats::random_like;
use serde_json::{Value, json};

use super::modes::Request;

pub(crate) fn skip() -> bool {
    std::env::var_os("TRIT_SKIP_DIAG").is_some()
}

pub(crate) fn f32_inputs(
    bert: &Bert,
    input: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    seed: u64,
) -> (Vec<f32>, Vec<f32>) {
    if skip() {
        return (Vec::new(), Vec::new());
    }
    let random = random_like(input, seed);
    let out = bert.forward(&random, mask, batch, seq, false).0;
    (random, out)
}

pub(crate) fn quant_random(
    bert: &Bert,
    request: &Request,
    capture: bool,
) -> (Vec<f32>, Vec<Vec<f32>>) {
    if skip() {
        return (Vec::new(), Vec::new());
    }
    let (out, stages, _) = bert.forward(
        request.random_input,
        request.mask,
        request.batch,
        request.seq,
        capture,
    );
    (out, stages)
}

pub(crate) fn random_vs_f32(out: &[f32], request: &Request) -> Value {
    if out.is_empty() {
        Value::Null
    } else {
        json!(compare(
            out,
            request.f32_random,
            request.mask,
            request.hidden
        ))
    }
}
