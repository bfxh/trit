//! 每头独立位置训练的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::attention_f32_independent::{IndependentOptions, train_independent};

fn head() -> F32Attention {
    F32Attention::new(
        vec![1.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 1.0],
        2,
        2,
    )
    .expect("head")
}

#[test]
fn independent_positions_match_targets() {
    let heads = vec![head(), head()];
    let query = vec![1.0, 0.0];
    let keys = vec![vec![1.0, 0.0], vec![-1.0, 0.0]];
    let values = keys.clone();
    let report = train_independent(
        &heads,
        &query,
        &keys,
        &values,
        &IndependentOptions {
            rounds: 50,
            lr: 0.5,
            targets: vec![0, 1],
        },
    )
    .expect("train");
    assert_eq!(report.total, 2);
    assert_eq!(report.matched, 2);
    assert!(report.heads.iter().all(|head| head.matched));
    assert!(
        report
            .heads
            .iter()
            .all(|head| head.loss_after <= head.loss_before)
    );
}
