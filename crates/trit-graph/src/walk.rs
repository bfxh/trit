//! 三态随机游走 / PageRank：支持、反对、冻结三条通道一起传播。
//!
//! 语义（固定规则，参数可调）：
//! - 每条出边均分源节点的质量；
//! - 支持边：`+` 留 `+`、`-` 留 `-`；
//! - 反对边：`+`/`-` 互换；
//! - 中立边：到目标时**冻结**为"未知"（三通道合并进 `unknown`）；
//! - 阻尼 `d`：每轮向种子先验回流 `(1-d)`，否则质量会沿环无限累积。
//!
//! 输出是三通道质量 [`TriScore`]；`state()` 用"最大值胜出、并列或未知占优则悬置"。

use trit_core::value::Ternary;

use crate::view::TriGraphView;

/// 节点上的三通道质量（不保证归一化，和等于种子总质量）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TriScore {
    /// 支持通道。
    pub plus: f64,
    /// 反对通道。
    pub minus: f64,
    /// 未知/冻结通道。
    pub unknown: f64,
}

impl TriScore {
    /// 三通道总量。
    pub fn total(self) -> f64 {
        self.plus + self.minus + self.unknown
    }

    /// 三态判决：未知占优或并列 → 悬置；否则多数通道。
    pub fn state(self) -> Ternary {
        if self.unknown >= self.plus && self.unknown >= self.minus {
            Ternary::Unknown
        } else if self.plus > self.minus {
            Ternary::True
        } else if self.minus > self.plus {
            Ternary::False
        } else {
            Ternary::Unknown
        }
    }
}

/// 三态 PageRank。
///
/// `seeds` 是带状态的种子（同一节点可出现多次，质量累加）；`damping ∈ [0,1)`；
/// `max_iters` 轮或 L1 变化小于 `tolerance` 即停。
pub fn tri_pagerank(
    view: &impl TriGraphView,
    seeds: &[(usize, Ternary)],
    damping: f64,
    max_iters: usize,
    tolerance: f64,
) -> Vec<TriScore> {
    assert!(
        (0.0..1.0).contains(&damping),
        "damping 必须在 [0, 1) 内：{damping}"
    );
    let n = view.len();
    let mut prior = vec![TriScore::default(); n];
    for &(node, state) in seeds {
        assert!(node < n, "种子越界：{node} / {n}");
        match state {
            Ternary::True => prior[node].plus += 1.0,
            Ternary::False => prior[node].minus += 1.0,
            Ternary::Unknown => prior[node].unknown += 1.0,
        }
    }

    let mut current = prior.clone();
    for _ in 0..max_iters {
        let mut next: Vec<TriScore> = prior
            .iter()
            .map(|p| TriScore {
                plus: (1.0 - damping) * p.plus,
                minus: (1.0 - damping) * p.minus,
                unknown: (1.0 - damping) * p.unknown,
            })
            .collect();

        for u in 0..n {
            let mass = current[u];
            if mass.total() == 0.0 {
                continue;
            }
            let degree = view.out_degree(u);
            if degree == 0 {
                // 没有出边：质量留在原地（自环）。
                next[u].plus += damping * mass.plus;
                next[u].minus += damping * mass.minus;
                next[u].unknown += damping * mass.unknown;
                continue;
            }
            let share = damping / degree as f64;
            view.for_each_out(u, |v, edge| {
                let (plus, minus, unknown) = match edge {
                    Ternary::True => (mass.plus, mass.minus, mass.unknown),
                    Ternary::False => (mass.minus, mass.plus, mass.unknown),
                    // 中立边：冻结成未知。
                    Ternary::Unknown => (0.0, 0.0, mass.total()),
                };
                next[v].plus += share * plus;
                next[v].minus += share * minus;
                next[v].unknown += share * unknown;
            });
        }

        let delta: f64 = current
            .iter()
            .zip(&next)
            .map(|(a, b)| {
                (a.plus - b.plus).abs() + (a.minus - b.minus).abs() + (a.unknown - b.unknown).abs()
            })
            .sum();
        current = next;
        if delta < tolerance {
            break;
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TritGraph;

    fn chain(edges: &[(usize, usize, Ternary)], n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for &(u, v, state) in edges {
            g.set_edge(u, v, Some(state));
        }
        g
    }

    fn states(scores: &[TriScore]) -> Vec<Ternary> {
        scores.iter().map(|s| s.state()).collect()
    }

    #[test]
    fn support_edges_carry_true() {
        let g = chain(
            &[
                (0, 1, Ternary::True),
                (1, 2, Ternary::True),
                (2, 3, Ternary::True),
            ],
            4,
        );
        let scores = tri_pagerank(&g, &[(0, Ternary::True)], 0.85, 80, 1e-12);
        assert_eq!(states(&scores), vec![Ternary::True; 4]);
        // 质量守恒：种子总质量 = 1。
        let total: f64 = scores.iter().map(|s| s.total()).sum();
        assert!((total - 1.0).abs() < 1e-6, "total={total}");
    }

    #[test]
    fn oppose_edge_flips_state() {
        let g = chain(&[(0, 1, Ternary::False)], 2);
        let scores = tri_pagerank(&g, &[(0, Ternary::True)], 0.5, 20, 1e-12);
        assert!(scores[1].minus > 0.0);
        assert_eq!(scores[1].state(), Ternary::False);
    }

    #[test]
    fn neutral_edge_freezes_to_unknown() {
        let g = chain(&[(0, 1, Ternary::Unknown)], 2);
        let scores = tri_pagerank(&g, &[(0, Ternary::True)], 0.5, 20, 1e-12);
        assert!(scores[1].unknown > 0.0);
        assert_eq!(scores[1].state(), Ternary::Unknown);
    }

    #[test]
    fn seed_mass_accumulates() {
        // 0 -> 1 与 2 -> 1：两个 True 种子对一个 False 种子。
        let g = chain(&[(0, 1, Ternary::True), (2, 1, Ternary::True)], 3);
        let scores = tri_pagerank(
            &g,
            &[(0, Ternary::True), (0, Ternary::True), (2, Ternary::False)],
            0.5,
            30,
            1e-12,
        );
        assert!(scores[1].plus > scores[1].minus);
        assert_eq!(scores[1].state(), Ternary::True);
    }

    #[test]
    fn tie_is_unknown() {
        let g = TritGraph::new(1);
        let scores = tri_pagerank(
            &g,
            &[(0, Ternary::True), (0, Ternary::False)],
            0.5,
            10,
            1e-12,
        );
        assert_eq!(scores[0].state(), Ternary::Unknown);
    }

    #[test]
    fn no_seeds_stay_empty() {
        let g = chain(&[(0, 1, Ternary::True)], 2);
        let scores = tri_pagerank(&g, &[], 0.85, 10, 1e-12);
        assert!(scores.iter().all(|s| s.total() == 0.0));
        assert_eq!(states(&scores), vec![Ternary::Unknown; 2]);
    }

    #[test]
    fn dead_end_keeps_mass() {
        let g = chain(&[(0, 1, Ternary::True)], 2);
        let scores = tri_pagerank(&g, &[(1, Ternary::True)], 0.85, 50, 1e-12);
        // 节点 1 没有出边，质量留在原地。
        assert!(scores[1].plus > 0.0);
        assert_eq!(scores[1].state(), Ternary::True);
    }
}
