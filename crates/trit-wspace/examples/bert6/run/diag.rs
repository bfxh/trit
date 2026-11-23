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
    bert: Option<&Bert>,
    input: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    seed: u64,
) -> (Vec<f32>, Vec<f32>) {
    let Some(bert) = bert.filter(|_| !skip()) else {
        return (Vec::new(), Vec::new());
    };
    let random = random_like(input, seed);
    let out = bert.forward(&random, mask, batch, seq, false).0;
    (random, out)
}
pub(crate) fn quant_random(
    bert: &Bert,
    request: &Request,
    capture: bool,
) -> (Vec<f32>, Vec<Vec<f32>>) {
    if skip() || request.f32_random.is_empty() {
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

pub(crate) fn vs_f32(out: &[f32], reference: &[f32], request: &Request) -> Value {
    if out.is_empty() || reference.is_empty() {
        return Value::Null;
    }
    json!(compare(out, reference, request.mask, request.hidden))
}
