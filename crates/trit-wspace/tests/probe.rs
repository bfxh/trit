//! 行级探针：消融效果回放、预算抽样、采样验证集。

use std::collections::HashSet;

use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, probe_rows};

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
