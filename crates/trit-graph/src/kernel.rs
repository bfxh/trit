//! 三态图核：WL 子树核与路径核。
//!
//! 两者都只依赖 [`TriGraphView`]，可以作用在稠密图或 CSR 上；边状态与节点三态
//! 都进签名，所以"结构一样、符号相反"的图会被区分。
//!
//! 核值本身没有归一化含义，用 [`normalized`] 换成 `[0,1]` 再比较。

use std::collections::HashMap;

use crate::view::TriGraphView;
use crate::wl::{edge_code, state_code};

/// 颜色直方图。
fn counts(colors: &[u32]) -> HashMap<u32, u64> {
    let mut out = HashMap::new();
    for &color in colors {
        *out.entry(color).or_insert(0) += 1;
    }
    out
}

/// 两个直方图的点积。
fn dot<K: std::hash::Hash + Eq + Clone>(a: &HashMap<K, u64>, b: &HashMap<K, u64>) -> u64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    small
        .iter()
        .filter_map(|(key, value)| large.get(key).map(|other| value * other))
        .sum()
}

/// WL 签名：`(当前颜色, 排序后的 (边码, 邻居颜色) 多重集)`。
type Signature = (u32, Vec<(u8, u32)>);

/// 联合 WL 细化一步：两图共用同一张签名→颜色表，颜色编号才可比。
fn refine<G: TriGraphView>(
    graph: &G,
    colors: &[u32],
    table: &mut HashMap<Signature, u32>,
) -> Vec<u32> {
    let mut next = Vec::with_capacity(graph.len());
    for u in 0..graph.len() {
        let mut neighbors = Vec::new();
        graph.for_each_out(u, |v, edge| neighbors.push((edge_code(edge), colors[v])));
        neighbors.sort_unstable();
        let next_id = table.len() as u32;
        let id = *table.entry((colors[u], neighbors)).or_insert(next_id);
        next.push(id);
    }
    next
}

/// WL 子树核：`sum_{h=0..rounds} <hist(color_h(a)), hist(color_h(b))>`。
///
/// 两图的颜色联合编号；边状态参与签名。`rounds = 0` 时只看节点三态。
pub fn wl_subtree_kernel<G: TriGraphView>(a: &G, b: &G, rounds: usize) -> u64 {
    let mut colors_a: Vec<u32> = (0..a.len()).map(|v| state_code(a.node(v))).collect();
    let mut colors_b: Vec<u32> = (0..b.len()).map(|v| state_code(b.node(v))).collect();
    let mut total = dot(&counts(&colors_a), &counts(&colors_b));
    for _ in 0..rounds {
        let mut table: HashMap<Signature, u32> = HashMap::new();
        let next_a = refine(a, &colors_a, &mut table);
        let next_b = refine(b, &colors_b, &mut table);
        total += dot(&counts(&next_a), &counts(&next_b));
        let stable = next_a == colors_a && next_b == colors_b;
        colors_a = next_a;
        colors_b = next_b;
        if stable {
            break;
        }
    }
    total
}

/// 路径计数：长度 `len` 的有向路径签名 → 条数。
///
/// 签名 = 起点节点码 + 每一步 `(边码, 终点节点码)`。`len = 0` 只看节点。
fn path_counts<G: TriGraphView>(graph: &G, len: usize) -> HashMap<Vec<u8>, u64> {
    let mut out = HashMap::new();
    for start in 0..graph.len() {
        let mut signature = vec![state_code(graph.node(start)) as u8];
        walk(graph, start, len, &mut signature, &mut out);
    }
    out
}

fn walk<G: TriGraphView>(
    graph: &G,
    node: usize,
    remaining: usize,
    signature: &mut Vec<u8>,
    out: &mut HashMap<Vec<u8>, u64>,
) {
    if remaining == 0 {
        *out.entry(signature.clone()).or_insert(0) += 1;
        return;
    }
    graph.for_each_out(node, |next, edge| {
        signature.push(edge_code(edge));
        signature.push(state_code(graph.node(next)) as u8);
        walk(graph, next, remaining - 1, signature, out);
        signature.truncate(signature.len() - 2);
    });
}

