//! 树分解容器（M0）：袋（节点位集）+ 袋树 + 覆盖/运行交校验。
//!
//! 只提供容器与校验，**不做树分解算法**（如何构造袋是 Tri-TD-GNN 的输入，
//! 可以由外部工具给出）。校验三条：节点覆盖、边覆盖、运行交（running intersection）。

use crate::council::{Council, tally};
use crate::graph::TritGraph;
use crate::tree::Tree;

/// 树分解：每个袋是一个节点位集，袋之间是一棵树。
#[derive(Clone, Debug)]
pub struct TreeDecomposition {
    n: usize,
    bags: Vec<Vec<u64>>,
    tree: Tree,
}

impl TreeDecomposition {
    /// 构造。`parent[i]` 是袋 `i` 的父袋；要求 parent 合法、恰好一个根、且整棵树连通。
    pub fn new(n: usize, bags: &[Vec<usize>], parent: &[Option<usize>]) -> Self {
        assert_eq!(bags.len(), parent.len(), "袋数与父指针数不一致");
        let words = n.div_ceil(64);
        let mut bitsets = Vec::with_capacity(bags.len());
        for (index, bag) in bags.iter().enumerate() {
            let mut bits = vec![0u64; words];
            for &node in bag {
                assert!(node < n, "袋 {index} 的节点越界：{node} / {n}");
                bits[node / 64] |= 1u64 << (node % 64);
            }
            bitsets.push(bits);
        }

        let mut roots = Vec::new();
        for (index, slot) in parent.iter().enumerate() {
            match slot {
                None => roots.push(index),
                Some(p) => assert!(*p < bags.len(), "父袋越界：{p} / {}", bags.len()),
            }
        }
        assert_eq!(roots.len(), 1, "袋树必须恰好有一个根，实际 {}", roots.len());
        let mut children = vec![Vec::new(); bags.len()];
        for (index, slot) in parent.iter().enumerate() {
            if let Some(p) = *slot {
                assert_ne!(p, index, "袋不能以自己为父");
                children[p].push(index);
            }
        }
        // DFS 先序（树必须连通）。
        let mut preorder = Vec::with_capacity(bags.len());
        let mut stack = vec![roots[0]];
        while let Some(u) = stack.pop() {
            preorder.push(u);
            for &child in children[u].iter().rev() {
                stack.push(child);
            }
        }
        assert_eq!(preorder.len(), bags.len(), "袋树不连通");
        let tree = Tree::new(parent.to_vec(), preorder);
        Self {
            n,
            bags: bitsets,
            tree,
        }
    }

    /// 袋数。
    pub fn bag_count(&self) -> usize {
        self.bags.len()
    }

