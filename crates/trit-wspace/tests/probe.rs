//! 行级探针：消融效果回放、预算抽样、采样验证集。

use std::collections::HashSet;

use trit_wspace::{
    Edit, Layer, Net, Oracle, Patch, ProbeSelection, Trit, probe_rows, probe_rows_adaptive,
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

#[test]
fn output_row_ablation_changes_everything() {
    let net = xor_net();
    let oracle = xor_oracle();
    let report = probe_rows(&net, &oracle, 0, 0).expect("probe");
    assert_eq!(report.rows, 3, "2 个隐层行 + 1 个输出行");
    assert_eq!(report.probed_rows, 3);
    assert!(!report.sampled);
    assert_eq!(report.selection, ProbeSelection::All);
    assert_eq!(report.stride, 1);
    assert_eq!(report.before_loss, 0);
    assert_eq!(report.samples, 4);

    let output = report
        .top
        .iter()
        .find(|impact| impact.layer == 1 && impact.row == 0)
        .expect("output row");
    assert_eq!(output.trits, 3, "2 个权重 + 1 个偏置");
    assert_eq!(output.zero.changed, 4);
    assert_eq!(output.zero.after_loss, 4, "全零输出对 4 个样本都错");
    assert_eq!(output.zero.gain, -4);
}

#[test]
fn hidden_row_ablation_is_measured() {
    let net = xor_net();
    let report = probe_rows(&net, &xor_oracle(), 0, 0).expect("probe");
    let hidden = report
        .top
        .iter()
        .find(|impact| impact.layer == 0 && impact.row == 0)
        .expect("hidden row");
    assert!(hidden.zero.changed > 0);
    assert!(hidden.negate.changed > 0);
}

#[test]
fn probe_effects_replay_exactly() {
    let net = Net::random(&[2, 3, 1], 9).expect("net");
    let oracle = Oracle::ternary(2, |input| {
        Trit::sign_of(2 * i32::from(input[0].to_i8()) + i32::from(input[1].to_i8()))
    })
    .expect("oracle");
    let report = probe_rows(&net, &oracle, 0, 0).expect("probe");
    let baseline = oracle.outputs_of(&net).expect("baseline");
    let baseline_set: HashSet<usize> = baseline
        .iter()
        .zip(oracle.outputs())
        .enumerate()
        .filter_map(|(index, (got, want))| (got != want).then_some(index))
        .collect();

    for impact in &report.top {
        let cols = net.layer(impact.layer).expect("layer").cols();
        let mut zero_edits = Vec::new();
        for col in 0..cols {
            zero_edits.push(Edit::SetTrit {
                layer: impact.layer,
                row: impact.row,
                col,
                to: Trit::Zero,
            });
        }
        zero_edits.push(Edit::SetBias {
            layer: impact.layer,
            row: impact.row,
            to: Trit::Zero,
        });
        let mut zeroed = net.clone();
        Patch::new(zero_edits).apply(&mut zeroed).expect("zero");
        assert_effect(
            &zeroed,
            &oracle,
            &baseline,
            &baseline_set,
            report.before_loss,
            &impact.zero,
        );

        let mut negated = net.clone();
        Patch::new(vec![Edit::NegateRow {
            layer: impact.layer,
            row: impact.row,
        }])
        .apply(&mut negated)
        .expect("negate");
        assert_effect(
            &negated,
            &oracle,
            &baseline,
            &baseline_set,
            report.before_loss,
            &impact.negate,
        );
    }
}

fn assert_effect(
    trial: &Net,
    oracle: &Oracle,
    baseline: &[Vec<Trit>],
    baseline_set: &HashSet<usize>,
    before_loss: usize,
    effect: &trit_wspace::RowEffect,
) {
    let outputs = oracle.outputs_of(trial).expect("outputs");
    let mismatches: Vec<usize> = outputs
        .iter()
        .zip(oracle.outputs())
        .enumerate()
        .filter_map(|(index, (got, want))| (got != want).then_some(index))
        .collect();
    let after_set: HashSet<usize> = mismatches.iter().copied().collect();
    let changed = baseline
        .iter()
        .zip(&outputs)
        .filter(|(left, right)| left != right)
        .count();
    let fixed = baseline_set
        .iter()
        .filter(|index| !after_set.contains(index))
        .count();
    let broken = mismatches
        .iter()
        .filter(|index| !baseline_set.contains(index))
        .count();
    assert_eq!(effect.changed, changed);
    assert_eq!(effect.after_loss, mismatches.len());
    assert_eq!(effect.fixed, fixed);
    assert_eq!(effect.broken, broken);
    assert_eq!(
        effect.gain,
        before_loss as isize - mismatches.len() as isize
    );
}

#[test]
fn budget_samples_evenly() {
    let net = Net::random(&[2, 5, 4], 3).expect("net");
    let oracle = Oracle::sampled_behavior(&net, 2, 4, 7).expect("oracle");
    let report = probe_rows(&net, &oracle, 3, 0).expect("probe");
    assert_eq!(report.rows, 9, "5 + 4");
    assert_eq!(report.probed_rows, 3);
    assert!(report.sampled);
    assert_eq!(report.selection, ProbeSelection::Uniform);
    assert_eq!(report.stride, 3);
    let coordinates: Vec<(usize, usize)> = report
        .top
        .iter()
        .map(|impact| (impact.layer, impact.row))
        .collect();
    for expected in [(0, 0), (0, 3), (1, 1)] {
        assert!(coordinates.contains(&expected), "缺少 {expected:?}");
    }
}

#[test]
fn sampled_behavior_is_deterministic_and_exact() {
    let net = Net::random(&[3, 2, 1], 5).expect("net");
    let first = Oracle::sampled_behavior(&net, 3, 8, 42).expect("first");
    let second = Oracle::sampled_behavior(&net, 3, 8, 42).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.len(), 8);
    assert!(first.exact(&net).expect("exact"), "期望输出就是当前行为");
    let other = Oracle::sampled_behavior(&net, 3, 8, 43).expect("other");
    assert_ne!(first.inputs(), other.inputs());
    assert!(Oracle::sampled_behavior(&net, 2, 8, 42).is_err());
    assert!(Oracle::sampled_behavior(&net, 3, 0, 42).is_err());
}

