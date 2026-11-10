//! 三态图：节点与边都带 `{-1, 0, +1}`，邻接用位集（稠密）。

use trit_core::value::Ternary;

use crate::nodes::NodeStates;
use crate::view::TriGraphView;

/// 三态图（稠密位集邻接）。
///
/// 节点状态：`True` / `Unknown` / `False`（默认 `Unknown`）。
/// 边状态：`Some(True)` 支持、`Some(False)` 反对、`Some(Unknown)` 中立、`None` 不存在。
#[derive(Clone, Debug)]
pub struct TritGraph {
    nodes: NodeStates,
    words: usize,
    edge_pos: Vec<u64>,
    edge_neg: Vec<u64>,
    edge_valid: Vec<u64>,
}

impl TritGraph {
    /// 建一个 `n` 个节点、无边、节点全 `Unknown` 的图。
    pub fn new(n: usize) -> Self {
        let nodes = NodeStates::new(n);
        let words = nodes.words();
        Self {
            nodes,
            words,
            edge_pos: vec![0; n * words],
            edge_neg: vec![0; n * words],
            edge_valid: vec![0; n * words],
        }
    }

    /// 节点数。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否为空图。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 设置节点状态。
    pub fn set_node(&mut self, node: usize, value: Ternary) {
        self.nodes.set(node, value);
    }

    /// 读取节点状态。
    pub fn node(&self, node: usize) -> Ternary {
        self.nodes.get(node)
    }

    /// 标记/取消"关键节点"：关键节点的否定票会触发强否决。
    pub fn set_critical(&mut self, node: usize, critical: bool) {
        self.nodes.set_critical(node, critical);
    }

    /// 是否关键节点。
    pub fn is_critical(&self, node: usize) -> bool {
        self.nodes.is_critical(node)
    }

    /// 设置一条边：`None` 删除，`Some(True/False)` 支持/反对，`Some(Unknown)` 中立。
    pub fn set_edge(&mut self, from: usize, to: usize, state: Option<Ternary>) {
        assert!(from < self.len(), "起点越界：{from} / {}", self.len());
        assert!(to < self.len(), "终点越界：{to} / {}", self.len());
        let index = from * self.words + to / 64;
        let bit = 1u64 << (to % 64);
        match state {
            None => {
                self.edge_valid[index] &= !bit;
                self.edge_pos[index] &= !bit;
                self.edge_neg[index] &= !bit;
            }
            Some(value) => {
                self.edge_valid[index] |= bit;
                match value {
                    Ternary::True => {
                        self.edge_pos[index] |= bit;
                        self.edge_neg[index] &= !bit;
                    }
                    Ternary::False => {
                        self.edge_neg[index] |= bit;
                        self.edge_pos[index] &= !bit;
                    }
                    Ternary::Unknown => {
                        self.edge_pos[index] &= !bit;
                        self.edge_neg[index] &= !bit;
                    }
                }
            }
        }
    }

    /// 读取一条边。
    pub fn edge(&self, from: usize, to: usize) -> Option<Ternary> {
        assert!(from < self.len(), "起点越界：{from} / {}", self.len());
        assert!(to < self.len(), "终点越界：{to} / {}", self.len());
        let index = from * self.words + to / 64;
        let bit = 1u64 << (to % 64);
        if self.edge_valid[index] & bit == 0 {
            return None;
        }
        if self.edge_pos[index] & bit != 0 {
            Some(Ternary::True)
        } else if self.edge_neg[index] & bit != 0 {
            Some(Ternary::False)
        } else {
            Some(Ternary::Unknown)
        }
    }

    /// 全部边，调试/测试用。
    pub fn edges(&self) -> Vec<(usize, usize, Ternary)> {
        let mut out = Vec::new();
        for u in 0..self.len() {
            TriGraphView::for_each_out(self, u, |v, state| out.push((u, v, state)));
        }
        out
    }

    /// 出度（含中立边）。
    pub fn out_degree(&self, node: usize) -> usize {
        TriGraphView::out_degree(self, node)
    }

    /// 出边按状态计数：`(支持, 反对, 中立)`。
    pub fn out_counts(&self, node: usize) -> (usize, usize, usize) {
        TriGraphView::out_counts(self, node)
    }

    /// 拓扑序（Kahn；只看存在的边）。有环返回 `None`。
    pub fn topological_order(&self) -> Option<Vec<usize>> {
        TriGraphView::topological_order(self)
    }

    /// 是否 DAG。
    pub fn is_dag(&self) -> bool {
        TriGraphView::is_dag(self)
    }

