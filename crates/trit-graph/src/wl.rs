//! 三态 1-WL 颜色细化：为图核/同构提供离散颜色原语。
//!
//! 初始颜色由节点三态决定；每轮把 `(当前颜色, 邻居的 (边状态, 颜色) 多重集)`
//! 压成一个新颜色。停止条件是**划分稳定**（不是颜色编号相等）。
//!
//! 这是**有向** 1-WL（只看出边）。和经典 WL 一样，它区分不了正则图
//! （例如 C3 与 C6），这是已知上限，不是实现缺陷。

use std::collections::{HashMap, HashSet};

use trit_core::value::Ternary;

use crate::view::TriGraphView;

/// 节点三态 → 初始颜色编号。
fn state_code(value: Ternary) -> u32 {
    match value {
        Ternary::True => 0,
        Ternary::Unknown => 1,
        Ternary::False => 2,
    }
}

/// 边三态 → 签名里的码。
fn edge_code(value: Ternary) -> u8 {
    match value {
        Ternary::True => 0,
        Ternary::Unknown => 1,
        Ternary::False => 2,
    }
}

/// 跑三态 1-WL，返回每个节点的颜色编号。
///
/// 编号只保证"同一划分内相等"，不保证跨图可比（不同图各自从 0 编号）。
pub fn wl_refine(view: &impl TriGraphView) -> Vec<u32> {
    let n = view.len();
    let mut colors: Vec<u32> = (0..n).map(|v| state_code(view.node(v))).collect();
    // 划分最多 n 轮就稳定；多留一轮给终态检测。
    for _ in 0..=n {
        let mut signatures: Vec<(u32, Vec<(u8, u32)>)> = Vec::with_capacity(n);
        for u in 0..n {
            let mut neighbors = Vec::new();
            view.for_each_out(u, |v, edge| neighbors.push((edge_code(edge), colors[v])));
            neighbors.sort_unstable();
            signatures.push((colors[u], neighbors));
        }
        let mut ids: HashMap<(u32, Vec<(u8, u32)>), u32> = HashMap::new();
        let mut next = Vec::with_capacity(n);
        for signature in signatures {
            let next_id = ids.len() as u32;
            let id = *ids.entry(signature).or_insert(next_id);
            next.push(id);
        }
        // 划分稳定：每个节点的"同色关系"不变。用归一化后的编号序列比较。
        let stable = partition_stable(&colors, &next);
        colors = next;
        if stable {
            break;
        }
    }
    colors
}

/// 两个编号序列是否表示同一划分（忽略编号重命名）。
fn partition_stable(old: &[u32], new: &[u32]) -> bool {
    if old.len() != new.len() {
        return false;
    }
    let old_max = old.iter().copied().max().unwrap_or(0) as usize;
    let new_max = new.iter().copied().max().unwrap_or(0) as usize;
    let mut forward = vec![None; old_max + 1];
    let mut backward = vec![None; new_max + 1];
    for (&a, &b) in old.iter().zip(new) {
        let a = a as usize;
        let b = b as usize;
        match forward[a] {
            Some(mapped) if mapped != b => return false,
            Some(_) => {}
            None => forward[a] = Some(b),
        }
        match backward[b] {
            Some(mapped) if mapped != a => return false,
            Some(_) => {}
            None => backward[b] = Some(a),
        }
    }
    true
}

/// 颜色数（划分大小）。用于快速比较两张图的可区分性。
pub fn color_count(colors: &[u32]) -> usize {
    let seen: HashSet<u32> = colors.iter().copied().collect();
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TritCsr, TritGraph};

    fn path(n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for u in 0..n - 1 {
            g.set_edge(u, u + 1, Some(Ternary::True));
        }
        g
    }

    fn star(n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for v in 1..n {
            g.set_edge(0, v, Some(Ternary::True));
        }
        g
    }

    fn directed_cycle(n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for u in 0..n {
            g.set_edge(u, (u + 1) % n, Some(Ternary::True));
        }
        g
    }

    #[test]
    fn distinguishes_path_from_star() {
        let path_colors = wl_refine(&path(5));
        let star_colors = wl_refine(&star(5));
        // 有向路径每个位置都不同（5 色）；星形只有中心/叶子两类。
        assert_eq!(color_count(&path_colors), 5, "{path_colors:?}");
        assert_eq!(color_count(&star_colors), 2, "{star_colors:?}");
    }

    #[test]
    fn edge_signs_change_colors() {
        // 菱形 0->1, 0->2, 1->3, 2->3：把 2->3 改成反对边，1 与 2 的颜色应分开。
        let diamond = |right_sign: Ternary| {
            let mut g = TritGraph::new(4);
            g.set_edge(0, 1, Some(Ternary::True));
            g.set_edge(0, 2, Some(Ternary::True));
            g.set_edge(1, 3, Some(Ternary::True));
            g.set_edge(2, 3, Some(right_sign));
            g
        };
        let same = wl_refine(&diamond(Ternary::True));
        let mixed = wl_refine(&diamond(Ternary::False));
        assert_eq!(color_count(&same), 3, "{same:?}");
        assert_eq!(color_count(&mixed), 4, "{mixed:?}");
    }

    #[test]
    fn node_states_are_initial_colors() {
        let mut g = path(3);
        g.set_node(1, Ternary::False);
        let colors = wl_refine(&g);
        assert_ne!(colors[1], colors[0]);
        assert_ne!(colors[1], colors[2]);
    }

    #[test]
    fn regular_cycles_are_not_distinguished() {
        // 1-WL 的已知上限：同为 1-正则（有向）的 C3 与 C6 都是单色。
        assert_eq!(color_count(&wl_refine(&directed_cycle(3))), 1);
        assert_eq!(color_count(&wl_refine(&directed_cycle(6))), 1);
    }

    #[test]
    fn csr_and_dense_agree() {
        let dense = path(6);
        let csr = TritCsr::from_dense(&dense);
        assert_eq!(wl_refine(&dense), wl_refine(&csr));
    }
}
