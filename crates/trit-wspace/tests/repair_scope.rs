//! `repair` 的候选限制：scope 只允许指定坐标时，束搜索不会越界。

use trit_wspace::repair::{RepairOptions, repair};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
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
    .expect("apply break");
    net
}

#[test]
fn scoped_repair_only_touches_allowed_coordinates() {
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let broken = broken_xor();
    assert!(oracle.loss(&broken).expect("loss") > 0);

    let scope = vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Neg,
        },
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
    ];
    let options = RepairOptions {
        max_edits: 1,
        beam_width: 4,
        scope: Some(scope),
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options)
        .expect("repair")
        .expect("scoped repair should find the fix");
    assert_eq!(found.patch.len(), 1);
    assert_eq!(found.before_loss, oracle.loss(&broken).expect("loss"));
    assert_eq!(found.after_loss, 0);
    for edit in found.patch.edits() {
        assert!(
            matches!(
                edit,
                Edit::SetTrit {
                    layer: 0,
                    row: 0,
                    col: 0,
                    ..
                }
            ),
            "scoped repair 越界：{edit:?}"
        );
    }
}

#[test]
fn scoped_repair_returns_none_when_scope_cannot_fix() {
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let broken = broken_xor();
    // 只允许改一层里与错误无关的另一行。
    let scope = vec![
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 1,
            to: Trit::Neg,
        },
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 1,
            to: Trit::Pos,
        },
    ];
    let options = RepairOptions {
        max_edits: 1,
        beam_width: 4,
        scope: Some(scope),
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options).expect("repair");
    assert!(found.is_none(), "scope 内无解时必须返回 None，不能越界改");
}

#[test]
fn unscoped_repair_still_finds_the_fix() {
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let broken = broken_xor();
    let options = RepairOptions {
        max_edits: 1,
        beam_width: 4,
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options)
        .expect("repair")
        .expect("unscoped repair should find the fix");
    assert_eq!(found.after_loss, 0);
}
