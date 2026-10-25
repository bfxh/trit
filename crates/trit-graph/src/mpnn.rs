//! TriMPNN M0：可学习的三态转移表 + 一层三态消息传递。
//!
//! 固定规则的三态议会（[`crate::council`]）换成一张 **3×3 转移表**：
//! `(边状态, 邻居节点状态) → 投票`。表项的 latent 是 f32，前向取硬三票
//! （死区 `±0.5` 内为"弃权"），反向用 [`Ste`] 把梯度直通回 latent。
//!
//! 目前是**一层消息传递 + 表级监督拟合**；整图标签的端到端训练在后续里程碑。

use trit_core::value::Ternary;
use trit_learn::Ste;

use crate::council::tally;
use crate::view::TriGraphView;

/// 死区：`|latent| ≤ DEAD_ZONE` 的票视为"弃权"。
const DEAD_ZONE: f32 = 0.5;

/// 三态转移表。
#[derive(Clone, Debug)]
pub struct TransitionTable {
    latent: [[f32; 3]; 3],
}

impl TransitionTable {
    /// 用给定 latent 构造（行 = 边状态 `+ / 0 / -`，列 = 节点状态 `+ / 0 / -`）。
    pub fn new(latent: [[f32; 3]; 3]) -> Self {
        Self { latent }
    }

    /// 恒等规则：支持边复制邻居状态、反对边取反、中立边弃权。
    pub fn identity() -> Self {
        let mut latent = [[0.0f32; 3]; 3];
        for (edge, row) in latent.iter_mut().enumerate() {
            let sign = match edge {
                0 => 1.0,
                1 => 0.0,
                _ => -1.0,
            };
            for (state, slot) in row.iter_mut().enumerate() {
                *slot = sign * value_of(index_to_state(state));
            }
        }
        Self { latent }
    }

    /// latent 矩阵（调试/测试用）。
    pub fn latent(&self) -> &[[f32; 3]; 3] {
        &self.latent
    }

    /// 前向硬票：`latent > 0.5 → +1`、`< -0.5 → -1`、否则 `0`。
    pub fn vote(&self, edge: Ternary, state: Ternary) -> Ternary {
        let latent = self.latent[index_of(edge)][index_of(state)];
        if latent > DEAD_ZONE {
            Ternary::True
        } else if latent < -DEAD_ZONE {
            Ternary::False
        } else {
            Ternary::Unknown
        }
    }

    /// 一步监督拟合：样本 `(边状态, 邻居状态, 目标票)`。
    ///
    /// 前向硬票、反向 STE；返回本轮"硬票 ≠ 目标"的比例。
    pub fn fit_step(&mut self, samples: &[(Ternary, Ternary, Ternary)], lr: f32, ste: Ste) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let mut wrong = 0usize;
        for &(edge, state, target) in samples {
            let e = index_of(edge);
            let s = index_of(state);
            let current = self.vote(edge, state);
            if current != target {
                wrong += 1;
            }
            let grad_q = value_of(target) - value_of(current);
            let grad = ste.backward(self.latent[e][s], 1.0, grad_q);
            self.latent[e][s] += lr * grad;
        }
        wrong as f32 / samples.len() as f32
    }
}

/// 一层三态消息传递。
#[derive(Clone, Debug)]
pub struct TriMpnn {
    table: TransitionTable,
    rounds: usize,
}

impl TriMpnn {
    /// 用给定转移表与传播轮数构造。
    pub fn new(table: TransitionTable, rounds: usize) -> Self {
        Self { table, rounds }
    }

    /// 转移表。
    pub fn table(&self) -> &TransitionTable {
        &self.table
    }

    /// 一轮传播：每个节点收集**入边**消息，按转移表投票，再开一次三态议会。
    ///
    /// 没有入边的节点保持原状态（不把孤立节点冲成 `Unknown`）。
    pub fn step(&self, view: &impl TriGraphView, states: &[Ternary]) -> Vec<Ternary> {
        assert_eq!(states.len(), view.len(), "状态长度必须等于节点数");
        let n = view.len();
        let mut votes: Vec<Vec<Ternary>> = vec![Vec::new(); n];
        for (source, &state) in states.iter().enumerate() {
            view.for_each_out(source, |target, edge| {
                votes[target].push(self.table.vote(edge, state));
            });
        }
        (0..n)
            .map(|node| {
                if votes[node].is_empty() {
                    states[node]
                } else {
                    tally(votes[node].iter().map(|&vote| (vote, false))).outcome
                }
            })
            .collect()
    }

    /// 跑 `rounds` 轮。
    pub fn run(&self, view: &impl TriGraphView, states: &[Ternary]) -> Vec<Ternary> {
        let mut current = states.to_vec();
        for _ in 0..self.rounds {
            current = self.step(view, &current);
        }
        current
    }
}

