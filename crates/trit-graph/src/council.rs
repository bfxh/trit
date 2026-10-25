//! 三态议会：把邻居的三态票合成为一个判决。
//!
//! 规则（M0 固定表，后续会变成可学习的三态转移表）：
//! - 边 `+1` 投邻居节点的状态；边 `-1` 投其**反**；边 `0`（中立）弃权；
//! - **强否决**：关键节点投出 `-1` ⇒ 直接判 `-1`；
//! - 否则多数票：`+` 多判 `+`、`-` 多判 `-`、平票/全弃权判 `0`。

use trit_core::value::Ternary;

use crate::view::TriGraphView;

/// 一次议会的读数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Council {
    /// 三态判决。
    pub outcome: Ternary,
    /// 支持票数。
    pub plus: usize,
    /// 反对票数。
    pub minus: usize,
    /// 弃权（中立边或未知票）。
    pub abstain: usize,
    /// 是否被关键节点否决。
    pub vetoed: bool,
}

/// 对 `node` 的出边开一次三态议会（稠密图与 CSR 共用）。
pub fn council(graph: &impl TriGraphView, node: usize) -> Council {
    let mut plus = 0usize;
    let mut minus = 0usize;
    let mut abstain = 0usize;
    let mut vetoed = false;

    graph.for_each_out(node, |neighbor, edge| {
        let vote = match edge {
            Ternary::True => Some(graph.node(neighbor)),
            Ternary::False => Some(negate(graph.node(neighbor))),
            Ternary::Unknown => None,
        };
        match vote {
            Some(Ternary::True) => plus += 1,
            Some(Ternary::False) => {
                minus += 1;
                if graph.is_critical(neighbor) {
                    vetoed = true;
                }
            }
            Some(Ternary::Unknown) | None => abstain += 1,
        }
    });

    let outcome = if vetoed {
        Ternary::False
    } else if plus > minus {
        Ternary::True
    } else if minus > plus {
        Ternary::False
    } else {
        Ternary::Unknown
    };

    Council {
        outcome,
        plus,
        minus,
        abstain,
        vetoed,
    }
}

fn negate(value: Ternary) -> Ternary {
    match value {
        Ternary::True => Ternary::False,
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TritGraph;

    fn graph(n: usize, edges: &[(usize, usize, Ternary)], nodes: &[(usize, Ternary)]) -> TritGraph {
        let mut g = TritGraph::new(n);
        for &(node, value) in nodes {
            g.set_node(node, value);
        }
        for &(u, v, state) in edges {
            g.set_edge(u, v, Some(state));
        }
        g
    }

    #[test]
    fn majority_wins() {
        // 0 -> 1(+) -> 0，0 -> 2(+) -> 0，0 -> 3(-) -> 0
        let g = graph(
            4,
            &[
                (0, 1, Ternary::True),
                (0, 2, Ternary::True),
                (0, 3, Ternary::False),
            ],
            &[(1, Ternary::True), (2, Ternary::True), (3, Ternary::True)],
        );
        let c = council(&g, 0);
        assert_eq!(c.plus, 2);
        assert_eq!(c.minus, 1);
        assert_eq!(c.outcome, Ternary::True);
    }

    #[test]
    fn minus_edge_inverts_vote() {
        // 反对边指向 -1 的邻居 ⇒ 投 +1。
        let g = graph(2, &[(0, 1, Ternary::False)], &[(1, Ternary::False)]);
        let c = council(&g, 0);
        assert_eq!(c.plus, 1);
        assert_eq!(c.outcome, Ternary::True);
    }

    #[test]
    fn tie_falls_back_to_unknown() {
        // 支持边指向 +1（+1 票）与 -1（-1 票）⇒ 平票。
        let g = graph(
            3,
            &[(0, 1, Ternary::True), (0, 2, Ternary::True)],
            &[(1, Ternary::True), (2, Ternary::False)],
        );
        let c = council(&g, 0);
        assert_eq!((c.plus, c.minus), (1, 1));
        assert_eq!(c.outcome, Ternary::Unknown);
    }

    #[test]
    fn inverted_minus_votes_can_win() {
        // 反对边指向 -1 的邻居 ⇒ 反后为 +1；两条反对边给出两票支持。
        let g = graph(
            3,
            &[(0, 1, Ternary::False), (0, 2, Ternary::False)],
            &[(1, Ternary::False), (2, Ternary::False)],
        );
        let c = council(&g, 0);
        assert_eq!((c.plus, c.minus), (2, 0));
        assert_eq!(c.outcome, Ternary::True);
    }

    #[test]
    fn critical_false_vetoes_even_with_majority() {
        let mut g = graph(
            4,
            &[
                (0, 1, Ternary::True),
                (0, 2, Ternary::True),
                (0, 3, Ternary::True),
            ],
            &[(1, Ternary::True), (2, Ternary::True), (3, Ternary::False)],
        );
        g.set_critical(3, true);
        let c = council(&g, 0);
        assert!(c.vetoed);
        assert_eq!(c.outcome, Ternary::False);
        assert_eq!(c.plus, 2);
        assert_eq!(c.minus, 1);
    }

    #[test]
    fn neutral_edges_abstain() {
        let g = graph(
            3,
            &[(0, 1, Ternary::Unknown), (0, 2, Ternary::Unknown)],
            &[(1, Ternary::True), (2, Ternary::False)],
        );
        let c = council(&g, 0);
        assert_eq!(c.abstain, 2);
        assert_eq!(c.outcome, Ternary::Unknown);
    }

    #[test]
    fn unknown_votes_abstain() {
        let g = graph(2, &[(0, 1, Ternary::True)], &[(1, Ternary::Unknown)]);
        let c = council(&g, 0);
        assert_eq!(c.abstain, 1);
        assert_eq!(c.outcome, Ternary::Unknown);
    }
}
