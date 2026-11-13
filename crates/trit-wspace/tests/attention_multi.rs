//! 多头三值注意力的测试。

use trit_wspace::attention::AttentionHead;
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
fn multi_head_returns_one_trace_per_head() {
    let multi = MultiHeadAttention::new(vec![head(), head()]).expect("multi");
    let query = vec![Trit::Pos, Trit::Zero];
    let keys = vec![vec![Trit::Pos, Trit::Zero], vec![Trit::Neg, Trit::Zero]];
    let values = keys.clone();
    let traces = multi.forward(&query, &keys, &values, &[]).expect("forward");
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[0].dominant, 0);
    assert_eq!(traces[1].dominant, 0);
}

#[test]
fn position_bias_changes_dominant() {
    let multi = MultiHeadAttention::new(vec![head()]).expect("multi");
    let query = vec![Trit::Pos, Trit::Zero];
    let keys = vec![vec![Trit::Pos, Trit::Zero], vec![Trit::Neg, Trit::Zero]];
    let values = keys.clone();
    let traces = multi
        .forward(&query, &keys, &values, &[0, 100])
        .expect("forward");
    assert_eq!(traces[0].dominant, 1, "位置偏置应把注意力拉到 key 1");
    assert!(traces[0].weights[1] > traces[0].weights[0]);
}
