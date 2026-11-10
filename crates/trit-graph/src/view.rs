//! 三态图的只读视图：稠密位集图与稀疏 CSR 共用拓扑/树/计数逻辑。

use trit_core::value::Ternary;

use crate::Tree;

/// 三态图的只读视图。
///
/// 只要求实现四件事（节点数、节点状态、关键位、出边遍历）；出度、三态计数、
/// 拓扑序、DFS 树都从这里派生，避免稠密/稀疏两套实现分叉。
pub trait TriGraphView {
    /// 节点数。
    fn len(&self) -> usize;

    /// 是否为空图。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 节点三态。
    fn node(&self, node: usize) -> Ternary;

    /// 是否关键节点（否定票可触发强否决）。
    fn is_critical(&self, node: usize) -> bool;

    /// 遍历 `node` 的存在出边，回调 `(目标, 边状态)`。
    fn for_each_out(&self, node: usize, f: impl FnMut(usize, Ternary));

    /// 出度（含中立边）。
    fn out_degree(&self, node: usize) -> usize {
        let mut count = 0;
        self.for_each_out(node, |_, _| count += 1);
        count
    }

    /// 出边按状态计数：`(支持, 反对, 中立)`。
    fn out_counts(&self, node: usize) -> (usize, usize, usize) {
        let mut plus = 0;
        let mut minus = 0;
        let mut neutral = 0;
        self.for_each_out(node, |_, state| match state {
            Ternary::True => plus += 1,
            Ternary::False => minus += 1,
            Ternary::Unknown => neutral += 1,
        });
        (plus, minus, neutral)
    }

    /// 拓扑序（Kahn；只看存在的边）。有环返回 `None`。
    fn topological_order(&self) -> Option<Vec<usize>> {
        let mut indegree = vec![0usize; self.len()];
        for u in 0..self.len() {
            self.for_each_out(u, |v, _| indegree[v] += 1);
        }
        let mut ready: Vec<usize> = (0..self.len()).filter(|&v| indegree[v] == 0).collect();
        let mut order = Vec::with_capacity(self.len());
        while let Some(u) = ready.pop() {
            order.push(u);
            self.for_each_out(u, |v, _| {
                indegree[v] -= 1;
                if indegree[v] == 0 {
                    ready.push(v);
                }
            });
        }
        (order.len() == self.len()).then_some(order)
    }

    /// 是否 DAG。
    fn is_dag(&self) -> bool {
        self.topological_order().is_some()
    }

    /// 从 `root` 跑 DFS 生成树（只走存在的边；回边/横叉边忽略）。
    fn dfs_tree(&self, root: usize) -> Tree {
        assert!(root < self.len(), "根越界：{root} / {}", self.len());
        let n = self.len();
        let mut parent = vec![None; n];
        let mut visited = vec![false; n];
        let mut preorder = Vec::with_capacity(n);
        let mut stack = vec![root];
        visited[root] = true;
        while let Some(u) = stack.pop() {
            preorder.push(u);
            let mut children = Vec::new();
            self.for_each_out(u, |v, _| {
                if !visited[v] {
                    children.push(v);
                }
            });
            for v in children.into_iter().rev() {
                if !visited[v] {
                    visited[v] = true;
                    parent[v] = Some(u);
                    stack.push(v);
                }
            }
        }
        Tree::new(parent, preorder)
    }
}
