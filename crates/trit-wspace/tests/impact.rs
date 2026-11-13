//! impact（多参数收益 + 交互项）与 repair-exhaustive（穷举不可修性）的测试。

use trit_wspace::exhaustive::exhaustive;
use trit_wspace::impact::{GroupKind, GroupMode, group_edits, impact};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle")
}

#[test]
fn row_group_covers_weights_and_bias() {
    let row = group_edits(&xor_net(), GroupKind::Row, 0, Some(0), GroupMode::Flip).expect("row");
    assert_eq!(row.len(), 3, "row 0 有 2 个权重 + 1 个 bias");
    let layer = group_edits(&xor_net(), GroupKind::Layer, 0, None, GroupMode::Zero).expect("layer");
    assert_eq!(layer.len(), 6, "layer 0 有 4 个权重 + 2 个 bias");
}

#[test]
fn joint_edit_reports_gain_and_individual() {
    let mut broken = xor_net();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 1,
            to: Trit::Zero,
        },
    ])
    .apply(&mut broken)
    .expect("break");
    let fixes = vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Neg,
        },
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 1,
            to: Trit::Neg,
        },
    ];
    let report = impact(&broken, &xor_oracle(), &fixes).expect("impact");
    assert_eq!(report.individual.len(), 2);
    assert!(report.before_loss > 0);
    assert_eq!(report.after_loss, 0);
    assert_eq!(report.gain, report.before_loss as isize);
    let singles: isize = report.individual.iter().map(|item| item.gain).sum();
    assert_eq!(report.synergy, report.gain - singles);
}

#[test]
fn one_break_is_found_with_one_edit() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("break");
    let report = exhaustive(&broken, &xor_oracle(), 1, 200_000).expect("exhaustive");
    assert!(report.found);
    assert_eq!(report.best_after_loss, 0);
    assert!(report.evaluated > 0);
}

#[test]
fn max_edits_zero_reports_current_state() {
    let report = exhaustive(&xor_net(), &xor_oracle(), 0, 200_000).expect("exhaustive");
    assert!(report.found);
    assert_eq!(report.evaluated, 0);
}
