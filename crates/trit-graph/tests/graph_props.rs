//! 三态图结构的性质测试：稠密/稀疏两条存储必须给同一份拓扑结论，
//! 派生逻辑（拓扑序、DFS 树、议会、随机游走）必须对边界给出确定答案。
//!
//! 这些断言盯的是"两种存储分叉"和"静默给 0/悬置"这两类失效。

use trit_core::value::Ternary;
use trit_graph::classifier::synthetic_samples;
use trit_graph::{
    TernaryClassifier, TransitionTable, TreeDecomposition, TriMpnn, TritCsr, TritGraph,
    TritHypergraph, color_count, council, normalized, path_kernel, tally, tri_pagerank, wl_refine,
    wl_subtree_kernel,
};

fn graph(n: usize, edges: &[(usize, usize, Ternary)]) -> TritGraph {
    let mut g = TritGraph::new(n);
    for &(from, to, state) in edges {
        g.set_edge(from, to, Some(state));
    }
    g
}

// ---------- 稠密存储 ----------

#[test]
fn nodes_default_to_unknown_and_edges_round_trip() {
    let mut g = TritGraph::new(3);
    for i in 0..3 {
        assert_eq!(
            g.node(i),
            Ternary::Unknown,
            "未设置的节点是 Unknown，不是 False"
        );
    }
    g.set_node(0, Ternary::True);
    g.set_critical(0, true);
    assert_eq!(g.node(0), Ternary::True);
    assert!(g.is_critical(0), "ORCH_GRAPH_CRITICAL");

    for state in [Ternary::True, Ternary::False, Ternary::Unknown] {
        g.set_edge(0, 1, Some(state));
        assert_eq!(
            g.edge(0, 1),
            Some(state),
            "ORCH_GRAPH_EDGE_ROUNDTRIP {state:?}"
        );
        // 有向：反向没有边。
        assert_eq!(g.edge(1, 0), None);
    }
}

#[test]
fn deleting_an_edge_clears_its_state_too() {
    // 只清 valid 位而留下 pos 位，下次重设 Unknown 会读成 True——这是经典的复活 bug。
    let mut g = TritGraph::new(2);
    g.set_edge(0, 1, Some(Ternary::True));
    g.set_edge(0, 1, None);
    assert_eq!(g.edge(0, 1), None, "ORCH_GRAPH_EDGE_DELETE");
    g.set_edge(0, 1, Some(Ternary::Unknown));
    assert_eq!(
        g.edge(0, 1),
        Some(Ternary::Unknown),
        "删后重设必须真的变回中立"
    );
    g.set_edge(0, 1, Some(Ternary::True));
    g.set_edge(0, 1, Some(Ternary::False));
    assert_eq!(g.edge(0, 1), Some(Ternary::False), "支持位必须被清掉");
}

#[test]
fn degrees_and_three_state_counts_agree_with_the_edges() {
    let g = graph(
        4,
        &[
            (0, 1, Ternary::True),
            (0, 2, Ternary::False),
            (0, 3, Ternary::Unknown),
            (1, 2, Ternary::True),
        ],
    );
    assert_eq!(g.out_degree(0), 3, "ORCH_GRAPH_DEGREE");
    assert_eq!(g.out_counts(0), (1, 1, 1), "ORCH_GRAPH_OUT_COUNTS");
    assert_eq!(g.out_degree(3), 0);
    assert_eq!(g.out_counts(1), (1, 0, 0));
    let edges = g.edges();
    assert_eq!(edges.len(), 4);
}

#[test]
fn topological_order_is_a_real_order_or_none() {
    let dag = graph(
        4,
        &[
            (0, 1, Ternary::True),
            (1, 2, Ternary::True),
            (0, 2, Ternary::False),
        ],
    );
    let order = dag.topological_order().expect("DAG");
    assert_eq!(order.len(), 4);
    let position = |node: usize| order.iter().position(|&n| n == node).unwrap();
    for &(from, to, _) in &dag.edges() {
        assert!(
            position(from) < position(to),
            "ORCH_GRAPH_TOPO {from} → {to}"
        );
    }
    assert!(dag.is_dag());

    let cyclic = graph(
        3,
        &[
            (0, 1, Ternary::True),
            (1, 2, Ternary::True),
            (2, 0, Ternary::True),
        ],
    );
    assert!(cyclic.topological_order().is_none(), "有环必须返回 None");
    assert!(!cyclic.is_dag(), "ORCH_GRAPH_CYCLE");
}

