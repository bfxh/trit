//! 提议器不变量：每个预测都能回放，排序确定，不可达目标不给假希望。

use trit_wspace::{Edit, Error, Layer, Net, Oracle, Patch, Trit, analyze};

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
fn top_candidate_is_the_exact_fix() {
    let broken = broken_xor();
    let oracle = xor_oracle();
    let analysis = analyze::analyze(&broken, &oracle, 8).expect("analyze");
    assert!(analysis.before_loss > 0);
    assert_eq!(analysis.evaluated, 18, "9 个三值位置 × 2 个替代值");

    let top = &analysis.candidates[0];
    assert_eq!(top.after_loss, 0, "单点修补应能完全修好");
    assert!(top.gain > 0);
    assert!(top.fixed >= 1);
    assert_eq!(top.broken, 0);
    let mut fixed = broken.clone();
    Patch::new(vec![top.edit.clone()])
        .apply(&mut fixed)
        .expect("apply");
    assert!(oracle.exact(&fixed).expect("exact"));
}

#[test]
fn every_prediction_replays_exactly() {
    let net = Net::random(&[3, 3, 1], 7).expect("net");
    let oracle = Oracle::ternary(3, |input| {
        Trit::sign_of(
            i32::from(input[0].to_i8()) + 2 * i32::from(input[1].to_i8())
                - i32::from(input[2].to_i8()),
        )
    })
    .expect("oracle");
    let analysis = analyze::analyze(&net, &oracle, 0).expect("analyze");
    assert_eq!(analysis.candidates.len(), analysis.evaluated);

    let before = oracle.outputs_of(&net).expect("before");
    let want = oracle.outputs();
    for impact in &analysis.candidates {
        let mut trial = net.clone();
        Patch::new(vec![impact.edit.clone()])
            .apply(&mut trial)
            .expect("apply");
        let after = oracle.outputs_of(&trial).expect("after");
        let fixed = before
            .iter()
            .zip(want)
            .zip(&after)
            .filter(|((left, expected), right)| left != expected && right == expected)
            .count();
        let broken = before
            .iter()
            .zip(want)
            .zip(&after)
            .filter(|((left, expected), right)| left == expected && right != expected)
            .count();
        let changed = before
            .iter()
            .zip(&after)
            .filter(|(left, right)| left != right)
            .count();
        assert_eq!(impact.after_loss, oracle.loss(&trial).expect("loss"));
        assert_eq!(impact.fixed, fixed);
        assert_eq!(impact.broken, broken);
        assert_eq!(impact.changed, changed);
    }
}

#[test]
fn no_single_edit_can_solve_xor_in_a_2_1_net() {
    let net = Net::zeros(&[2, 1]).expect("net");
    let analysis = analyze::analyze(&net, &xor_oracle(), 0).expect("analyze");
    assert!(!analysis.candidates.is_empty());
    assert!(
        analysis
            .candidates
            .iter()
            .all(|impact| impact.after_loss > 0),
        "单层 2→1 不可能靠一条编辑把 XOR 修到 0 失配"
    );
    assert!(
        analysis.candidates.iter().any(|impact| impact.gain > 0),
        "但常量输出仍应把 4 条失配降到 2 条：提议器不能是退化的"
    );
    assert!(analysis.before_loss > 0);
}

#[test]
fn limit_is_respected_and_results_are_deterministic() {
    let net = Net::random(&[2, 3, 1], 9).expect("net");
    let oracle = Oracle::boolean(2, |bits| bits[0] || bits[1]).expect("oracle");
    let first = analyze::analyze(&net, &oracle, 5).expect("first");
    let second = analyze::analyze(&net, &oracle, 5).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.candidates.len(), 5);
    assert_eq!(first.evaluated, 26, "13 个三值位置 × 2");

    let all = analyze::analyze(&net, &oracle, 0).expect("all");
    assert_eq!(all.candidates.len(), all.evaluated);
}

fn two_corruptions() -> Net {
    let mut net = xor_net();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 1,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
    ])
    .apply(&mut net)
    .expect("corrupt");
    net
}

