//! `repair` 的行级原语：整行取反能撤销一次取反。

use trit_wspace::repair::{RepairOptions, repair};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

#[test]
fn row_ops_can_undo_a_row_negation() {
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let mut broken = xor_net();
    Patch::new(vec![Edit::NegateRow { layer: 0, row: 0 }])
        .apply(&mut broken)
        .expect("apply negate");
    assert!(oracle.loss(&broken).expect("loss") > 0);

    let options = RepairOptions {
        max_edits: 1,
        beam_width: 4,
        row_ops: true,
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options)
        .expect("repair")
        .expect("row_ops candidates should undo the negation");
    assert_eq!(found.patch.len(), 1);
    assert!(
        matches!(found.patch.edits()[0], Edit::NegateRow { layer: 0, row: 0 }),
        "应找到整行取反：{:?}",
        found.patch.edits()
    );
}