#[test]
fn dfs_tree_visits_every_reachable_node_exactly_once() {
    let g = graph(
        5,
        &[
            (0, 1, Ternary::True),
            (0, 2, Ternary::True),
            (1, 3, Ternary::True),
            (2, 3, Ternary::False),
        ],
    );
    let tree = g.dfs_tree(0);
    assert_eq!(tree.len(), 5);
    let preorder = tree.preorder().to_vec();
    assert_eq!(preorder[0], 0, "先序第一项是根");
    assert_eq!(preorder.len(), 4, "节点 4 不可达，不该进先序：{preorder:?}");
    let mut seen = std::collections::BTreeSet::new();
    for &node in &preorder {
        assert!(seen.insert(node), "重复访问 {node}");
    }
    // roots 是"所有没有父指针的节点"：不可达的 4 号也在里面。
    // 所以 roots() 不能当"可达集合"用，可达节点只能读 preorder。
    let roots: Vec<usize> = tree.roots().collect();
    assert!(roots.contains(&0), "ORCH_GRAPH_TREE_ROOTS {roots:?}");
    assert!(
        roots.contains(&4),
        "不可达节点也是 root（forest 语义）：{roots:?}"
    );
    // 逆先序当后序：孩子的下标在父之后。
    for node in 0..5 {
        if let Some(parent) = tree.parent(node) {
            let node_at = preorder.iter().position(|&n| n == node).unwrap();
            let parent_at = preorder.iter().position(|&n| n == parent).unwrap();
            assert!(parent_at < node_at, "父必须在孩子之前");
        }
    }
}

// ---------- 稀疏 CSR 与稠密必须同结论 ----------

#[test]
fn csr_and_dense_agree_on_everything_observable() {
    let mut dense = graph(
        6,
        &[
            (0, 1, Ternary::True),
            (0, 2, Ternary::Unknown),
            (1, 3, Ternary::False),
            (2, 3, Ternary::True),
            (3, 4, Ternary::True),
        ],
    );
    dense.set_node(2, Ternary::True);
    dense.set_critical(2, true);
    let csr = TritCsr::from_dense(&dense);

    assert_eq!(csr.len(), dense.len(), "ORCH_GRAPH_CSR_LEN");
    assert_eq!(csr.nnz(), 5, "nnz 是存在的边数：{}", csr.nnz());
    for node in 0..6 {
        assert_eq!(csr.node(node), dense.node(node), "node {node}");
        assert_eq!(
            csr.is_critical(node),
            dense.is_critical(node),
            "critical {node}"
        );
        assert_eq!(
            csr.out_degree(node),
            dense.out_degree(node),
            "degree {node}"
        );
        assert_eq!(
            csr.out_counts(node),
            dense.out_counts(node),
            "counts {node}"
        );
    }
    for from in 0..6 {
        for to in 0..6 {
            assert_eq!(
                csr.edge(from, to),
                dense.edge(from, to),
                "ORCH_GRAPH_CSR_EDGE {from}→{to}"
            );
        }
    }
    assert_eq!(
        csr.topological_order(),
        dense.topological_order(),
        "ORCH_GRAPH_CSR_TOPO"
    );
}

#[test]
fn csr_neighbors_are_exactly_the_existing_out_edges() {
    let dense = graph(4, &[(0, 1, Ternary::True), (0, 3, Ternary::False)]);
    let csr = TritCsr::from_dense(&dense);
    let list: Vec<(usize, Ternary)> = csr.neighbors(0).collect();
    assert_eq!(
        list,
        vec![(1, Ternary::True), (3, Ternary::False)],
        "ORCH_GRAPH_CSR_NEIGHBORS"
    );
    assert!(csr.neighbors(2).collect::<Vec<_>>().is_empty());
}

#[test]
fn an_empty_graph_is_valid_not_an_error() {
    let dense = TritGraph::new(0);
    let csr = TritCsr::from_dense(&dense);
    assert!(dense.is_empty() && csr.is_empty());
    assert_eq!(dense.topological_order(), Some(Vec::new()));
    assert!(csr.topological_order() == Some(Vec::new()));
}

// ---------- 议会 ----------

