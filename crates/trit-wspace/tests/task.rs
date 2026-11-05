//! 三值任务向量：差分、套用、多任务投票合并。

use trit_wspace::{Edit, Layer, Net, Patch, TaskVector, Trit, checksum, merge_tasks};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn tuned(base: &Net, edits: Vec<Edit>) -> Net {
    let mut net = base.clone();
    Patch::new(edits).apply(&mut net).expect("apply tune");
    net
}

fn set(layer: usize, row: usize, col: usize, to: Trit) -> Edit {
    Edit::SetTrit {
        layer,
        row,
        col,
        to,
    }
}

#[test]
fn vector_round_trip_and_base_guard() {
    let base = xor_net();
    let changed = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let vector = TaskVector::between(&base, &changed).expect("vector");
    assert_eq!(vector.base(), checksum(&base));
    assert_eq!(vector.edits().len(), 1);
    assert_eq!(vector.apply(&base).expect("apply"), changed);
    assert!(vector.apply(&changed).is_err(), "基准不一致应拒绝");
}

#[test]
fn merge_keeps_disjoint_tasks() {
    let base = xor_net();
    let left = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let right = tuned(&base, vec![set(0, 0, 1, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &left).expect("left"),
        TaskVector::between(&base, &right).expect("right"),
    ];
    let merged = merge_tasks(&base, &tasks).expect("merge");
    let layer = merged.layer(0).expect("layer");
    assert_eq!(layer.trit(0, 0), Some(Trit::Zero));
    assert_eq!(layer.trit(0, 1), Some(Trit::Zero));
    assert_eq!(merged.layer(1), base.layer(1), "未投票的行保持原样");
}

#[test]
fn merge_votes_targets_and_breaks_ties_to_base() {
    let base = xor_net();
    let zero_a = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let zero_b = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let tasks = [
        TaskVector::between(&base, &zero_a).expect("a"),
        TaskVector::between(&base, &zero_b).expect("b"),
    ];
    let merged = merge_tasks(&base, &tasks).expect("merge zeros");
    assert_eq!(merged.layer(0).expect("l0").trit(0, 0), Some(Trit::Zero));

    let zero_base = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let pos = tuned(&zero_base, vec![set(0, 0, 0, Trit::Pos)]);
    let neg = tuned(&zero_base, vec![set(0, 0, 0, Trit::Neg)]);
    let tasks = [
        TaskVector::between(&zero_base, &pos).expect("pos"),
        TaskVector::between(&zero_base, &neg).expect("neg"),
    ];
    let merged = merge_tasks(&zero_base, &tasks).expect("merge tie");
    assert_eq!(
        merged.layer(0).expect("l0").trit(0, 0),
        Some(Trit::Zero),
        "平票保留基准"
    );

    let tasks = [
        TaskVector::between(&zero_base, &pos).expect("pos"),
        TaskVector::between(&zero_base, &pos).expect("pos2"),
        TaskVector::between(&zero_base, &neg).expect("neg"),
    ];
    let merged = merge_tasks(&zero_base, &tasks).expect("merge majority");
    assert_eq!(merged.layer(0).expect("l0").trit(0, 0), Some(Trit::Pos));
}

#[test]
fn merge_rejects_foreign_base() {
    let base = xor_net();
    let other = tuned(&base, vec![set(0, 0, 0, Trit::Zero)]);
    let vector = TaskVector::between(&base, &other).expect("vector");
    assert!(merge_tasks(&other, std::slice::from_ref(&vector)).is_err());
}

#[test]
fn task_vector_requires_same_arch() {
    let base = xor_net();
    let small = Net::new(vec![Layer::new(1, 2, &[1, 1], &[0]).expect("l0")]).expect("net");
    assert!(TaskVector::between(&base, &small).is_err());
}

#[test]
fn merge_without_tasks_is_identity() {
    let base = xor_net();
    assert_eq!(merge_tasks(&base, &[]).expect("merge"), base);
}
