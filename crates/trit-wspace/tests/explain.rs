//! 单样本解释：翻转/修复判定、top-K 复核、越界。

use trit_wspace::{Edit, Error, Layer, Net, Oracle, Patch, Trit, explain_sample};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle")
}

fn broken_xor() -> Net {
    let mut net = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut net)
    .expect("corrupt");
    net
}

#[test]
fn explain_ranks_the_fixing_edit_first() {
    let broken = broken_xor();
    let oracle = xor_oracle();
    let explanation = explain_sample(&broken, &oracle, 0, 8).expect("explain");
    assert_eq!(explanation.index, 0);
    assert_eq!(explanation.input, vec![Trit::Neg, Trit::Neg]);
    assert_eq!(explanation.want, vec![Trit::Neg]);
    assert_eq!(explanation.got, vec![Trit::Zero]);
    assert_eq!(explanation.before_loss, 2);
    assert_eq!(explanation.fully_evaluated, 8);
    assert_eq!(explanation.candidates.len(), 8);

    let top = &explanation.candidates[0];
    assert!(top.fixes);
    assert!(top.flips);
    assert_eq!(top.after_loss, Some(0));
    assert_eq!(top.gain, Some(2));
    let mut fixed = broken.clone();
    Patch::new(vec![top.edit.clone()])
        .apply(&mut fixed)
        .expect("apply");
    assert!(oracle.exact(&fixed).expect("exact"));
}

#[test]
fn flips_match_direct_evaluation() {
    let net = Net::random(&[2, 2, 1], 11).expect("net");
    let oracle = Oracle::ternary(2, |input| {
        Trit::sign_of(2 * i32::from(input[0].to_i8()) + i32::from(input[1].to_i8()))
    })
    .expect("oracle");
    let explanation = explain_sample(&net, &oracle, 0, 0).expect("explain");
    assert_eq!(explanation.fully_evaluated, explanation.evaluated);
    assert_eq!(explanation.evaluated, 18);
    let before = explanation.got.clone();
    for impact in &explanation.candidates {
        let mut trial = net.clone();
        Patch::new(vec![impact.edit.clone()])
            .apply(&mut trial)
            .expect("apply");
        let after = trial.forward(&explanation.input).expect("forward");
        assert_eq!(impact.flips, after != before);
        assert_eq!(
            impact.fixes,
            before != explanation.want && after == explanation.want
        );
        let loss = oracle.loss(&trial).expect("loss");
        assert_eq!(impact.after_loss, Some(loss));
        assert_eq!(
            impact.gain,
            Some(explanation.before_loss as isize - loss as isize)
        );
    }
}

#[test]
fn top_limits_fully_evaluated_candidates() {
    let net = Net::random(&[2, 3, 1], 9).expect("net");
    let oracle = xor_oracle();
    let explanation = explain_sample(&net, &oracle, 0, 4).expect("explain");
    assert_eq!(explanation.evaluated, 26);
    assert_eq!(explanation.fully_evaluated, 4);
    assert_eq!(explanation.candidates.len(), 4);
}

#[test]
fn bad_sample_index_is_rejected() {
    let net = xor_net();
    let oracle = xor_oracle();
    let error = explain_sample(&net, &oracle, 99, 4).expect_err("越界应报错");
    assert!(
        matches!(error, Error::SampleIndex { index: 99, len: 4 }),
        "{error}"
    );
}
