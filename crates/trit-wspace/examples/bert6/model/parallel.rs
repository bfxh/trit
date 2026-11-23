//! 按 batch 并行的前向：每句独立，逐句结果与串行完全一致。
//!
//! 只在 `capture == false`（全量评测）时启用；逐层快照仍走串行路径。
//! `TRIT_THREADS`：正整数或 `auto`；默认 1，保持 CI 行为不变。

use std::thread;

use super::Bert;

pub(super) fn threads() -> usize {
    match std::env::var("TRIT_THREADS").ok().as_deref() {
        Some("auto") => thread::available_parallelism().map_or(1, |n| n.get()),
        Some(value) => value.trim().parse().unwrap_or(1),
        None => 1,
    }
}

pub(super) fn forward(
    bert: &Bert,
    input: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    threads: usize,
) -> (Vec<f32>, Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let stride = seq * bert.hidden;
    let workers = threads.min(batch).max(1);
    let chunk_items = batch.div_ceil(workers);
    let mut output = vec![0.0f32; batch * stride];
    thread::scope(|scope| {
        for (chunk, out_chunk) in output.chunks_mut(chunk_items * stride).enumerate() {
            let start = chunk * chunk_items;
            let items = out_chunk.len() / stride;
            let in_chunk = &input[start * stride..(start + items) * stride];
            let mask_chunk = &mask[start * seq..(start + items) * seq];
            scope.spawn(move || {
                for local in 0..items {
                    let mut current = in_chunk[local * stride..(local + 1) * stride].to_vec();
                    let item_mask = &mask_chunk[local * seq..(local + 1) * seq];
                    for (index, layer) in bert.layers.iter().enumerate() {
                        let layer_scales = bert.scales.as_ref().map(|all| &all[index]);
                        layer.forward(
                            &mut current,
                            item_mask,
                            seq,
                            bert.hidden,
                            bert.heads,
                            bert.head_dim,
                            layer_scales,
                        );
                    }
                    out_chunk[local * stride..(local + 1) * stride].copy_from_slice(&current);
                }
            });
        }
    });
    (output, Vec::new(), Vec::new())
}