#[test]
fn council_counts_votes_and_only_critical_negatives_veto() {
    let c = tally([
        (Ternary::True, false),
        (Ternary::True, false),
        (Ternary::False, true),
        (Ternary::Unknown, false),
    ]);
    assert_eq!(c.plus, 2, "ORCH_GRAPH_TALLY_PLUS");
    assert_eq!(c.minus, 1);
    assert_eq!(c.abstain, 1);
    assert!(c.vetoed, "关键节点的反对票是强否决");
    assert_eq!(c.outcome, Ternary::False, "两票支持也压不过强否决");

    let soft = tally([
        (Ternary::True, false),
        (Ternary::True, false),
        (Ternary::False, false),
    ]);
    assert!(!soft.vetoed);
    assert_eq!(soft.outcome, Ternary::True, "多数票胜出");
}

#[test]
fn a_tie_or_all_abstain_suspends_to_unknown() {
    assert_eq!(
        tally([(Ternary::True, false), (Ternary::False, false)]).outcome,
        Ternary::Unknown,
        "ORCH_GRAPH_TALLY_TIE"
    );
    assert_eq!(
        tally([]).outcome,
        Ternary::Unknown,
        "空议会是悬置，不是通过"
    );
    assert_eq!(
        tally([(Ternary::Unknown, false), (Ternary::Unknown, true)]).outcome,
        Ternary::Unknown,
        "全弃权（即使有关键节点）仍是悬置"
    );
}

#[test]
fn council_flips_neighbour_state_on_a_negative_edge() {
    // 反对边投的是邻居状态的「反」：邻居 True ⇒ 投反对；邻居 False ⇒ 投支持。
    let mut g = TritGraph::new(3);
    g.set_node(1, Ternary::True);
    g.set_node(2, Ternary::False);
    g.set_edge(0, 1, Some(Ternary::False));
    g.set_edge(0, 2, Some(Ternary::False));
    let c = council(&g, 0);
    assert_eq!(
        (c.plus, c.minus),
        (1, 1),
        "ORCH_GRAPH_COUNCIL_NEGATE: {c:?}"
    );
    assert_eq!(c.outcome, Ternary::Unknown, "一正一反 ⇒ 悬置");

    // 中立边只弃权，不投票：把那条正投票也变成中立，只剩邻居 1 的反投票。
    g.set_edge(0, 2, Some(Ternary::Unknown));
    let c = council(&g, 0);
    assert_eq!(c.abstain, 1, "{c:?}");
    assert_eq!((c.plus, c.minus), (0, 1), "{c:?}");
    assert_eq!(c.outcome, Ternary::False, "只剩那一张反对票");
}

// ---------- WL 与图核 ----------

#[test]
fn wl_colors_are_deterministic_and_never_exceed_node_count() {
    let g = graph(
        5,
        &[
            (0, 1, Ternary::True),
            (1, 2, Ternary::False),
            (2, 3, Ternary::Unknown),
            (3, 4, Ternary::True),
        ],
    );
    let first = wl_refine(&g);
    let second = wl_refine(&g);
    assert_eq!(first, second, "ORCH_GRAPH_WL_DETERMINISTIC");
    assert_eq!(first.len(), 5);
    assert!(color_count(&first) <= 5, "颜色数不能超过节点数");
}

#[test]
fn isomorphic_graphs_get_the_same_wl_color_counts() {
    // 两个同构的 4-环（一个稠密一个 CSR）必须细化到同样的颜色数。
    let edges = [
        (0, 1, Ternary::True),
        (1, 2, Ternary::True),
        (2, 3, Ternary::True),
        (3, 0, Ternary::True),
    ];
    let dense = graph(4, &edges);
    let csr = TritCsr::from_dense(&dense);
    let a = wl_refine(&dense);
    let b = wl_refine(&csr);
    assert_eq!(color_count(&a), color_count(&b), "ORCH_GRAPH_WL_STORAGE");
}

#[test]
fn a_kernel_never_scores_a_stranger_higher_than_itself() {
    let a = graph(3, &[(0, 1, Ternary::True), (1, 2, Ternary::True)]);
    let other = graph(3, &[(0, 2, Ternary::False)]);
    let self_score = wl_subtree_kernel(&a, &a, 2);
    assert!(self_score > 0);
    assert!(
        self_score >= wl_subtree_kernel(&a, &other, 2),
        "ORCH_GRAPH_KERNEL_SELF"
    );
    assert!(
        path_kernel(&a, &a, 3) >= path_kernel(&a, &other, 3),
        "ORCH_GRAPH_PATH_KERNEL"
    );
}

