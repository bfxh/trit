//! 行内定位：trit 级影响回放、预算、两级读取。

use trit_wspace::{
    Edit, Layer, LocalizeSelection, Net, Oracle, Patch, ProbeSelection, Trit, localize,
    localize_row,
};

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
    assert_eq!(localization.selection, LocalizeSelection::All);
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

fn linear_oracle() -> Oracle {
    Oracle::new(
        vec![
            vec![Trit::Pos, Trit::Pos],
            vec![Trit::Pos, Trit::Neg],
            vec![Trit::Neg, Trit::Pos],
            vec![Trit::Neg, Trit::Neg],
        ],
        vec![
            vec![Trit::Pos],
            vec![Trit::Zero],
            vec![Trit::Zero],
            vec![Trit::Neg],
        ],
    )
    .expect("oracle")
}

#[test]
fn adaptive_selection_finds_trit_stride_would_miss() {
    // 权重 [1,1] 偏置 0：positions=3，固定步长 2 只会看 {col0, bias}，漏掉 col1。
    let net = Net::new(vec![Layer::new(1, 2, &[1, 1], &[0]).expect("l0")]).expect("net");
    let oracle = linear_oracle();
    let mut broken = net.clone();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 1,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");

    let localization = localize_row(&broken, &oracle, 0, 0, 0, 2).expect("localize");
    assert_eq!(localization.positions, 3, "2 个权重 + 1 个偏置");
    assert_eq!(localization.probed, 2);
    assert!(localization.sampled);
    assert_eq!(localization.selection, LocalizeSelection::Adaptive);
    assert_eq!(localization.stride, 0, "自适应选点没有固定步长");
    assert_eq!(localization.evaluated, 4);

    let fix = localization
        .top
        .iter()
        .find(|impact| impact.after_loss == 0)
        .expect("应能定位到 col1");
    assert_eq!(
        fix.edit,
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 1,
            to: Trit::Pos,
        }
    );
}

#[test]
fn adaptive_selection_skips_never_active_column() {
    // col0 的输入恒为 Zero；预算 1 必须选真正活跃的 col1，而不是按序号选 col0。
    let net = Net::new(vec![Layer::new(1, 2, &[1, 1], &[0]).expect("l0")]).expect("net");
    let oracle = Oracle::new(
        vec![
            vec![Trit::Zero, Trit::Pos],
            vec![Trit::Zero, Trit::Pos],
            vec![Trit::Zero, Trit::Neg],
            vec![Trit::Zero, Trit::Neg],
        ],
        vec![
            vec![Trit::Pos],
            vec![Trit::Pos],
            vec![Trit::Neg],
            vec![Trit::Neg],
        ],
    )
    .expect("oracle");
    let localization = localize_row(&net, &oracle, 0, 0, 0, 1).expect("localize");
    assert_eq!(localization.probed, 1);
    assert_eq!(localization.selection, LocalizeSelection::Adaptive);
    assert!(
        localization
            .top
            .iter()
            .all(|impact| matches!(&impact.edit, Edit::SetTrit { col: 1, .. })),
        "只应探测活跃的 col1"
    );
}

#[test]
fn adaptive_selection_uses_previous_layer_activation() {
    // layer0 把 input col0 映到隐藏 r1；layer1 的 col0 恒为 0、col1 活跃。
    // 若错误地用原始输入打分，两列同分（序号选 col0），就会探错位置。
    let net = Net::new(vec![
        Layer::new(2, 2, &[0, 0, 1, 0], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, 1], &[0]).expect("l1"),
    ])
    .expect("net");
    let oracle = Oracle::new(
        vec![
            vec![Trit::Pos, Trit::Pos],
            vec![Trit::Pos, Trit::Neg],
            vec![Trit::Neg, Trit::Pos],
            vec![Trit::Neg, Trit::Neg],
        ],
        vec![
            vec![Trit::Pos],
            vec![Trit::Pos],
            vec![Trit::Neg],
            vec![Trit::Neg],
        ],
    )
    .expect("oracle");
    let localization = localize_row(&net, &oracle, 1, 0, 0, 1).expect("localize");
    assert_eq!(localization.probed, 1);
    assert_eq!(localization.selection, LocalizeSelection::Adaptive);
    assert!(
        localization.top.iter().all(|impact| matches!(
            &impact.edit,
            Edit::SetTrit {
                layer: 1,
                col: 1,
                ..
            }
        )),
        "隐藏层应按上一层激活选 col1"
    );
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
