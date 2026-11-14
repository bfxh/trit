//! f32 位置编码反向传播的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::attention_f32_train::{TrainOptions, train_positions};

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
fn loss_decreases_and_positions_move() {
    let head = head();
    let query = vec![1.0, 0.0];
    let keys = vec![vec![1.0, 0.0], vec![-1.0, 0.0]];
    let values = keys.clone();
    let report = train_positions(
        &head,
        &query,
        &keys,
        &values,
        &TrainOptions {
            rounds: 10,
            lr: 0.5,
            target: 0,
        },
    )
    .expect("train");
    assert_eq!(report.positions.len(), 2);
    assert_eq!(report.losses.len(), 10);
    assert!(report.losses[9] < report.losses[0]);
    assert!(report.positions[0] > report.positions[1]);
    assert!(report.grad_norms.iter().all(|norm| *norm > 0.0));
}