fn index_of(state: Ternary) -> usize {
    match state {
        Ternary::True => 0,
        Ternary::Unknown => 1,
        Ternary::False => 2,
    }
}

fn index_to_state(index: usize) -> Ternary {
    match index {
        0 => Ternary::True,
        2 => Ternary::False,
        _ => Ternary::Unknown,
    }
}

fn value_of(state: Ternary) -> f32 {
    match state {
        Ternary::True => 1.0,
        Ternary::Unknown => 0.0,
        Ternary::False => -1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TritGraph;

    fn paths(edges: &[(usize, usize, Ternary)], n: usize) -> TritGraph {
        let mut g = TritGraph::new(n);
        for &(u, v, state) in edges {
            g.set_edge(u, v, Some(state));
        }
        g
    }

    #[test]
    fn identity_table_votes_all_nine_pairs() {
        let table = TransitionTable::identity();
        for edge in [Ternary::True, Ternary::Unknown, Ternary::False] {
            for state in [Ternary::True, Ternary::Unknown, Ternary::False] {
                let want = match edge {
                    Ternary::True => state,
                    Ternary::False => match state {
                        Ternary::True => Ternary::False,
                        Ternary::False => Ternary::True,
                        Ternary::Unknown => Ternary::Unknown,
                    },
                    Ternary::Unknown => Ternary::Unknown,
                };
                assert_eq!(
                    table.vote(edge, state),
                    want,
                    "edge={edge:?} state={state:?}"
                );
            }
        }
    }

    #[test]
    fn dead_zone_is_unknown() {
        let mut latent = [[0.0f32; 3]; 3];
        latent[0][0] = 0.5;
        let table = TransitionTable::new(latent);
        assert_eq!(table.vote(Ternary::True, Ternary::True), Ternary::Unknown);
        latent[0][0] = 0.51;
        let table = TransitionTable::new(latent);
        assert_eq!(table.vote(Ternary::True, Ternary::True), Ternary::True);
    }

    #[test]
    fn fit_recovers_identity_table() {
        let samples: Vec<(Ternary, Ternary, Ternary)> =
            [Ternary::True, Ternary::Unknown, Ternary::False]
                .into_iter()
                .flat_map(|edge| {
                    [Ternary::True, Ternary::Unknown, Ternary::False]
                        .into_iter()
                        .map(move |state| {
                            let target = match edge {
                                Ternary::True => state,
                                Ternary::False => match state {
                                    Ternary::True => Ternary::False,
                                    Ternary::False => Ternary::True,
                                    Ternary::Unknown => Ternary::Unknown,
                                },
                                Ternary::Unknown => Ternary::Unknown,
                            };
                            (edge, state, target)
                        })
                })
                .collect();
        let mut table = TransitionTable::new([[0.0; 3]; 3]);
        for _ in 0..50 {
            table.fit_step(&samples, 0.5, Ste::Identity);
        }
        for &(edge, state, target) in &samples {
            assert_eq!(
                table.vote(edge, state),
                target,
                "edge={edge:?} state={state:?}"
            );
        }
    }

    #[test]
    fn propagates_along_support_edges() {
        let g = paths(&[(0, 1, Ternary::True), (1, 2, Ternary::True)], 3);
        let mpnn = TriMpnn::new(TransitionTable::identity(), 2);
        let states = [Ternary::True, Ternary::Unknown, Ternary::Unknown];
        assert_eq!(
            mpnn.run(&g, &states),
            vec![Ternary::True, Ternary::True, Ternary::True]
        );
    }

    #[test]
    fn oppose_edge_flips_message() {
        let g = paths(&[(0, 1, Ternary::False)], 2);
        let mpnn = TriMpnn::new(TransitionTable::identity(), 1);
        let states = [Ternary::True, Ternary::Unknown];
        assert_eq!(mpnn.run(&g, &states), vec![Ternary::True, Ternary::False]);
    }

    #[test]
    fn neutral_edge_abstains() {
        let g = paths(&[(0, 1, Ternary::Unknown)], 2);
        let mpnn = TriMpnn::new(TransitionTable::identity(), 1);
        let states = [Ternary::True, Ternary::False];
        // 中立项投弃权票；目标没有其它入边 → 议会全弃权 → Unknown。
        assert_eq!(mpnn.run(&g, &states), vec![Ternary::True, Ternary::Unknown]);
    }

    #[test]
    fn isolated_nodes_keep_state() {
        let g = TritGraph::new(2);
        let mpnn = TriMpnn::new(TransitionTable::identity(), 3);
        let states = [Ternary::True, Ternary::False];
        assert_eq!(mpnn.run(&g, &states), vec![Ternary::True, Ternary::False]);
    }
}
