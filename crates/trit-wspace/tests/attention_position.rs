//! 三值位置编码的测试。

use trit_wspace::attention::AttentionHead;
use trit_wspace::attention_multi::MultiHeadAttention;
use trit_wspace::attention_position::{PositionEncoding, forward_with_encodings};
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
fn apply_adds_trits_elementwise() {
    let encoding = PositionEncoding::new(vec![vec![Trit::Pos, Trit::Zero]]).expect("encoding");
    let keys = vec![vec![Trit::Neg, Trit::Zero]];
    let encoded = encoding.apply(&keys);
    assert_eq!(encoded, vec![vec![Trit::Zero, Trit::Zero]]);
}

#[test]
fn encoding_changes_dominant() {
    let multi = MultiHeadAttention::new(vec![head()]).expect("multi");
    let query = vec![Trit::Pos, Trit::Pos];
    let keys = vec![vec![Trit::Neg, Trit::Zero], vec![Trit::Neg, Trit::Neg]];
    let values = keys.clone();
    let plain = forward_with_encodings(&multi, &query, &keys, &values, &[]).expect("plain");
    assert_eq!(plain[0].dominant, 0, "plain: -1 > -2");
    let encoding = PositionEncoding::new(vec![
        vec![Trit::Zero, Trit::Zero],
        vec![Trit::Pos, Trit::Pos],
    ])
    .expect("encoding");
    let biased =
        forward_with_encodings(&multi, &query, &keys, &values, &[encoding]).expect("biased");
    assert_eq!(
        biased[0].dominant, 1,
        "位置编码把 key 1 拉到 0，超过 key 0 的 -1"
    );
    assert!(biased[0].weights[1] > biased[0].weights[0]);
}