fn layered_net() -> Net {
    // 隐层 r0 全零（不工作），r1/r2 活跃；输出层 1 行。
    Net::new(vec![
        Layer::new(3, 2, &[0, 0, 1, -1, -1, 1], &[0, 0, 0]).expect("l0"),
        Layer::new(1, 3, &[1, 1, 1], &[0]).expect("l1"),
    ])
    .expect("net")
}

#[test]
fn adaptive_prefers_active_rows() {
    let net = layered_net();
    let oracle = xor_oracle();
    let report = probe_rows_adaptive(&net, &oracle, 2, 0).expect("probe");
    assert_eq!(report.selection, ProbeSelection::Adaptive);
    assert!(report.sampled);
    assert_eq!(report.stride, 0, "自适应抽样没有固定步长");
    assert_eq!(report.probed_rows, 2);
    assert!(report.pilot_rows >= 1);
    assert!(report.pilot_kept >= 1);
    let coordinates: Vec<(usize, usize)> = report
        .top
        .iter()
        .map(|impact| (impact.layer, impact.row))
        .collect();
    assert!(coordinates.contains(&(1, 0)), "输出层必须钉住");
    assert!(!coordinates.contains(&(0, 0)), "全零行不应被选");
    assert!(
        coordinates.contains(&(0, 1)) || coordinates.contains(&(0, 2)),
        "应选到活跃的隐层行"
    );
}

#[test]
fn adaptive_pins_output_row_with_budget_one() {
    let net = layered_net();
    let report = probe_rows_adaptive(&net, &xor_oracle(), 1, 0).expect("probe");
    assert_eq!(report.probed_rows, 1);
    assert_eq!(report.pilot_rows, 0, "预算 1 只够输出行，不做 pilot");
    assert_eq!(report.top[0].layer, 1);
    assert_eq!(report.top[0].row, 0);
}

