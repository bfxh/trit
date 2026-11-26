//! F1/F2 回归集成测试：`states` 维度校验 + 非有限 logits/latent 守卫。
//!
//! 覆盖：空 `states`、长度差 1 / 差 n、超长；NaN / +inf / -inf / 全 `-inf`；
//! `fit` 后表项不被污染（前向输出不含 NaN）；非有限学习率不回写毒值。

use trit_core::BusError;
use trit_core::value::Ternary;
use trit_graph::{ClassifierSample, TernaryClassifier, TransitionTable, TritGraph};

fn two_node_graph() -> TritGraph {
    let mut graph = TritGraph::new(2);
    graph.set_edge(1, 0, Some(Ternary::True));
    graph
}

fn make_sample(states: Vec<Ternary>) -> ClassifierSample {
    ClassifierSample {
        graph: two_node_graph(),
        states,
        label: Ternary::True,
    }
}

#[test]
fn empty_states_is_rejected() {
    let graph = two_node_graph();
    let classifier = TernaryClassifier::with_table(TransitionTable::identity());
    let states: Vec<Ternary> = Vec::new();

    assert!(matches!(
        classifier.predict(&graph, &states),
        Err(BusError::WidthMismatch { .. })
    ));
    assert!(matches!(
        classifier.probabilities(&graph, &states),
        Err(BusError::WidthMismatch { .. })
    ));

    let sample = make_sample(states);
    let classifier = TernaryClassifier::with_table(TransitionTable::identity());
    assert!(matches!(
        classifier.loss(&sample),
        Err(BusError::WidthMismatch { .. })
    ));
    assert!(matches!(
        classifier.loss_and_grads(&sample),
        Err(BusError::WidthMismatch { .. })
    ));
}

#[test]
fn states_one_short_is_rejected() {
    let graph = two_node_graph();
    let classifier = TernaryClassifier::with_table(TransitionTable::identity());
    let states = vec![Ternary::True]; // len 1 < 2
    assert!(matches!(
        classifier.predict(&graph, &states),
        Err(BusError::WidthMismatch { .. })
    ));
}

#[test]
fn states_n_short_is_rejected() {
    let mut graph = TritGraph::new(4);
    graph.set_edge(1, 0, Some(Ternary::True));
    let classifier = TernaryClassifier::with_table(TransitionTable::identity());
    let states = vec![Ternary::True]; // len 1 < 4
    assert!(matches!(
        classifier.loss(&make_sample(states)),
        Err(BusError::WidthMismatch { .. })
    ));
}

#[test]
fn states_too_long_is_rejected() {
    let graph = two_node_graph();
    let classifier = TernaryClassifier::with_table(TransitionTable::identity());
    let states = vec![Ternary::True, Ternary::False, Ternary::Unknown]; // len 3 > 2
    assert!(matches!(
        classifier.predict(&graph, &states),
        Err(BusError::WidthMismatch { .. })
    ));
}

#[test]
fn matching_length_is_accepted() {
    let graph = two_node_graph();
    let classifier = TernaryClassifier::with_table(TransitionTable::identity());
    let states = vec![Ternary::Unknown, Ternary::True];
    let probabilities = classifier.probabilities(&graph, &states).unwrap();
    assert!(probabilities.iter().all(|p| p.is_finite()));
    assert_eq!(classifier.predict(&graph, &states).unwrap(), Ternary::True);
}

#[test]
fn nan_latent_table_yields_finite_outputs() {
    let table = TransitionTable::new([[f32::NAN; 3]; 3]);
    let classifier = TernaryClassifier::with_table(table);
    let graph = two_node_graph();
    let states = vec![Ternary::Unknown, Ternary::True];

    let probabilities = classifier.probabilities(&graph, &states).unwrap();
    assert!(
        probabilities.iter().all(|p| p.is_finite()),
        "{probabilities:?}"
    );

    let loss = classifier.loss(&make_sample(states)).unwrap();
    assert!(loss.is_finite(), "loss={loss}");

    let (loss, dtable, dhead) = classifier
        .loss_and_grads(&make_sample(vec![Ternary::Unknown, Ternary::True]))
        .unwrap();
    assert!(loss.is_finite());
    assert!(dtable.iter().flatten().all(|v| v.is_finite()));
    assert!(dhead.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn inf_latent_table_yields_finite_outputs() {
    let mut latent = [[0.0f32; 3]; 3];
    latent[0][0] = f32::INFINITY;
    latent[1][1] = f32::NEG_INFINITY;
    let classifier = TernaryClassifier::with_table(TransitionTable::new(latent));
    let graph = two_node_graph();
    let states = vec![Ternary::Unknown, Ternary::True];
    let probabilities = classifier.probabilities(&graph, &states).unwrap();
    assert!(
        probabilities.iter().all(|p| p.is_finite()),
        "{probabilities:?}"
    );
}

#[test]
fn fit_does_not_poison_table_with_nan() {
    let mut classifier = TernaryClassifier::with_table(TransitionTable::new([[f32::NAN; 3]; 3]));
    let sample = make_sample(vec![Ternary::Unknown, Ternary::True]);

    let loss = classifier.fit(&[sample], 20, 0.5).unwrap();
    assert!(loss.is_finite(), "fit loss={loss}");

    // 再前向一次：概率与损失仍不含 NaN。
    let probabilities = classifier
        .probabilities(&two_node_graph(), &[Ternary::Unknown, Ternary::True])
        .unwrap();
    assert!(
        probabilities.iter().all(|p| p.is_finite()),
        "{probabilities:?}"
    );
    let loss = classifier
        .loss(&make_sample(vec![Ternary::Unknown, Ternary::True]))
        .unwrap();
    assert!(loss.is_finite(), "post-fit loss={loss}");

    // 被触及的表项不得变成 NaN；未触及的预置毒值不在此断言（输入侧责任）。
    for &value in &classifier.table().latent()[0] {
        assert!(value.is_finite(), "touched latent={value}");
    }
}

#[test]
fn fit_with_infinite_lr_does_not_write_poison() {
    let mut classifier = TernaryClassifier::new();
    let sample = make_sample(vec![Ternary::Unknown, Ternary::True]);
    // 非有限学习率按 0 步长处理：表项保持有限。
    let loss = classifier.fit(&[sample], 10, f32::INFINITY).unwrap();
    assert!(loss.is_finite());
    for row in classifier.table().latent() {
        for &value in row {
            assert!(value.is_finite(), "latent={value}");
        }
    }
}
