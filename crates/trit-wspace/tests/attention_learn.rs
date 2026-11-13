//! 从头学习三值位置编码的测试。

use trit_wspace::attention::AttentionHead;
use trit_wspace::attention_learn::{LearnOptions, learn_positions};
use trit_wspace::attention_multi::MultiHeadAttention;
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
fn learns_positions_to_match_targets() {
    // 两个头各有一张位置表，初始 dominant 都是 key 1（并列时取最后一个）。
    let multi = MultiHeadAttention::new(vec![head(), head()]).expect("multi");
    let query = vec![Trit::Pos, Trit::Zero];
    let keys = vec![vec![Trit::Pos, Trit::Zero], vec![Trit::Pos, Trit::Zero]];
    let values = keys.clone();
    let targets = vec![0, 1];
    let report = learn_positions(
        &multi,
        &query,
        &keys,
        &values,
        &targets,
        &LearnOptions {
            max_rounds: 4,
            max_positions: 2,
            max_dims: 2,
        },
    )
    .expect("learn");
    assert_eq!(report.tables.len(), 2);
    assert_eq!(report.matched, 2);
    assert!(report.converged);
    assert!(report.rounds >= 1);
}
