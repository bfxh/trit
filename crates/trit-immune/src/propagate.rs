//! 三值传播引擎：跨树污染边上的 (min, max) 半环传播（Bellman-Ford 式标签修正）。
//!
//! 与 [bfxh/trit](https://github.com/bfxh/trit) `trit-core::TernaryTopology` 的三值可达性
//! （max∘min：路径值取边值最小、节点对取所有路径最大，`True` 边可通行、`False` 边断开）
//! 互为**极性对偶**：本 crate 的 `-1` 表示"可穿透/传染"（最坏值），故
//! **路径强度 = max(源严重度, 沿途边通过值)**（最薄弱环节决定，已拦截边视为断开），
//! **目标污染等级 = min(所有路径)**（攻击者走最优路径）。
//! 在我们的枚举上 K3 AND/OR 恰为数值 min/max，所以 (min, max) 半环与 K3 完全同构。

use crate::trit::Trit;

/// 三值污染图：节点 `0..n`，带通过强度的有向边。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Contagion {
    nodes: usize,
    edges: Vec<(usize, usize, Trit)>,
}

impl Contagion {
    pub fn new(nodes: usize) -> Self {
        Self {
            nodes,
            edges: Vec::new(),
        }
    }

    /// 加一条有向污染边。`pass`：`-1` 确认可穿透 / `0` 疑似可穿透 / `+1` 已证拦截。
    pub fn link(&mut self, from: usize, to: usize, pass: Trit) {
        self.edges.push((from, to, pass));
    }

    /// 从源集合出发的污染等级（按节点下标对齐；不可达 = `+1`）。
    /// 源以 (节点, 严重度) 给出：严重度封顶它下游能传播到的最坏值。
    pub fn spread(&self, sources: &[(usize, Trit)]) -> Vec<Trit> {
        let mut dist = vec![Trit::Hardened; self.nodes];
        for &(s, sev) in sources {
            if s < self.nodes {
                dist[s] = dist[s].and(sev);
            }
        }
        // (min, max) 是分配格：环不会产生更优路径（max 只增不减），最优路径是简单路径，
        // ≤ n 轮标签修正必收敛，提前退出。
        for _ in 0..=self.nodes {
            let mut changed = false;
            for &(u, v, pass) in &self.edges {
                if u >= self.nodes || v >= self.nodes {
                    continue;
                }
                let cand = dist[u].or(pass); // 数值 max：路径强度由最薄弱环节决定
                if cand.to_i8() < dist[v].to_i8() {
                    dist[v] = cand;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Trit::*;

    #[test]
    fn direct_suspect_and_blocked_edges() {
        let mut g = Contagion::new(4);
        g.link(0, 1, Vulnerable);
        g.link(0, 2, Unknown);
        g.link(0, 3, Hardened);
        let d = g.spread(&[(0, Vulnerable)]);
        assert_eq!(d[0], Vulnerable);
        assert_eq!(d[1], Vulnerable);
        assert_eq!(d[2], Unknown);
        assert_eq!(d[3], Hardened, "已证拦截的边不传播");
    }

    #[test]
    fn source_severity_caps_the_path() {
        // 可疑源（0）即使穿过"确认可穿透"的边，也只能传播 0
        let mut g = Contagion::new(2);
        g.link(0, 1, Vulnerable);
        let d = g.spread(&[(0, Unknown)]);
        assert_eq!(d[1], Unknown);
    }

    #[test]
    fn cycles_converge() {
        let mut g = Contagion::new(3);
        g.link(0, 1, Vulnerable);
        g.link(1, 2, Unknown);
        g.link(2, 1, Vulnerable);
        let d = g.spread(&[(0, Vulnerable)]);
        assert_eq!(d[1], Vulnerable);
        assert_eq!(d[2], Unknown);
    }

    #[test]
    fn brute_force_and_topology_dual_over_all_edge_values() {
        // 菱形图 0→1→3、0→2→3，81 种边值组合：
        // 1) spread 与简单路径穷举一致；
        // 2) 与 TernaryTopology 的 max∘min 极性对偶恒等（逐点取反）。
        let vals = [Vulnerable, Unknown, Hardened];
        for &e01 in &vals {
            for &e02 in &vals {
                for &e13 in &vals {
                    for &e23 in &vals {
                        let mut g = Contagion::new(4);
                        g.link(0, 1, e01);
                        g.link(0, 2, e02);
                        g.link(1, 3, e13);
                        g.link(2, 3, e23);
                        let got = g.spread(&[(0, Vulnerable)]);

                        // 穷举：路径强度 = max(源严重度, 沿途边)，目标 = min(所有路径)
                        let t1 = Vulnerable.or(e01);
                        let t2 = Vulnerable.or(e02);
                        assert_eq!(got[1], t1);
                        assert_eq!(got[2], t2);
                        assert_eq!(got[3], t1.or(e13).and(t2.or(e23)));

                        // 对偶：值取反后按 max∘min（拓扑语义）应得 spread 的逐点取反
                        let r1 = Vulnerable.not().and(e01.not()).and(e13.not());
                        let r2 = Vulnerable.not().and(e02.not()).and(e23.not());
                        let topo3 = r1.or(r2);
                        assert_eq!(got[3], topo3.not());
                    }
                }
            }
        }
    }
}
