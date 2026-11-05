//! 行内定位：trit 级影响回放、预算、两级读取。

use trit_wspace::{Edit, Layer, Net, Oracle, Patch, ProbeSelection, Trit, localize, localize_row};

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
fn localize_row_finds_culprit_trit() {
    let broken = broken_xor();
    let oracle = xor_oracle();
    let localization = localize_row(&broken, &oracle, 0, 0, 0, 0).expect("localize");
    assert_eq!(localization.positions, 3, "2 个权重 + 1 个偏置");
    assert_eq!(localization.probed, 3);
    assert!(!localization.sampled);
    assert_eq!(localization.stride, 1);
    assert_eq!(localization.evaluated, 6, "3 个位置 × 2 个替代值");

    let top = &localization.top[0];
    assert!(top.gain > 0);
    assert_eq!(top.after_loss, 0);
    let mut fixed = broken.clone();
    Patch::new(vec![top.edit.clone()])
        .apply(&mut fixed)
        .expect("apply");
    assert!(oracle.exact(&fixed).expect("exact"));
}

#[test]
fn localize_row_predictions_replay() {
    let net = Net::random(&[2, 3, 1], 9).expect("net");
    let oracle = Oracle::ternary(2, |input| {
        Trit::sign_of(2 * i32::from(input[0].to_i8()) + i32::from(input[1].to_i8()))
    })
    .expect("oracle");
    let localization = localize_row(&net, &oracle, 0, 1, 0, 0).expect("localize");
    let baseline = oracle.outputs_of(&net).expect("baseline");
    let before_loss = oracle.loss(&net).expect("loss");
    for impact in &localization.top {
        let mut trial = net.clone();
        Patch::new(vec![impact.edit.clone()])
            .apply(&mut trial)
            .expect("apply");
        let outputs = oracle.outputs_of(&trial).expect("outputs");
        let loss = oracle.loss(&trial).expect("loss");
        let changed = baseline
            .iter()
            .zip(&outputs)
            .filter(|(left, right)| left != right)
            .count();
        assert_eq!(impact.before_loss, before_loss);
        assert_eq!(impact.after_loss, loss);
        assert_eq!(impact.gain, before_loss as isize - loss as isize);
        assert_eq!(impact.changed, changed);
    }
}

#[test]
fn localize_row_budget_samples_positions() {
    let net = Net::random(&[6, 1], 3).expect("net");
    let oracle = Oracle::sampled_behavior(&net, 6, 4, 7).expect("oracle");
    let localization = localize_row(&net, &oracle, 0, 0, 0, 3).expect("localize");
    assert_eq!(localization.positions, 7, "6 个权重 + 1 个偏置");
    assert_eq!(localization.probed, 3);
    assert!(localization.sampled);
    assert_eq!(localization.stride, 3);
    assert_eq!(localization.evaluated, 6);
}

#[test]
fn two_stage_localize_picks_active_row_and_trit() {
    // row0 活跃、row1 全零；输出只读 row0。破坏 row0 col0。
    let net = Net::new(vec![
        Layer::new(2, 2, &[1, 1, 0, 0], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, 0], &[0]).expect("l1"),
    ])
    .expect("net");
    let oracle = Oracle::sampled_behavior(&net, 2, 16, 7).expect("oracle");
    let mut broken = net.clone();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    assert!(!oracle.exact(&broken).expect("loss"));

    let report = localize(&broken, &oracle, 2, 2, 0, 4).expect("localize");
    assert_eq!(report.probe.selection, ProbeSelection::Adaptive);
    let coordinates: Vec<(usize, usize)> =
        report.rows.iter().map(|row| (row.layer, row.row)).collect();
    assert!(coordinates.contains(&(1, 0)), "输出层必须钉住");
    assert!(coordinates.contains(&(0, 0)), "活跃的 row0 应被选");
    assert!(!coordinates.contains(&(0, 1)), "全零 row1 不应被选");

    let row0 = report
        .rows
        .iter()
        .find(|row| row.layer == 0 && row.row == 0)
        .expect("row0 localization");
    let top = &row0.top[0];
    assert_eq!(top.after_loss, 0);
    let mut fixed = broken.clone();
    Patch::new(vec![top.edit.clone()])
        .apply(&mut fixed)
        .expect("apply");
    assert!(oracle.exact(&fixed).expect("exact"));
}
