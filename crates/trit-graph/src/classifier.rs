//! 端到端三态节点分类 M0：一层软松弛训练 + 硬三票推理。
//!
//! 任务形态：给一张有向图和初始节点三态，预测**目标节点**（固定 0 号）的三态。
//! 模型只有一层消息传递：
//! - 软前向（训练）：邻居状态做 one-hot/均匀分布，表项取 `tanh(latent)`，聚合后过三叉头；
//! - 硬前向（推理）：表项取硬三票，按议会合成——与 [`crate::council`] 同一规则。
//!
//! 训练是**解析梯度**（不是有限差分）：`tanh → mean → 线性头 → softmax → CE`。
//! 图级池化/多层/边特征在后续里程碑。

use trit_core::BusError;
use trit_core::value::Ternary;

use crate::council::tally;
use crate::graph::TritGraph;
use crate::guards::{GradHead, GradTable, check_states_len, finite_latent, finite_scalar, softmax};
use crate::mpnn::TransitionTable;
use crate::view::TriGraphView;

/// 目标节点固定为 0 号。
pub const TARGET: usize = 0;

/// 一条训练样本。
#[derive(Clone, Debug)]
pub struct ClassifierSample {
    /// 图（只用到 `TARGET` 的入边）。
    pub graph: TritGraph,
    /// 初始节点三态。
    pub states: Vec<Ternary>,
    /// 目标节点的真实三态。
    pub label: Ternary,
}

/// 三态节点分类器（单层）。
#[derive(Clone, Debug)]
pub struct TernaryClassifier {
    table: TransitionTable,
    /// `head[0][k] * feature + head[1][k]`。
    head: [[f32; 3]; 2],
}

impl Default for TernaryClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl TernaryClassifier {
    /// 零表 + 非零小头的初始化（头为零会让表永远收不到梯度）。
    pub fn new() -> Self {
        Self {
            table: TransitionTable::new([[0.0; 3]; 3]),
            head: [[0.1, 0.0, -0.1], [0.0; 3]],
        }
    }

    /// 用指定转移表构造（推理/对照用）。
    pub fn with_table(table: TransitionTable) -> Self {
        Self {
            table,
            head: [[0.1, 0.0, -0.1], [0.0; 3]],
        }
    }

    /// 转移表。
    pub fn table(&self) -> &TransitionTable {
        &self.table
    }

    /// 软特征：目标节点的入边投票（`tanh(latent)` 加权、按入度平均）。
    pub fn feature(&self, view: &impl TriGraphView, states: &[Ternary]) -> Result<f32, BusError> {
        check_states_len(view, states)?;
        let incoming = incoming_edges(view, TARGET);
        if incoming.is_empty() {
            return Ok(0.0);
        }
        let mut sum = 0.0f32;
        for (source, edge) in &incoming {
            let probabilities = soft_state(states[*source]);
            let row = &self.table.latent()[index_of(*edge)];
            for (slot, &probability) in probabilities.iter().enumerate() {
                sum += probability * finite_latent(row[slot]).tanh();
            }
        }
        Ok(sum / incoming.len() as f32)
    }

    fn logits(&self, feature: f32) -> [f32; 3] {
        [
            self.head[0][0] * feature + self.head[1][0],
            self.head[0][1] * feature + self.head[1][1],
            self.head[0][2] * feature + self.head[1][2],
        ]
    }

    /// 软概率（`[+1, 0, -1]` 三类）。
    pub fn probabilities(
        &self,
        view: &impl TriGraphView,
        states: &[Ternary],
    ) -> Result<[f32; 3], BusError> {
        let feature = self.feature(view, states)?;
        Ok(softmax(self.logits(feature)))
    }

    /// 损失：`-ln p(label)`。
    pub fn loss(&self, sample: &ClassifierSample) -> Result<f32, BusError> {
        let probabilities = self.probabilities(&sample.graph, &sample.states)?;
        Ok(-(probabilities[index_of(sample.label)] + 1e-12).ln())
    }