/// 路径核：长度 `0..=max_len` 的带标签路径计数点积。
///
/// M0 限制 `max_len ≤ 4`（路径条数随度数指数增长）。
pub fn path_kernel<G: TriGraphView>(a: &G, b: &G, max_len: usize) -> u64 {
    assert!(max_len <= 4, "M0 路径核只支持 max_len ≤ 4：{max_len}");
    let mut total = 0;
    for len in 0..=max_len {
        total += dot(&path_counts(a, len), &path_counts(b, len));
    }
    total
}

/// 归一化核：`k / sqrt(k(a,a) · k(b,b))`，落在 `[0,1]`。
pub fn normalized(k: u64, self_a: u64, self_b: u64) -> f64 {
    if self_a == 0 || self_b == 0 {
        return 0.0;
    }
    k as f64 / ((self_a as f64).sqrt() * (self_b as f64).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TritGraph;
    use trit_core::value::Ternary;

    fn chain(n: usize, sign: Ternary) -> TritGraph {
        let mut g = TritGraph::new(n);
        for u in 0..n.saturating_sub(1) {
            g.set_edge(u, u + 1, Some(sign));
        }
        g
    }

    fn cycle(n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for u in 0..n {
            g.set_edge(u, (u + 1) % n, Some(Ternary::True));
        }
        g
    }

    /// 同构重命名：路径 `n-1 -> ... -> 0`。
    fn reversed_chain(n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for u in 0..n.saturating_sub(1) {
            g.set_edge(u + 1, u, Some(Ternary::True));
        }
        g
    }

    fn wl_norm(a: &TritGraph, b: &TritGraph, rounds: usize) -> f64 {
        let k = wl_subtree_kernel(a, b, rounds);
        normalized(
            k,
            wl_subtree_kernel(a, a, rounds),
            wl_subtree_kernel(b, b, rounds),
        )
    }

    fn path_norm(a: &TritGraph, b: &TritGraph, len: usize) -> f64 {
        normalized(
            path_kernel(a, b, len),
            path_kernel(a, a, len),
            path_kernel(b, b, len),
        )
    }

    #[test]
    fn isomorphic_graphs_have_kernel_one() {
        let a = chain(5, Ternary::True);
        let b = reversed_chain(5);
        assert!((wl_norm(&a, &b, 3) - 1.0).abs() < 1e-9);
        assert!((path_norm(&a, &b, 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn edge_signs_matter() {
        let a = chain(4, Ternary::True);
        let mut b = chain(4, Ternary::True);
        b.set_edge(1, 2, Some(Ternary::False));
        assert!(wl_norm(&a, &b, 3) < 0.99);
        assert!(path_norm(&a, &b, 3) < 0.99);
    }

    #[test]
    fn node_states_matter() {
        let a = chain(3, Ternary::True);
        let mut b = chain(3, Ternary::True);
        b.set_node(1, Ternary::False);
        assert!(wl_norm(&a, &b, 2) < 0.99);
        assert!(path_norm(&a, &b, 2) < 0.99);
    }

    #[test]
    fn structure_matters() {
        let cycle3 = cycle(3);
        let path3 = chain(3, Ternary::True);
        assert!(wl_norm(&cycle3, &path3, 3) < 0.99);
        assert!(path_norm(&cycle3, &path3, 2) < 0.99);
    }

    #[test]
    fn regular_cycles_are_proportional() {
        // 两个正则环的计数成比例：归一化核都为 1，两种核都区分不了（已知上限）。
        let c3 = cycle(3);
        let c6 = cycle(6);
        assert!((wl_norm(&c3, &c6, 3) - 1.0).abs() < 1e-9);
        assert!((path_norm(&c3, &c6, 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn empty_graphs_are_zero() {
        let empty = TritGraph::new(0);
        assert_eq!(wl_subtree_kernel(&empty, &empty, 3), 0);
        assert_eq!(path_kernel(&empty, &empty, 3), 0);
        assert_eq!(normalized(0, 0, 0), 0.0);
    }
}
