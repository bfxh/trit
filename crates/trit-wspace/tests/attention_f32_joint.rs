//! 多头联合训练的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::attention_f32_joint::{JointOptions, train_joint};

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
fn joint_training_makes_progress() {
    let mut heads = vec![head(), head()];
    let query = vec![1.0, 0.0];
    let keys = vec![vec![1.0, 0.0], vec![-1.0, 0.0]];
    let report = train_joint(
        &mut heads,
        &query,
        &keys,
        &JointOptions {
            rounds: 10,
            lr: 0.5,
            targets: vec![0, 1],
        },
    )
    .expect("train");
    assert_eq!(report.positions.len(), 2);
    assert_eq!(report.losses.len(), 10);
    assert!(report.losses[9] < report.losses[0]);
    assert!(report.grad_norms.iter().all(|norm| *norm > 0.0));
    // 两个头的 Q/K 都被更新过
    assert!(heads[0].query != vec![1.0, 0.0, 0.0, 1.0] || heads[0].key != vec![1.0, 0.0, 0.0, 1.0]);
}
