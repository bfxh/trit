//! 三态超图（M0 容器）：节点三态 + 三态超边（成员位集）+ 超边议会。
//!
//! M0 只做容器与判决原语：超边不参与消息传递，也没有超边之间的结构（那是
//! Tri-HyperGNN / 树分解的部分，见 `docs/ROADMAP.md`）。

use trit_core::value::Ternary;

use crate::council::{Council, tally};
use crate::nodes::NodeStates;

/// 一条超边：成员节点位集 + 自己的三态 + 是否关键。
#[derive(Clone, Debug)]
pub struct Hyperedge {
    members: Vec<u64>,
    state: Ternary,
    critical: bool,
    len: usize,
}

impl Hyperedge {
    /// 超边自身的三态（池化/否决时用）。
    pub fn state(&self) -> Ternary {
        self.state
    }

    /// 超边是否关键。
    pub fn is_critical(&self) -> bool {
        self.critical
    }

    /// 成员数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空超边。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 是否包含某个节点。
    pub fn contains(&self, node: usize) -> bool {
        self.members[node / 64] & (1u64 << (node % 64)) != 0
    }

    /// 成员迭代。
    pub fn members(&self) -> impl Iterator<Item = usize> + '_ {
        self.members.iter().enumerate().flat_map(|(w, &word)| {
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
}

/// 三态超图。
#[derive(Clone, Debug)]
pub struct TritHypergraph {
    nodes: NodeStates,
    words: usize,
    edges: Vec<Hyperedge>,
}

impl TritHypergraph {
    /// 建一个 `n` 个节点、没有超图的超图（节点全 `Unknown`）。
    pub fn new(n: usize) -> Self {
        let nodes = NodeStates::new(n);
        let words = nodes.words();
        Self {
            nodes,
            words,
            edges: Vec::new(),
        }
    }

    /// 节点数。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否没有节点。
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

    /// 标记/取消关键节点。
    pub fn set_critical(&mut self, node: usize, critical: bool) {
        self.nodes.set_critical(node, critical);
    }

    /// 是否关键节点。
    pub fn is_critical(&self, node: usize) -> bool {
        self.nodes.is_critical(node)
    }

    /// 加一条超边，返回它的编号。成员重复会被去重。
    pub fn add_hyperedge(&mut self, members: &[usize], state: Ternary, critical: bool) -> usize {
        let mut bitset = vec![0u64; self.words];
        let mut len = 0usize;
        for &node in members {
            assert!(node < self.len(), "超边成员越界：{node} / {}", self.len());
            let (word, bit) = (node / 64, 1u64 << (node % 64));
            if bitset[word] & bit == 0 {
                bitset[word] |= bit;
                len += 1;
            }
        }
        self.edges.push(Hyperedge {
            members: bitset,
            state,
            critical,
            len,
        });
        self.edges.len() - 1
    }

    /// 全部超边。
    pub fn hyperedges(&self) -> &[Hyperedge] {
        &self.edges
    }

    /// 按编号取超边。
    pub fn hyperedge(&self, id: usize) -> Option<&Hyperedge> {
        self.edges.get(id)
    }

    /// 包含 `node` 的超边编号（线性扫；M0 规模）。
    pub fn incident(&self, node: usize) -> Vec<usize> {
        assert!(node < self.len(), "节点越界：{node} / {}", self.len());
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.contains(node))
            .map(|(id, _)| id)
            .collect()
    }

    /// 超边议会：成员节点按三态投票（关键节点的反对票强否决）。
    ///
    /// 超边**自身**的三态不参与这次判决；它用于后续池化/超边间传播。
    pub fn verdict(&self, id: usize) -> Option<Council> {
        let edge = self.edges.get(id)?;
        Some(tally(edge.members().map(|node| {
            (self.nodes.get(node), self.nodes.is_critical(node))
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TritHypergraph {
        // 两条超边：{0,1,2} 支持；{2,3} 反对。
        let mut h = TritHypergraph::new(4);
        h.set_node(0, Ternary::True);
        h.set_node(1, Ternary::True);
        h.set_node(2, Ternary::False);
        h.set_node(3, Ternary::False);
        h.add_hyperedge(&[0, 1, 2], Ternary::True, false);
        h.add_hyperedge(&[2, 3], Ternary::False, false);
        h
    }

    #[test]
    fn construction_and_incidence() {
        let h = sample();
        assert_eq!(h.hyperedges().len(), 2);
        assert_eq!(h.hyperedge(0).expect("有").len(), 3);
        assert!(h.hyperedge(0).expect("有").contains(1));
        assert!(!h.hyperedge(0).expect("有").contains(3));
        assert_eq!(h.incident(2), vec![0, 1]);
        assert!(h.incident(0) == vec![0]);
    }

    #[test]
    fn duplicate_members_are_deduplicated() {
        let mut h = TritHypergraph::new(3);
        h.add_hyperedge(&[0, 0, 1], Ternary::True, false);
        assert_eq!(h.hyperedge(0).expect("有").len(), 2);
    }

    #[test]
    fn verdict_majority_and_veto() {
        let h = sample();
        let verdict = h.verdict(0).expect("有");
        assert_eq!((verdict.plus, verdict.minus), (2, 1));
        assert_eq!(verdict.outcome, Ternary::True);
        // 让节点 2 变成关键：它的 -1 触发否决。
        let mut h = sample();
        h.set_critical(2, true);
        let verdict = h.verdict(0).expect("有");
        assert!(verdict.vetoed);
        assert_eq!(verdict.outcome, Ternary::False);
    }

    #[test]
    fn empty_hyperedge_abstains() {
        let mut h = TritHypergraph::new(2);
        h.add_hyperedge(&[], Ternary::Unknown, false);
        let verdict = h.verdict(0).expect("有");
        assert_eq!(verdict.outcome, Ternary::Unknown);
        assert_eq!(verdict.abstain, 0);
    }

    #[test]
    fn invalid_id_returns_none() {
        let h = sample();
        assert!(h.verdict(99).is_none());
    }
}