    /// 解析梯度：返回 `(loss, ∂L/∂table_latent, ∂L/∂head)`。
    pub fn loss_and_grads(
        &self,
        sample: &ClassifierSample,
    ) -> Result<(f32, GradTable, GradHead), BusError> {
        let incoming = incoming_edges(&sample.graph, TARGET);
        let feature = self.feature(&sample.graph, &sample.states)?;
        let probabilities = softmax(self.logits(feature));
        let loss = -(probabilities[index_of(sample.label)] + 1e-12).ln();

        let mut dlogits = probabilities;
        dlogits[index_of(sample.label)] -= 1.0;

        let mut dhead = [[0.0f32; 3]; 2];
        for (k, &dlogit) in dlogits.iter().enumerate() {
            dhead[0][k] = dlogit * feature;
            dhead[1][k] = dlogit;
        }
        let dfeature: f32 = (0..3).map(|k| dlogits[k] * self.head[0][k]).sum();

        let mut dtable = [[0.0f32; 3]; 3];
        if !incoming.is_empty() {
            let scale = dfeature / incoming.len() as f32;
            for (source, edge) in &incoming {
                let probabilities = soft_state(sample.states[*source]);
                let row = &self.table.latent()[index_of(*edge)];
                for slot in 0..3 {
                    let derivative = 1.0 - finite_latent(row[slot]).tanh().powi(2);
                    dtable[index_of(*edge)][slot] += scale * probabilities[slot] * derivative;
                }
            }
        }
        Ok((loss, dtable, dhead))
    }

    /// 全批梯度下降；返回最后一轮的平均损失。
    pub fn fit(
        &mut self,
        samples: &[ClassifierSample],
        epochs: usize,
        lr: f32,
    ) -> Result<f32, BusError> {
        assert!(!samples.is_empty(), "没有训练样本");
        for sample in samples {
            check_states_len(&sample.graph, &sample.states)?;
        }
        let lr = finite_scalar(lr);
        let mut mean_loss = 0.0f32;
        for _ in 0..epochs {
            let mut dtable = [[0.0f32; 3]; 3];
            let mut dhead = [[0.0f32; 3]; 2];
            mean_loss = 0.0;
            for sample in samples {
                let (loss, table_grad, head_grad) = self.loss_and_grads(sample)?;
                mean_loss += loss;
                for (dst, src) in dtable.iter_mut().zip(table_grad) {
                    for (target, value) in dst.iter_mut().zip(src) {
                        *target += value;
                    }
                }
                for (dst, src) in dhead.iter_mut().zip(head_grad) {
                    for (target, value) in dst.iter_mut().zip(src) {
                        *target += value;
                    }
                }
            }
            let scale = lr / samples.len() as f32;
            let latent = self.table.latent_mut();
            for (e, row) in latent.iter_mut().enumerate() {
                for (s, value) in row.iter_mut().enumerate() {
                    *value = finite_latent(*value) - scale * dtable[e][s];
                }
            }
            for (row, dst) in self.head.iter_mut().enumerate() {
                for (k, value) in dst.iter_mut().enumerate() {
                    *value -= scale * dhead[row][k];
                }
            }
            mean_loss /= samples.len() as f32;
        }
        Ok(mean_loss)
    }

    /// 硬三票推理：表项取硬票，按议会合成；没有入边时保持原状态。
    pub fn predict(
        &self,
        view: &impl TriGraphView,
        states: &[Ternary],
    ) -> Result<Ternary, BusError> {
        check_states_len(view, states)?;
        let incoming = incoming_edges(view, TARGET);
        if incoming.is_empty() {
            return Ok(states[TARGET]);
        }
        let votes: Vec<(Ternary, bool)> = incoming
            .iter()
            .map(|(source, edge)| (self.table.vote(*edge, states[*source]), false))
            .collect();
        Ok(tally(votes).outcome)
    }
}

fn incoming_edges<G: TriGraphView>(view: &G, target: usize) -> Vec<(usize, Ternary)> {
    let mut incoming = Vec::new();
    for source in 0..view.len() {
        view.for_each_out(source, |hit, edge| {
            if hit == target {
                incoming.push((source, edge));
            }
        });
    }
    incoming
}

fn soft_state(state: Ternary) -> [f32; 3] {
    match state {
        Ternary::True => [1.0, 0.0, 0.0],
        Ternary::Unknown => [1.0 / 3.0; 3],
        Ternary::False => [0.0, 0.0, 1.0],
    }
}

fn index_of(state: Ternary) -> usize {
    match state {
        Ternary::True => 0,
        Ternary::Unknown => 1,
        Ternary::False => 2,
    }
}

/// 生成"一跳到目标节点"的合成任务：真实标签 = 恒等规则下的议会结果。
#[doc(hidden)]
pub fn synthetic_samples(count: usize, seed: u64) -> Vec<ClassifierSample> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % 3) as i8 - 1
    };
    let identity = TransitionTable::identity();
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let n = 4usize;
        let mut graph = TritGraph::new(n);
        let mut states = vec![Ternary::Unknown; n];
        let mut votes = Vec::new();
        for (source, slot) in states.iter_mut().enumerate().skip(1) {
            let node = trit(next());
            let edge = trit(next());
            *slot = node;
            graph.set_edge(source, TARGET, Some(edge));
            votes.push((identity.vote(edge, node), false));
        }
        let label = tally(votes).outcome;
        samples.push(ClassifierSample {
            graph,
            states,
            label,
        });
    }
    samples
}