#[test]
fn adaptive_without_budget_probes_all() {
    let net = layered_net();
    let report = probe_rows_adaptive(&net, &xor_oracle(), 0, 0).expect("probe");
    assert_eq!(report.selection, ProbeSelection::All);
    assert!(!report.sampled);
    assert_eq!(report.probed_rows, report.rows);
    assert_eq!(report.pilot_rows, 0);
}

#[test]
fn adaptive_uses_activity_not_just_nonzero_weights() {
    // r0 权重非零但在 oracle 样本上恒不激活（a+b=0）；r1 活跃。
    // 只看 nonzero 权重会打平并选中 r0；只有真的统计活跃度才会选 r1。
    let net = Net::new(vec![
        Layer::new(2, 2, &[1, 1, -1, 1], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, 1], &[0]).expect("l1"),
    ])
    .expect("net");
    let oracle = Oracle::new(
        vec![vec![Trit::Neg, Trit::Pos], vec![Trit::Pos, Trit::Neg]],
        vec![vec![Trit::Pos], vec![Trit::Neg]],
    )
    .expect("oracle");
    let report = probe_rows_adaptive(&net, &oracle, 2, 0).expect("probe");
    let coordinates: Vec<(usize, usize)> = report
        .top
        .iter()
        .map(|impact| (impact.layer, impact.row))
        .collect();
    assert!(coordinates.contains(&(1, 0)), "输出层必须钉住");
    assert!(
        !coordinates.contains(&(0, 0)),
        "权重非零但从不激活的行不应被选"
    );
    assert!(coordinates.contains(&(0, 1)), "应选活跃行");
    assert_eq!(report.pilot_kept, 1, "activity 打分必须让 pilot 命中活跃行");
}

#[test]
fn pilot_drops_inert_high_activity_row() {
    // layer0 r1 静态最活跃，但下游 layer1 r0 只读 h0_0 ⇒ r1 消融无效果。
    // pilot 应把它丢掉，把预算给真正承重的 layer1 r0 / layer0 r0。
    let net = Net::new(vec![
        Layer::new(2, 2, &[1, 0, 1, 1], &[0, 1]).expect("l0"),
        Layer::new(2, 2, &[1, 0, 0, 0], &[0, 0]).expect("l1"),
        Layer::new(1, 2, &[1, 1], &[0]).expect("l2"),
    ])
    .expect("net");
    // 构造 oracle 让 layer0 r1 严格比 r0 / layer1 r0 更活跃。
    let inputs = vec![
        vec![Trit::Zero, Trit::Zero],
        vec![Trit::Zero, Trit::Pos],
        vec![Trit::Pos, Trit::Zero],
        vec![Trit::Pos, Trit::Pos],
    ];
    let outputs = inputs
        .iter()
        .map(|row| net.forward(row).expect("forward"))
        .collect::<Vec<_>>();
    let oracle = Oracle::new(inputs, outputs).expect("oracle");

    let small = probe_rows_adaptive(&net, &oracle, 2, 0).expect("probe");
    assert_eq!(small.selection, ProbeSelection::Adaptive);
    assert_eq!(small.pilot_rows, 1);
    assert_eq!(small.pilot_kept, 0, "pilot 行消融无效，应被丢掉");
    let coordinates: Vec<(usize, usize)> = small
        .top
        .iter()
        .map(|impact| (impact.layer, impact.row))
        .collect();
    assert!(coordinates.contains(&(1, 0)), "应选到真正承重的行");
    assert!(!coordinates.contains(&(0, 1)), "高活跃但无效的行不应被选");

    let full = probe_rows_adaptive(&net, &oracle, 3, 0).expect("probe");
    assert_eq!(full.pilot_rows, 2, "每个隐层各 pilot 一条");
    assert_eq!(full.pilot_kept, 1);
    let coordinates: Vec<(usize, usize)> = full
        .top
        .iter()
        .map(|impact| (impact.layer, impact.row))
        .collect();
    assert!(coordinates.contains(&(1, 0)));
    assert!(coordinates.contains(&(0, 0)));
    assert!(!coordinates.contains(&(0, 1)));
}