    /// 第 `i` 个袋的节点迭代器。
    pub fn bag(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        assert!(i < self.bags.len(), "袋编号越界：{i} / {}", self.bags.len());
        self.bags[i].iter().enumerate().flat_map(|(w, &word)| {
            let mut bits = word;
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                Some(w * 64 + bit)
            })
        })
    }

    /// 袋大小。
    pub fn bag_len(&self, i: usize) -> usize {
        self.bag(i).count()
    }

    /// 袋里是否有某个节点。
    pub fn bag_contains(&self, i: usize, node: usize) -> bool {
        assert!(i < self.bags.len(), "袋编号越界：{i} / {}", self.bags.len());
        self.bags[i][node / 64] & (1u64 << (node % 64)) != 0
    }

    /// 父袋。
    pub fn parent(&self, i: usize) -> Option<usize> {
        self.tree.parent(i)
    }

    /// 子袋。
    pub fn children(&self, i: usize) -> Vec<usize> {
        self.tree.children(i)
    }

    /// 袋议会：袋内节点按三态投票（关键节点的反对票强否决）。
    pub fn bag_verdict(&self, i: usize, graph: &TritGraph) -> Option<Council> {
        if i >= self.bags.len() {
            return None;
        }
        Some(tally(
            self.bag(i)
                .map(|node| (graph.node(node), graph.is_critical(node))),
        ))
    }

    /// 校验：节点覆盖、边覆盖、运行交。返回空 vec 表示通过。
    pub fn validate(&self, graph: &TritGraph) -> Vec<String> {
        assert_eq!(graph.len(), self.n, "图与分解的节点数不一致");
        let mut issues = Vec::new();

        for node in 0..self.n {
            if !(0..self.bags.len()).any(|bag| self.bag_contains(bag, node)) {
                issues.push(format!("节点 {node} 不在任何袋里"));
            }
        }
        for (u, v, _) in graph.edges() {
            let covered = (0..self.bags.len())
                .any(|bag| self.bag_contains(bag, u) && self.bag_contains(bag, v));
            if !covered {
                issues.push(format!("边 {u}->{v} 没有被任何袋覆盖"));
            }
        }
        for node in 0..self.n {
            let holders: Vec<usize> = (0..self.bags.len())
                .filter(|&bag| self.bag_contains(bag, node))
                .collect();
            if holders.is_empty() {
                continue;
            }
            // 运行交：包含该节点的袋在袋树上必须连通。
            // holders 诱导的森林边数 = 每个 holder 与父袋同为 holder 的次数。
            let links = holders
                .iter()
                .filter(|&&bag| {
                    self.tree
                        .parent(bag)
                        .is_some_and(|p| self.bag_contains(p, node))
                })
                .count();
            if links + 1 != holders.len() {
                issues.push(format!(
                    "节点 {node} 的袋不连通（{} 个袋，{} 条树边）",
                    holders.len(),
                    links
                ));
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_core::value::Ternary;

    fn path3() -> TritGraph {
        let mut g = TritGraph::new(3);
        g.set_edge(0, 1, Some(Ternary::True));
        g.set_edge(1, 2, Some(Ternary::True));
        g
    }

    #[test]
    fn valid_path_decomposition() {
        let decomp = TreeDecomposition::new(3, &[vec![0, 1], vec![1, 2]], &[None, Some(0)]);
        assert_eq!(decomp.bag_count(), 2);
        assert_eq!(decomp.bag(0).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(decomp.bag_len(1), 2);
        assert_eq!(decomp.parent(1), Some(0));
        assert_eq!(decomp.children(0), vec![1]);
        assert!(decomp.validate(&path3()).is_empty());
    }

    #[test]
    fn missing_edge_coverage_is_reported() {
        let mut g = path3();
        g.set_edge(0, 2, Some(Ternary::True));
        let decomp = TreeDecomposition::new(3, &[vec![0, 1], vec![1, 2]], &[None, Some(0)]);
        let issues = decomp.validate(&g);
        assert!(issues.iter().any(|s| s.contains("边 0->2")), "{issues:?}");
    }

    #[test]
    fn disconnected_running_intersection_is_reported() {
        let g = TritGraph::new(1);
        let decomp =
            TreeDecomposition::new(1, &[vec![0], vec![], vec![0]], &[None, Some(0), Some(1)]);
        let issues = decomp.validate(&g);
        assert!(issues.iter().any(|s| s.contains("不连通")), "{issues:?}");
    }

    #[test]
    fn bag_verdict_uses_member_states_and_veto() {
        let mut g = TritGraph::new(3);
        g.set_node(0, Ternary::True);
        g.set_node(2, Ternary::True);
        g.set_node(1, Ternary::False);
        g.set_critical(1, true);
        let decomp = TreeDecomposition::new(3, &[vec![0, 1], vec![1, 2]], &[None, Some(0)]);
        let verdict = decomp.bag_verdict(0, &g).expect("有");
        assert_eq!((verdict.plus, verdict.minus), (1, 1));
        assert!(verdict.vetoed);
        assert_eq!(verdict.outcome, Ternary::False);
    }

    #[test]
    fn missing_node_coverage_is_reported() {
        let g = TritGraph::new(3);
        let decomp = TreeDecomposition::new(3, &[vec![0]], &[None]);
        let issues = decomp.validate(&g);
        assert!(issues.iter().any(|s| s.contains("节点 1")), "{issues:?}");
        assert!(issues.iter().any(|s| s.contains("节点 2")), "{issues:?}");
    }

    #[test]
    #[should_panic(expected = "恰好有一个根")]
    fn two_roots_panic() {
        let _ = TreeDecomposition::new(2, &[vec![0], vec![1]], &[None, None]);
    }
}