#[test]
fn pair_candidates_find_two_edit_fix() {
    let broken = two_corruptions();
    let oracle = xor_oracle();
    let single = analyze::analyze(&broken, &oracle, 0).expect("singles");
    assert!(
        single.candidates.iter().all(|impact| impact.after_loss > 0),
        "测试前提：没有任何单点编辑能修好这两处破坏"
    );

    let pairs = analyze::analyze_pairs(&broken, &oracle, 0, 20_000).expect("pairs");
    assert_eq!(pairs.before_loss, single.before_loss);
    assert!(pairs.evaluated > 0);
    let top = &pairs.candidates[0];
    assert_eq!(top.after_loss, 0);
    assert!(top.gain > 0);
    assert_eq!(top.broken, 0);
    let mut fixed = broken.clone();
    Patch::new(vec![top.first.clone(), top.second.clone()])
        .apply(&mut fixed)
        .expect("apply pair");
    assert!(oracle.exact(&fixed).expect("exact"));
}

#[test]
fn every_pair_prediction_replays_exactly() {
    let net = Net::random(&[3, 3, 1], 7).expect("net");
    let oracle = Oracle::ternary(3, |input| {
        Trit::sign_of(
            i32::from(input[0].to_i8()) + 2 * i32::from(input[1].to_i8())
                - i32::from(input[2].to_i8()),
        )
    })
    .expect("oracle");
    let analysis = analyze::analyze_pairs(&net, &oracle, 0, 20_000).expect("pairs");
    assert_eq!(analysis.candidates.len(), analysis.evaluated);

    let before = oracle.outputs_of(&net).expect("before");
    let want = oracle.outputs();
    for impact in &analysis.candidates {
        let mut trial = net.clone();
        Patch::new(vec![impact.first.clone(), impact.second.clone()])
            .apply(&mut trial)
            .expect("apply");
        let after = oracle.outputs_of(&trial).expect("after");
        let fixed = before
            .iter()
            .zip(want)
            .zip(&after)
            .filter(|((left, expected), right)| left != expected && right == expected)
            .count();
        let broken = before
            .iter()
            .zip(want)
            .zip(&after)
            .filter(|((left, expected), right)| left == expected && right != expected)
            .count();
        let changed = before
            .iter()
            .zip(&after)
            .filter(|(left, right)| left != right)
            .count();
        assert_eq!(impact.after_loss, oracle.loss(&trial).expect("loss"));
        assert_eq!(impact.fixed, fixed);
        assert_eq!(impact.broken, broken);
        assert_eq!(impact.changed, changed);
    }
}

#[test]
fn pair_limit_and_determinism() {
    let net = Net::random(&[2, 3, 1], 9).expect("net");
    let oracle = Oracle::boolean(2, |bits| bits[0] || bits[1]).expect("oracle");
    let first = analyze::analyze_pairs(&net, &oracle, 5, 20_000).expect("first");
    let second = analyze::analyze_pairs(&net, &oracle, 5, 20_000).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.candidates.len(), 5);
    assert_eq!(first.evaluated, 312, "26 个候选的 325 对减去 13 对同坐标");

    let all = analyze::analyze_pairs(&net, &oracle, 0, 20_000).expect("all");
    assert_eq!(all.candidates.len(), all.evaluated);
}

#[test]
fn pair_budget_is_honest() {
    let net = Net::random(&[3, 3, 1], 7).expect("net");
    let oracle = Oracle::boolean(3, |bits| bits[0] != bits[1]).expect("oracle");
    let error = analyze::analyze_pairs(&net, &oracle, 0, 100).expect_err("超预算应报错");
    assert!(matches!(error, Error::PairBudget { .. }), "{error}");
}

#[test]
fn same_coordinate_pairs_are_excluded() {
    let net = Net::new(vec![Layer::new(1, 1, &[1], &[0]).expect("layer")]).expect("net");
    let oracle = Oracle::boolean_trit(1, |_| Trit::Pos).expect("oracle");
    let analysis = analyze::analyze_pairs(&net, &oracle, 0, 20_000).expect("pairs");
    // 1 个 trit + 1 个偏置 = 4 个候选；6 对里 2 对是同坐标（同一位置的两个替代值）。
    assert_eq!(analysis.evaluated, 4);
    assert_eq!(analysis.candidates.len(), 4);
}
