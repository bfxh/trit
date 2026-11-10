//! 稀疏三态邻接（CSR）：`n + nnz` 存储，适合大图；语义与稠密 [`TritGraph`] 一致。

use trit_core::value::Ternary;

use crate::graph::TritGraph;
use crate::nodes::NodeStates;
use crate::view::TriGraphView;

/// 三态 CSR：三组状态塞在同一条 `targets/states` 边表里，靠状态区分。
#[derive(Clone, Debug)]
pub struct TritCsr {
    nodes: NodeStates,
    offsets: Vec<usize>,
    targets: Vec<u32>,
    states: Vec<Ternary>,
}

impl TritCsr {
    /// 从稠密图转换（保留节点三态与关键位）。
    pub fn from_dense(graph: &TritGraph) -> Self {
        let n = graph.len();
        let mut offsets = vec![0usize; n + 1];
        for u in 0..n {
            offsets[u + 1] = offsets[u] + graph.out_degree(u);
        }
        let mut targets = vec![0u32; offsets[n]];
        let mut states = vec![Ternary::Unknown; offsets[n]];
        let mut nodes = NodeStates::new(n);
        for v in 0..n {
            nodes.set(v, graph.node(v));
            nodes.set_critical(v, graph.is_critical(v));
        }
        for u in 0..n {
            let mut cursor = offsets[u];
            graph.for_each_out(u, |v, state| {
                targets[cursor] = v as u32;
                states[cursor] = state;
                cursor += 1;
            });
            debug_assert_eq!(cursor, offsets[u + 1], "CSR 偏移与出度不一致");
        }
        Self {
            nodes,
            offsets,
            targets,
            states,
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

    /// 边数（非零元）。
    pub fn nnz(&self) -> usize {
        self.targets.len()
    }

    /// 设置节点状态（边表不变）。
    pub fn set_node(&mut self, node: usize, value: Ternary) {
        self.nodes.set(node, value);
    }

    /// 读取节点状态。
    pub fn node(&self, node: usize) -> Ternary {
        self.nodes.get(node)
    }

    /// 标记/取消关键节点。
    pub fn set_critical(&mut self, node: usize, critical: bool) {
        self.nodes.set_critical(node, critical);
    }

    /// 是否关键节点。
    pub fn is_critical(&self, node: usize) -> bool {
        self.nodes.is_critical(node)
    }

    /// `u` 的出边迭代器。
    pub fn neighbors(&self, u: usize) -> impl Iterator<Item = (usize, Ternary)> + '_ {
        assert!(u < self.len(), "节点越界：{u} / {}", self.len());
        let range = self.offsets[u]..self.offsets[u + 1];
        range.map(move |i| (self.targets[i] as usize, self.states[i]))
    }

    /// 查一条边（线性扫该行；测试/小图用）。
    pub fn edge(&self, from: usize, to: usize) -> Option<Ternary> {
        self.neighbors(from)
            .find(|(v, _)| *v == to)
            .map(|(_, state)| state)
    }

    /// 出度（含中立边）。
    pub fn out_degree(&self, node: usize) -> usize {
        TriGraphView::out_degree(self, node)
    }

    /// 出边按状态计数：`(支持, 反对, 中立)`。
    pub fn out_counts(&self, node: usize) -> (usize, usize, usize) {
        TriGraphView::out_counts(self, node)
    }

    /// 拓扑序（Kahn）。有环返回 `None`。
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

impl TriGraphView for TritCsr {
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
        for (v, state) in self.neighbors(u) {
            f(v, state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council;

    fn sample() -> TritGraph {
        let mut g = TritGraph::new(5);
        g.set_node(0, Ternary::True);
        g.set_node(2, Ternary::False);
        g.set_critical(2, true);
        g.set_edge(0, 1, Some(Ternary::True));
        g.set_edge(0, 2, Some(Ternary::True));
        g.set_edge(1, 3, Some(Ternary::False));
        g.set_edge(2, 4, Some(Ternary::Unknown));
        g.set_edge(4, 0, Some(Ternary::True));
        g
    }

    #[test]
    fn conversion_preserves_edges_and_states() {
        let dense = sample();
        let csr = TritCsr::from_dense(&dense);
        assert_eq!(csr.len(), 5);
        assert_eq!(csr.nnz(), dense.edges().len());
        for (u, v, state) in dense.edges() {
            assert_eq!(csr.edge(u, v), Some(state), "边 {u}->{v} 状态不一致");
        }
        for v in 0..5 {
            assert_eq!(csr.node(v), dense.node(v));
            assert_eq!(csr.is_critical(v), dense.is_critical(v));
        }
    }

    #[test]
    fn traversal_and_counts_match_dense() {
        let dense = sample();
        let csr = TritCsr::from_dense(&dense);
        for u in 0..5 {
            let mut a = Vec::new();
            dense.for_each_out(u, |v, s| a.push((v, s)));
            let b: Vec<_> = csr.neighbors(u).collect();
            assert_eq!(a, b, "节点 {u} 的邻域不一致");
            assert_eq!(csr.out_counts(u), dense.out_counts(u));
        }
    }

    #[test]
    fn topo_tree_and_council_match_dense() {
        let dense = sample();
        let csr = TritCsr::from_dense(&dense);
        // 有环：两者都应判 None。
        assert_eq!(csr.topological_order(), dense.topological_order());
        let a = council(&dense, 0);
        let b = council(&csr, 0);
        assert_eq!(a, b, "议会结果应一致");
    }

    #[test]
    fn empty_graph() {
        let dense = TritGraph::new(0);
        let csr = TritCsr::from_dense(&dense);
        assert!(csr.is_empty());
        assert_eq!(csr.nnz(), 0);
        assert_eq!(csr.topological_order(), Some(vec![]));
    }
}
