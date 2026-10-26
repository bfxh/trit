//! 端到端三态节点分类 M0：一层软松弛训练 + 硬三票推理。
//!
//! 任务形态：给一张有向图和初始节点三态，预测**目标节点**（固定 0 号）的三态。
//! 模型只有一层消息传递：
//! - 软前向（训练）：邻居状态做 one-hot/均匀分布，表项取 `tanh(latent)`，聚合后过三叉头；
//! - 硬前向（推理）：表项取硬三票，按议会合成——与 [`crate::council`] 同一规则。
//!
//! 训练是**解析梯度**（不是有限差分）：`tanh → mean → 线性头 → softmax → CE`。
//! 图级池化/多层/边特征在后续里程碑。

use trit_core::value::Ternary;

use crate::council::tally;
use crate::graph::TritGraph;
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
    pub fn feature(&self, view: &impl TriGraphView, states: &[Ternary]) -> f32 {
        let incoming = incoming_edges(view, TARGET);
        if incoming.is_empty() {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for (source, edge) in &incoming {
            let probabilities = soft_state(states[*source]);
            let row = &self.table.latent()[index_of(*edge)];
            for (slot, &probability) in probabilities.iter().enumerate() {
                sum += probability * row[slot].tanh();
            }
        }
        sum / incoming.len() as f32
    }

    fn logits(&self, feature: f32) -> [f32; 3] {
        [
            self.head[0][0] * feature + self.head[1][0],
            self.head[0][1] * feature + self.head[1][1],
            self.head[0][2] * feature + self.head[1][2],
        ]
    }

    /// 软概率（`[+1, 0, -1]` 三类）。
    pub fn probabilities(&self, view: &impl TriGraphView, states: &[Ternary]) -> [f32; 3] {
        softmax(self.logits(self.feature(view, states)))
    }

    /// 损失：`-ln p(label)`。
    pub fn loss(&self, sample: &ClassifierSample) -> f32 {
        let probabilities = self.probabilities(&sample.graph, &sample.states);
        -(probabilities[index_of(sample.label)] + 1e-12).ln()
    }

    /// 解析梯度：返回 `(loss, ∂L/∂table_latent, ∂L/∂head)`。
    pub fn loss_and_grads(&self, sample: &ClassifierSample) -> (f32, [[f32; 3]; 3], [[f32; 3]; 2]) {
        let incoming = incoming_edges(&sample.graph, TARGET);
        let feature = self.feature(&sample.graph, &sample.states);
        let probabilities = softmax(self.logits(feature));
        let loss = -(probabilities[index_of(sample.label)] + 1e-12).ln();

        let mut dlogits = probabilities;
        dlogits[index_of(sample.label)] -= 1.0;

        let mut dhead = [[0.0f32; 3]; 2];
        for k in 0..3 {
            dhead[0][k] = dlogits[k] * feature;
            dhead[1][k] = dlogits[k];
        }
        let dfeature: f32 = (0..3).map(|k| dlogits[k] * self.head[0][k]).sum();

        let mut dtable = [[0.0f32; 3]; 3];
        if !incoming.is_empty() {
            let scale = dfeature / incoming.len() as f32;
            for (source, edge) in &incoming {
                let probabilities = soft_state(sample.states[*source]);
                let row = &self.table.latent()[index_of(*edge)];
                for slot in 0..3 {
                    let derivative = 1.0 - row[slot].tanh().powi(2);
                    dtable[index_of(*edge)][slot] += scale * probabilities[slot] * derivative;
                }
            }
        }
        (loss, dtable, dhead)
    }

    /// 全批梯度下降；返回最后一轮的平均损失。
    pub fn fit(&mut self, samples: &[ClassifierSample], epochs: usize, lr: f32) -> f32 {
        assert!(!samples.is_empty(), "没有训练样本");
        let mut mean_loss = 0.0f32;
        for _ in 0..epochs {
            let mut dtable = [[0.0f32; 3]; 3];
            let mut dhead = [[0.0f32; 3]; 2];
            mean_loss = 0.0;
            for sample in samples {
                let (loss, table_grad, head_grad) = self.loss_and_grads(sample);
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
                    *value -= scale * dtable[e][s];
                }
            }
            for (row, dst) in self.head.iter_mut().enumerate() {
                for (k, value) in dst.iter_mut().enumerate() {
                    *value -= scale * dhead[row][k];
                }
            }
            mean_loss /= samples.len() as f32;
        }
        mean_loss
    }

    /// 硬三票推理：表项取硬票，按议会合成；没有入边时保持原状态。
    pub fn predict(&self, view: &impl TriGraphView, states: &[Ternary]) -> Ternary {
        let incoming = incoming_edges(view, TARGET);
        if incoming.is_empty() {
            return states[TARGET];
        }
        let votes: Vec<(Ternary, bool)> = incoming
            .iter()
            .map(|(source, edge)| (self.table.vote(*edge, states[*source]), false))
            .collect();
        tally(votes).outcome
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

fn softmax(logits: [f32; 3]) -> [f32; 3] {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: [f32; 3] = logits.map(|value| (value - max).exp());
    let sum: f32 = exp.iter().sum();
    exp.map(|value| value / sum)
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
        let classifier = TernaryClassifier::with_table(TransitionTable::identity());
        let support = hand_sample(&[(Ternary::True, Ternary::True)], Ternary::True);
        assert_eq!(
            classifier.predict(&support.graph, &support.states),
            Ternary::True
        );
        let oppose = hand_sample(&[(Ternary::False, Ternary::True)], Ternary::False);
        assert_eq!(
            classifier.predict(&oppose.graph, &oppose.states),
            Ternary::False
        );
        let neutral = hand_sample(&[(Ternary::Unknown, Ternary::True)], Ternary::Unknown);
        assert_eq!(
            classifier.predict(&neutral.graph, &neutral.states),
            Ternary::Unknown
        );
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
        let (_, dtable, dhead) = classifier.loss_and_grads(&sample);
        let epsilon = 1e-4;

        let mut probe = classifier.clone();
        for (e, row) in dtable.iter().enumerate() {
            for (s, &analytic) in row.iter().enumerate() {
                let base = probe.table.latent()[e][s];
                probe.table.latent_mut()[e][s] = base + epsilon;
                let plus = probe.loss(&sample);
                probe.table.latent_mut()[e][s] = base - epsilon;
                let minus = probe.loss(&sample);
                probe.table.latent_mut()[e][s] = base;
                let numeric = (plus - minus) / (2.0 * epsilon);
                assert!(
                    (numeric - analytic).abs() < 1e-3,
                    "table[{e}][{s}] numeric={numeric} analytic={analytic}"
                );
            }
        }
        for (row, analytic_row) in dhead.iter().enumerate() {
            for (k, &analytic) in analytic_row.iter().enumerate() {
                let base = probe.head[row][k];
                probe.head[row][k] = base + epsilon;
                let plus = probe.loss(&sample);
                probe.head[row][k] = base - epsilon;
                let minus = probe.loss(&sample);
                probe.head[row][k] = base;
                let numeric = (plus - minus) / (2.0 * epsilon);
                assert!(
                    (numeric - analytic).abs() < 1e-3,
                    "head[{row}][{k}] numeric={numeric} analytic={analytic}"
                );
            }
        }
    }

    #[test]
    fn training_beats_sign_blind_baseline() {
        let samples = synthetic_samples(300, 0x9E37_79B9_7F4A_7C15);
        let mut classifier = TernaryClassifier::new();
        let loss = classifier.fit(&samples, 300, 0.5);
        assert!(loss < 0.7, "final loss={loss}");

        let correct = samples
            .iter()
            .filter(|sample| classifier.predict(&sample.graph, &sample.states) == sample.label)
            .count();
        let accuracy = correct as f64 / samples.len() as f64;

        // 基线：忽略边符号，只看邻居节点状态的多数票。
        let baseline = samples
            .iter()
            .filter(|sample| {
                let votes: Vec<(Ternary, bool)> = sample
                    .states
                    .iter()
                    .skip(1)
                    .map(|&state| (state, false))
                    .collect();
                tally(votes).outcome == sample.label
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
            classifier.predict(&graph, &[Ternary::False]),
            Ternary::False
        );
    }
}