    /// 从 `root` 跑 DFS 生成树。
    pub fn dfs_tree(&self, root: usize) -> crate::Tree {
        TriGraphView::dfs_tree(self, root)
    }
}

impl TriGraphView for TritGraph {
    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn node(&self, node: usize) -> Ternary {
        self.nodes.get(node)
    }

    fn is_critical(&self, node: usize) -> bool {
        self.nodes.is_critical(node)
    }

    fn for_each_out(&self, u: usize, mut f: impl FnMut(usize, Ternary)) {
        assert!(u < self.len(), "节点越界：{u} / {}", self.len());
        let base = u * self.words;
        for w in 0..self.words {
            let mut bits = self.edge_valid[base + w];
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let v = w * 64 + bit;
                if v >= self.len() {
                    break;
                }
                let state = if self.edge_pos[base + w] >> bit & 1 == 1 {
                    Ternary::True
                } else if self.edge_neg[base + w] >> bit & 1 == 1 {
                    Ternary::False
                } else {
                    Ternary::Unknown
                };
                f(v, state);
                bits &= bits - 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TritGraph {
        // 0 -> 1 (支持)，0 -> 2 (反对)，1 -> 3 (中立)，2 -> 3 (支持)
        let mut g = TritGraph::new(4);
        g.set_node(0, Ternary::True);
        g.set_node(1, Ternary::False);
        g.set_edge(0, 1, Some(Ternary::True));
        g.set_edge(0, 2, Some(Ternary::False));
        g.set_edge(1, 3, Some(Ternary::Unknown));
        g.set_edge(2, 3, Some(Ternary::True));
        g
    }

    #[test]
    fn node_and_edge_round_trip() {
        let mut g = TritGraph::new(3);
        for (i, value) in [Ternary::True, Ternary::Unknown, Ternary::False]
            .into_iter()
            .enumerate()
        {
            g.set_node(i, value);
            assert_eq!(g.node(i), value);
        }
        g.set_edge(0, 1, Some(Ternary::True));
        g.set_edge(0, 2, Some(Ternary::Unknown));
        assert_eq!(g.edge(0, 1), Some(Ternary::True));
        assert_eq!(g.edge(0, 2), Some(Ternary::Unknown));
        assert_eq!(g.edge(1, 0), None);
        g.set_edge(0, 1, None);
        assert_eq!(g.edge(0, 1), None);
        assert_eq!(g.out_degree(0), 1);
        assert_eq!(g.out_counts(0), (0, 0, 1));
    }

    #[test]
    fn neighbor_iteration_crosses_words() {
        let mut g = TritGraph::new(130);
        g.set_edge(0, 1, Some(Ternary::True));
        g.set_edge(0, 2, Some(Ternary::True));
        g.set_edge(0, 64, Some(Ternary::False));
        g.set_edge(0, 129, Some(Ternary::Unknown));
        let mut got = Vec::new();
        TriGraphView::for_each_out(&g, 0, |v, state| got.push((v, state)));
        assert_eq!(
            got,
            vec![
                (1, Ternary::True),
                (2, Ternary::True),
                (64, Ternary::False),
                (129, Ternary::Unknown)
            ]
        );
        assert_eq!(g.out_degree(0), 4);
    }

    #[test]
    fn topological_order_respects_edges() {
        let g = sample();
        let order = g.topological_order().expect("是 DAG");
        let position = |v: usize| order.iter().position(|&x| x == v).expect("包含全部节点");
        for (u, v, _) in g.edges() {
            assert!(position(u) < position(v), "边 {u}->{v} 方向反了");
        }
    }

    #[test]
    fn cycle_is_detected() {
        let mut g = TritGraph::new(3);
        g.set_edge(0, 1, Some(Ternary::True));
        g.set_edge(1, 2, Some(Ternary::True));
        g.set_edge(2, 0, Some(Ternary::True));
        assert!(g.topological_order().is_none());
        assert!(!g.is_dag());
    }

    #[test]
    fn dfs_tree_parents_and_postorder() {
        let g = sample();
        let tree = g.dfs_tree(0);
        assert_eq!(tree.parent(0), None);
        assert_eq!(tree.roots().collect::<Vec<_>>(), vec![0]);
        let post = tree.postorder();
        let pos = |v: usize| post.iter().position(|&x| x == v).expect("包含");
        for v in 1..4 {
            if let Some(p) = tree.parent(v) {
                assert!(pos(v) < pos(p), "子 {v} 应排在父 {p} 前");
            }
        }
    }
}
