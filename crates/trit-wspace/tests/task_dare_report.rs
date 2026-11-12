//! DARE 路径的投票报告：rate=0 与普通合并一致、rate>0 报告仍来自投票阶段。

use trit_wspace::{
    Edit, Layer, Net, Patch, TaskVector, Trit, merge_tasks_dare_report,
    merge_tasks_weighted_dare_report, merge_tasks_weighted_per_task_dare_report,
    merge_tasks_weighted_report,
};

fn trit(layer: usize, row: usize, col: usize, to: Trit) -> Edit {
    Edit::SetTrit {
        layer,
        row,
        col,
        to,
    }
}

fn bias(layer: usize, row: usize, to: Trit) -> Edit {
    Edit::SetBias { layer, row, to }
}

fn base() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[1, -1, 0, 1], &[1, -1]).unwrap(),
        Layer::new(1, 2, &[1, 1], &[-1]).unwrap(),
    ])
    .unwrap()
}

fn tuned(base: &Net, edits: Vec<Edit>) -> Net {
    let mut net = base.clone();
    Patch::new(edits).apply(&mut net).unwrap();
    net
}

fn tasks() -> (Net, Vec<TaskVector>) {
    let base = base();
    // (0,1,0) 基准是 0：A 投 Pos、B 投 Neg，权重 1:1 平票、1:3 让 Neg 胜出。
    let a = tuned(
        &base,
        vec![
            trit(0, 0, 0, Trit::Neg),
            trit(0, 1, 0, Trit::Pos),
            trit(0, 1, 1, Trit::Pos),
            bias(1, 0, Trit::Pos),
        ],
    );
    let b = tuned(
        &base,
        vec![
            trit(0, 0, 0, Trit::Neg),
            trit(0, 1, 0, Trit::Neg),
            bias(1, 0, Trit::Pos),
        ],
    );
    let vectors = vec![
        TaskVector::between(&base, &a).unwrap(),
        TaskVector::between(&base, &b).unwrap(),
    ];
    (base, vectors)
}

#[test]
fn rate_zero_matches_plain_report_and_net() {
    let (base, tasks) = tasks();
    let weights = [1.0, 1.0];
    let (plain, plain_report) = merge_tasks_weighted_report(&base, &tasks, &weights).unwrap();
    let (dare, dare_report) =
        merge_tasks_weighted_dare_report(&base, &tasks, &weights, 0.0, 7).unwrap();
    assert_eq!(plain.state_hash(), dare.state_hash());
    assert_eq!(plain_report, dare_report);
}

#[test]
fn report_comes_from_the_vote_stage() {
    let (base, tasks) = tasks();
    let (_, even) = merge_tasks_weighted_report(&base, &tasks, &[1.0, 1.0]).unwrap();
    let (_, skewed) = merge_tasks_weighted_report(&base, &tasks, &[1.0, 3.0]).unwrap();
    let (_, dare_even) =
        merge_tasks_weighted_dare_report(&base, &tasks, &[1.0, 1.0], 0.5, 7).unwrap();
    let (_, dare_skewed) =
        merge_tasks_weighted_dare_report(&base, &tasks, &[1.0, 3.0], 0.5, 7).unwrap();
    // 投票字段与普通合并一致；dropped 是 DARE 稀疏化新增的计数。
    assert_eq!(dare_even.changed(), even.changed());
    assert_eq!(dare_skewed.changed(), skewed.changed());
    assert!(dare_even.dropped() > 0);
    let (_, rate_zero) =
        merge_tasks_weighted_dare_report(&base, &tasks, &[1.0, 3.0], 0.0, 7).unwrap();
    assert_eq!(rate_zero.dropped(), 0);
}

#[test]
fn equal_weight_report_matches_weighted_ones() {
    let (base, tasks) = tasks();
    let (equal, equal_report) = merge_tasks_dare_report(&base, &tasks, 0.5, 7).unwrap();
    let (weighted, weighted_report) =
        merge_tasks_weighted_dare_report(&base, &tasks, &[1.0, 1.0], 0.5, 7).unwrap();
    assert_eq!(equal.state_hash(), weighted.state_hash());
    assert_eq!(equal_report, weighted_report);
}

#[test]
fn per_task_report_is_not_empty() {
    let (base, tasks) = tasks();
    let (_, report) =
        merge_tasks_weighted_per_task_dare_report(&base, &tasks, &[1.0, 1.0], 0.5, 7).unwrap();
    assert_eq!(report.tasks(), 2);
    assert!(report.voted() > 0);
    assert!(report.dropped() > 0);
}