fn trit(value: i8) -> Ternary {
    match value {
        -1 => Ternary::False,
        1 => Ternary::True,
        _ => Ternary::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand_sample(edges: &[(Ternary, Ternary)], label: Ternary) -> ClassifierSample {
        let mut graph = TritGraph::new(edges.len() + 1);
        let mut states = vec![Ternary::Unknown; edges.len() + 1];
        for (i, &(edge, node)) in edges.iter().enumerate() {
            states[i + 1] = node;
            graph.set_edge(i + 1, TARGET, Some(edge));
        }
        ClassifierSample {
            graph,
            states,
            label,
        }
    }

    #[test]
    fn hard_predict_uses_identity_table() {
        let c = TernaryClassifier::with_table(TransitionTable::identity());
        let s = hand_sample(&[(Ternary::True, Ternary::True)], Ternary::True);
        assert_eq!(c.predict(&s.graph, &s.states).unwrap(), Ternary::True);
        let o = hand_sample(&[(Ternary::False, Ternary::True)], Ternary::False);
        assert_eq!(c.predict(&o.graph, &o.states).unwrap(), Ternary::False);
        let n = hand_sample(&[(Ternary::Unknown, Ternary::True)], Ternary::Unknown);
        assert_eq!(c.predict(&n.graph, &n.states).unwrap(), Ternary::Unknown);
    }

    #[test]
    fn gradients_match_finite_differences() {
        let sample = hand_sample(
            &[
                (Ternary::True, Ternary::True),
                (Ternary::False, Ternary::False),
            ],
            Ternary::True,
        );
        let classifier = TernaryClassifier::new();
        let (_, dtable, dhead) = classifier.loss_and_grads(&sample).unwrap();
        let epsilon = 1e-4;
        let mut probe = classifier.clone();
        for (e, row) in dtable.iter().enumerate() {
            for (s, &analytic) in row.iter().enumerate() {
                let base = probe.table.latent()[e][s];
                probe.table.latent_mut()[e][s] = base + epsilon;
                let plus = probe.loss(&sample).unwrap();
                probe.table.latent_mut()[e][s] = base - epsilon;
                let minus = probe.loss(&sample).unwrap();
                probe.table.latent_mut()[e][s] = base;
                let numeric = (plus - minus) / (2.0 * epsilon);
                assert!(
                    (numeric - analytic).abs() < 1e-3,
                    "t[{e}][{s}] {numeric}/{analytic}"
                );
            }
        }
        for (r, row) in dhead.iter().enumerate() {
            for (k, &analytic) in row.iter().enumerate() {
                let base = probe.head[r][k];
                probe.head[r][k] = base + epsilon;
                let plus = probe.loss(&sample).unwrap();
                probe.head[r][k] = base - epsilon;
                let minus = probe.loss(&sample).unwrap();
                probe.head[r][k] = base;
                let numeric = (plus - minus) / (2.0 * epsilon);
                assert!(
                    (numeric - analytic).abs() < 1e-3,
                    "h[{r}][{k}] {numeric}/{analytic}"
                );
            }
        }
    }

    #[test]
    fn training_beats_sign_blind_baseline() {
        let samples = synthetic_samples(300, 0x9E37_79B9_7F4A_7C15);
        let mut classifier = TernaryClassifier::new();
        let loss = classifier.fit(&samples, 300, 0.5).unwrap();
        assert!(loss < 0.7, "final loss={loss}");
        let correct = samples
            .iter()
            .filter(|s| classifier.predict(&s.graph, &s.states).unwrap() == s.label)
            .count();
        let accuracy = correct as f64 / samples.len() as f64;
        let baseline = samples
            .iter()
            .filter(|s| {
                let votes: Vec<(Ternary, bool)> =
                    s.states.iter().skip(1).map(|&st| (st, false)).collect();
                tally(votes).outcome == s.label
            })
            .count() as f64
            / samples.len() as f64;
        assert!(accuracy > 0.9, "accuracy={accuracy}");
        assert!(
            accuracy > baseline + 0.05,
            "accuracy={accuracy} baseline={baseline}"
        );
    }

    #[test]
    fn isolated_target_keeps_state() {
        let classifier = TernaryClassifier::with_table(TransitionTable::identity());
        let graph = TritGraph::new(1);
        assert_eq!(
            classifier.predict(&graph, &[Ternary::False]).unwrap(),
            Ternary::False
        );
    }
}
