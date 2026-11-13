//! `repair` 的 keep 守卫：只接受不破坏保持集的补丁。

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
fn keep_guard_rejects_fixes_that_break_keep() {
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let mut broken = xor_net();
    Patch::new(vec![Edit::NegateRow { layer: 0, row: 0 }])
        .apply(&mut broken)
        .expect("apply negate");

    // keep = broken 自己的行为：任何“修好 edit”的补丁都会改变它。
    let keep = Oracle::new(
        oracle.inputs().to_vec(),
        oracle.outputs_of(&broken).expect("broken outputs"),
    )
    .expect("keep oracle");
    let options = RepairOptions {
        max_edits: 1,
        beam_width: 4,
        row_ops: true,
        keep: Some(keep),
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options).expect("repair");
    assert!(found.is_none(), "keep 会破坏时必须返回 None，不能给出解");
}

#[test]
fn keep_guard_allows_fixes_that_preserve_keep() {
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let mut broken = xor_net();
    Patch::new(vec![Edit::NegateRow { layer: 0, row: 0 }])
        .apply(&mut broken)
        .expect("apply negate");
    // keep 用 base 的行为：修复后正好恢复 base，不会破坏 keep。
    let keep = oracle.clone();
    let options = RepairOptions {
        max_edits: 1,
        beam_width: 4,
        row_ops: true,
        keep: Some(keep),
        ..RepairOptions::default()
    };
    let found = repair(&broken, &oracle, &options)
        .expect("repair")
        .expect("修复且不破坏 keep 时应找到解");
    assert_eq!(found.after_loss, 0);
}
