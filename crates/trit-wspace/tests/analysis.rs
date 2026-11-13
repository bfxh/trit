//! trace / distribution / perturb 的性质测试。

use trit_wspace::distribution::distribution;
use trit_wspace::perturb::{PerturbMode, perturb};
use trit_wspace::trace::trace;
use trit_wspace::{Layer, Net, Oracle, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle")
}

#[test]
fn trace_layers_match_forward() {
    let net = xor_net();
    let oracle = xor_oracle();
    let input = &oracle.inputs()[0];
    let report = trace(&net, input).expect("trace");
    assert_eq!(report.output, net.forward(input).expect("forward"));
    assert_eq!(report.layers.len(), 2);
    for layer in &report.layers {
        for row in &layer.rows {
            assert_eq!(row.out, Trit::sign_of(row.acc));
        }
    }
}

#[test]
fn distribution_counts_inputs_and_outputs() {
    let oracle = xor_oracle();
    let dist = distribution(&oracle);
    assert_eq!(dist.samples, 4);
    assert_eq!(dist.inputs.len(), 2);
    assert!(
        dist.inputs
            .iter()
            .all(|coord| coord.neg + coord.zero + coord.pos == 4)
    );
    assert_eq!(dist.unique_inputs, 4);
    assert_eq!(
        dist.outputs.iter().map(|class| class.count).sum::<usize>(),
        4
    );
}

#[test]
fn perturb_flips_some_samples() {
    let net = xor_net();
    let oracle = xor_oracle();
    let report = perturb(&net, &oracle, PerturbMode::Flip).expect("perturb");
    assert_eq!(report.samples, 4);
    assert_eq!(report.width, 2);
    assert_eq!(report.perturbations, 8);
    assert_eq!(report.per_position.len(), 2);
    assert!(report.flipped > 0);
    assert!(report.lost > 0);
}
