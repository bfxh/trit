//! `trit-train` 产物 → WSP1 → 读回 → 任务合并的接入测试。

use trit_train::layer::TernaryLinear;
use trit_train::quant::Ste;
use trit_wspace::{Layer, Net, TaskVector, from_wsp, merge_tasks_weighted, to_wsp};

#[test]
fn trit_train_artifact_round_trip_and_merge() {
    let mut layer = TernaryLinear::new(
        2,
        2,
        |r, c| if r == c { 0.5 } else { -0.5 },
        Ste::Identity,
        0.05,
        1.0,
    );
    for _ in 0..100 {
        layer.train_step(&[1.0, -1.0], &[1.0, 0.0]);
    }
    let rows = layer.rows();
    let cols = layer.cols();
    let flat: Vec<i8> = layer.quantized().into_iter().flatten().collect();
    let net = Net::new(vec![
        Layer::new(rows, cols, &flat, &vec![0i8; rows]).expect("layer"),
    ])
    .expect("net");

    let text = to_wsp(&net);
    let back = from_wsp(&text).expect("parse");
    assert_eq!(back.state_hash(), net.state_hash());

    let zero = TaskVector::between(&net, &back).expect("vector");
    assert!(zero.edits().is_empty());
    let merged = merge_tasks_weighted(&net, &[zero], &[1.0]).expect("merge");
    assert_eq!(merged.state_hash(), net.state_hash());
}