#[test]
fn normalization_is_one_for_self_and_bounded_by_zero_one() {
    assert_eq!(normalized(9, 9, 9), 1.0);
    assert_eq!(normalized(0, 9, 9), 0.0);
    let value = normalized(3, 9, 16);
    assert!(
        (0.0..=1.0).contains(&value),
        "ORCH_GRAPH_NORMALIZED {value}"
    );
    assert_eq!(normalized(5, 0, 0), 0.0, "分母为 0 时必须给 0，不能 NaN");
}

// ---------- PageRank ----------

#[test]
fn pagerank_conserves_mass_and_returns_three_channels() {
    let g = graph(
        4,
        &[
            (0, 1, Ternary::True),
            (1, 2, Ternary::True),
            (2, 3, Ternary::True),
            (3, 0, Ternary::True),
        ],
    );
    let scores = tri_pagerank(&g, &[(0, Ternary::True)], 0.85, 30, 1e-12);
    assert_eq!(scores.len(), 4);
    let total: f64 = scores.iter().map(|s| s.total()).sum();
    assert!(
        (total - 1.0).abs() < 1e-6,
        "ORCH_GRAPH_PAGERANK_MASS total={total}"
    );
    for score in &scores {
        assert_eq!(
            score.state(),
            Ternary::True,
            "环上全是支持边，各点都该是支持"
        );
    }
}

#[test]
fn a_negative_edge_flips_the_channel_and_unknown_edge_freezes_it() {
    let neg = graph(2, &[(0, 1, Ternary::False)]);
    let scores = tri_pagerank(&neg, &[(0, Ternary::True)], 0.85, 20, 1e-12);
    assert!(
        scores[1].minus > scores[1].plus,
        "ORCH_GRAPH_PAGERANK_FLIP {scores:?}"
    );
    assert_eq!(scores[1].state(), Ternary::False);

    let frozen = graph(2, &[(0, 1, Ternary::Unknown)]);
    let scores = tri_pagerank(&frozen, &[(0, Ternary::True)], 0.85, 20, 1e-12);
    assert!(scores[1].unknown > 0.0, "中立边到目标冻结成未知");
    assert_eq!(
        scores[1].state(),
        Ternary::Unknown,
        "ORCH_GRAPH_PAGERANK_FREEZE"
    );
}

#[test]
fn a_sink_keeps_its_mass_instead_of_dropping_it() {
    // 没有出边时质量留在原地：否则每一步都漏掉一部分，总和会悄悄掉到 0。
    let sink = graph(1, &[]);
    let scores = tri_pagerank(&sink, &[(0, Ternary::True)], 0.85, 10, 0.0);
    assert!(
        (scores[0].total() - 1.0).abs() < 1e-9,
        "ORCH_GRAPH_PAGERANK_SINK"
    );
}

// ---------- 超图 ----------

#[test]
fn hyperedges_track_members_and_incidence() {
    let mut h = TritHypergraph::new(4);
    h.set_node(1, Ternary::True);
    h.set_node(2, Ternary::False);
    h.set_critical(2, true);
    let id = h.add_hyperedge(&[1, 2], Ternary::True, false);
    assert_eq!(id, 0);
    let edge = h.hyperedge(id).expect("just added");
    assert_eq!(edge.len(), 2);
    assert!(edge.contains(1) && edge.contains(2));
    assert_eq!(edge.state(), Ternary::True);
    assert_eq!(h.incident(1), vec![id], "ORCH_GRAPH_HYPER_INCIDENT");
    assert!(h.incident(3).is_empty());

    let verdict = h.verdict(id).expect("verdict");
    assert_eq!(
        verdict.outcome,
        Ternary::False,
        "关键节点的 False 触发强否决"
    );
    assert!(verdict.vetoed);
}

#[test]
fn an_empty_hyperedge_is_reportable_not_a_panic() {
    let mut h = TritHypergraph::new(2);
    let id = h.add_hyperedge(&[], Ternary::Unknown, false);
    let edge = h.hyperedge(id).expect("empty edge still exists");
    assert!(edge.is_empty());
    assert_eq!(edge.len(), 0);
    assert_eq!(
        h.verdict(id).expect("council").outcome,
        Ternary::Unknown,
        "空超边悬置"
    );
}

// ---------- 树分解 ----------

