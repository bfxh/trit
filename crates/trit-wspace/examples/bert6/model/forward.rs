//! 串行前向（逐层快照路径也走这里）。

use super::Bert;

pub(super) fn run(
    bert: &Bert,
    input: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    capture: bool,
) -> (Vec<f32>, Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut output = Vec::with_capacity(batch * seq * bert.hidden);
    let (mut snapshots, mut layer_outputs) = (Vec::new(), Vec::new());
    if capture {
        snapshots = vec![Vec::new(); bert.layers.len()];
        layer_outputs = vec![Vec::new(); bert.layers.len()];
    }
    for (item, chunk) in input.chunks(seq * bert.hidden).enumerate() {
        let mut current = chunk.to_vec();
        let item_mask = &mask[item * seq..(item + 1) * seq];
        for (index, layer) in bert.layers.iter().enumerate() {
            let layer_scales = bert.scales.as_ref().map(|all| &all[index]);
            let captured = layer.forward(
                &mut current,
                item_mask,
                seq,
                bert.hidden,
                bert.heads,
                bert.head_dim,
                layer_scales,
            );
            if capture {
                snapshots[index].extend_from_slice(&captured);
                layer_outputs[index].extend_from_slice(&current);
            }
        }
        output.extend_from_slice(&current);
    }
    (output, snapshots, layer_outputs)
}
