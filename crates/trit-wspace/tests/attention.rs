//! 三值注意力的性质测试。

use trit_wspace::attention::AttentionHead;
use trit_wspace::{Layer, Trit};

fn head() -> AttentionHead {
    AttentionHead::new(
        Layer::new(2, 2, &[1, 0, 0, 1], &[0, 0]).expect("q"),
        Layer::new(2, 2, &[1, 0, 0, 1], &[0, 0]).expect("k"),
        Layer::new(2, 2, &[1, 0, 0, 1], &[0, 0]).expect("v"),
    )
    .expect("head")
}

#[test]
fn weights_sum_to_one_and_pick_dominant() {
    let head = head();
    let query = vec![Trit::Pos, Trit::Zero];
    let keys = vec![vec![Trit::Pos, Trit::Zero], vec![Trit::Neg, Trit::Zero]];
    let values = vec![vec![Trit::Pos, Trit::Zero], vec![Trit::Zero, Trit::Pos]];
    let trace = head.forward(&query, &keys, &values).expect("forward");
    assert_eq!(trace.scores, vec![1, -1]);
    let sum: f32 = trace.weights.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6, "weights sum {sum}");
    assert_eq!(trace.dominant, 0, "q·k0 应更大");
    assert!(trace.weights[0] > trace.weights[1]);
    assert_eq!(trace.output.len(), 2);
}

#[test]
fn equal_scores_give_uniform_weights() {
    let head = head();
    let query = vec![Trit::Zero, Trit::Zero];
    let keys = vec![vec![Trit::Zero, Trit::Zero], vec![Trit::Zero, Trit::Zero]];
    let values = vec![vec![Trit::Pos, Trit::Zero], vec![Trit::Zero, Trit::Pos]];
    let trace = head.forward(&query, &keys, &values).expect("forward");
    assert_eq!(trace.scores, vec![0, 0]);
    assert!((trace.weights[0] - 0.5).abs() < 1e-6);
    assert!((trace.weights[1] - 0.5).abs() < 1e-6);
}