#[test]
fn decomposition_catches_a_missing_bag_cover() {
    // 节点 2 不在任何袋里：validate 必须报出来，而不是给一个空列表。
    let graph = TritGraph::new(3);
    let bags = vec![vec![0, 1], vec![0]];
    let parent = vec![None, Some(0)];
    let decomp = TreeDecomposition::new(3, &bags, &parent);
    let problems = decomp.validate(&graph);
    assert!(!problems.is_empty(), "ORCH_GRAPH_DECOMP_COVER");
    assert!(
        problems.iter().any(|p| p.contains('2')),
        "缺的节点要指名：{problems:?}"
    );
    assert!(decomp.bag_contains(0, 0));
    assert!(!decomp.bag_contains(1, 1));
    assert_eq!(decomp.children(0), vec![1]);
    assert_eq!(decomp.parent(1), Some(0));
}

#[test]
fn a_covering_decomposition_validates_clean() {
    let graph = TritGraph::new(3);
    let bags = vec![vec![0, 1, 2]];
    let parent = vec![None];
    let decomp = TreeDecomposition::new(3, &bags, &parent);
    assert!(decomp.validate(&graph).is_empty(), "ORCH_GRAPH_DECOMP_OK");
    assert_eq!(decomp.bag_count(), 1);
    assert_eq!(decomp.bag_len(0), 3);
}

// ---------- 消息传递与分类器 ----------

#[test]
fn the_identity_transition_table_leaves_states_alone() {
    let table = TransitionTable::identity();
    let g = graph(3, &[(0, 1, Ternary::True), (1, 2, Ternary::True)]);
    let states = vec![Ternary::True, Ternary::False, Ternary::Unknown];
    let mpnn = TriMpnn::new(table, 1);
    let out = mpnn.run(&g, &states);
    assert_eq!(out.len(), 3);
    // 恒等表不是"原样复制"：它表示对边状态的中立处理；这里只断言输出合法。
    for state in out {
        assert!(
            matches!(state, Ternary::True | Ternary::False | Ternary::Unknown),
            "ORCH_GRAPH_MPNN_RANGE"
        );
    }
}

#[test]
fn one_round_equals_one_step() {
    let g = graph(2, &[(0, 1, Ternary::True)]);
    let states = vec![Ternary::True, Ternary::Unknown];
    let one = TriMpnn::new(TransitionTable::identity(), 1);
    let two = TriMpnn::new(TransitionTable::identity(), 2);
    assert_eq!(
        two.run(&g, &states),
        one.step(&g, &states),
        "ORCH_GRAPH_MPNN_ROUNDS"
    );
}

#[test]
fn fitting_reduces_the_loss_on_synthetic_samples() {
    let samples = synthetic_samples(16, 7);
    let mut classifier = TernaryClassifier::new();
    let before = samples.iter().map(|s| classifier.loss(s)).sum::<f32>() / samples.len() as f32;
    let after = classifier.fit(&samples, 60, 0.05);
    let recomputed = samples.iter().map(|s| classifier.loss(s)).sum::<f32>() / samples.len() as f32;
    assert!(
        after < before,
        "ORCH_GRAPH_CLASSIFIER_FIT before={before} after={after}"
    );
    assert!(
        (after - recomputed).abs() < 1e-3,
        "fit 返回值必须与逐样本重算一致"
    );
}

#[test]
fn predictions_are_always_ternary() {
    let samples = synthetic_samples(8, 11);
    let classifier = TernaryClassifier::new();
    for sample in &samples {
        let prediction = classifier.predict(&sample.graph, &sample.states);
        assert!(
            matches!(
                prediction,
                Ternary::True | Ternary::False | Ternary::Unknown
            ),
            "ORCH_GRAPH_CLASSIFIER_RANGE"
        );
        let probabilities = classifier.probabilities(&sample.graph, &sample.states);
        let sum: f32 = probabilities.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4, "三通道概率必须归一：{sum}");
    }
}

// ---------- 视图一致性 ----------

#[test]
fn council_on_csr_equals_council_on_dense() {
    let mut dense = graph(
        4,
        &[
            (0, 1, Ternary::True),
            (0, 2, Ternary::False),
            (0, 3, Ternary::Unknown),
        ],
    );
    dense.set_node(1, Ternary::True);
    dense.set_node(2, Ternary::True);
    let csr = TritCsr::from_dense(&dense);
    let a = council(&dense, 0);
    let b = council(&csr, 0);
    assert_eq!(a, b, "ORCH_GRAPH_COUNCIL_STORAGE");
    assert_eq!(a.outcome, Ternary::Unknown, "支持与被翻转的支持各一票");
}
