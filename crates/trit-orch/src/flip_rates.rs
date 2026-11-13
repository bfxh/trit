//! 用回放语料估计 P(flip)：同类义务把 Unknown 翻成 T/F 的概率。
//!
//! 样本来自轨迹里的 `ObligationFulfilled` + 同一 task 前后两条 `JudgmentDerived`：
//! 前一条必须是 `Neither`，后一条是 `Proved` / `Refuted` 才算 flip。

use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{
    AdmissionObligation, Error, EvidenceVerdict, ObligationKind, ObligationOutcome, TraceLog,
    TracePayload,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FlipCounts {
    resolved: u64,
    flipped: u64,
}

/// 每个义务类型的 Beta 先验（伪计数）；默认 Beta(1,1) 即拉普拉斯平滑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlipPrior {
    alpha: u32,
    beta: u32,
}

impl Default for FlipPrior {
    fn default() -> Self {
        Self::uniform()
    }
}

impl FlipPrior {
    pub const fn new(alpha: u32, beta: u32) -> Self {
        Self { alpha, beta }
    }

    pub const fn uniform() -> Self {
        Self::new(1, 1)
    }

    pub const fn alpha(self) -> u32 {
        self.alpha
    }

    pub const fn beta(self) -> u32 {
        self.beta
    }

    /// 先验均值（基点）；总强度为 0 时回退到均匀 5000。
    pub fn mean_bp(self) -> u32 {
        let total = u64::from(self.alpha) + u64::from(self.beta);
        if total == 0 {
            return 5_000;
        }
        ((u64::from(self.alpha) * 10_000) / total) as u32
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlipRates {
    counts: BTreeMap<ObligationKind, FlipCounts>,
    #[serde(default)]
    priors: BTreeMap<ObligationKind, FlipPrior>,
    #[serde(default)]
    window: Option<usize>,
    #[serde(default)]
    history: BTreeMap<ObligationKind, VecDeque<bool>>,
}

impl FlipRates {
    /// 每个 kind 只保留最近 `window` 个样本；窗口填满后先验占比稳定在
    /// `strength / (window + strength)`，不会随历史无限累积而漂移。
    pub fn windowed(window: usize) -> Result<Self, Error> {
        if window == 0 {
            return Err(Error::FlipWindowZero);
        }
        Ok(Self {
            window: Some(window),
            ..Self::default()
        })
    }

    pub fn window(&self) -> Option<usize> {
        self.window
    }

    pub fn record(&mut self, kind: ObligationKind, flipped: bool) {
        {
            let counts = self.counts.entry(kind).or_default();
            counts.resolved += 1;
            if flipped {
                counts.flipped += 1;
            }
        }
        let Some(window) = self.window else {
            return;
        };
        let history = self.history.entry(kind).or_default();
        history.push_back(flipped);
        while history.len() > window {
            let Some(old_flipped) = history.pop_front() else {
                break;
            };
            let empty = if let Some(counts) = self.counts.get_mut(&kind) {
                counts.resolved = counts.resolved.saturating_sub(1);
                if old_flipped {
                    counts.flipped = counts.flipped.saturating_sub(1);
                }
                counts.resolved == 0
            } else {
                false
            };
            if empty {
                self.counts.remove(&kind);
            }
        }
    }

    pub fn resolved(&self, kind: ObligationKind) -> u64 {
        self.counts.get(&kind).map_or(0, |counts| counts.resolved)
    }

    pub fn flipped(&self, kind: ObligationKind) -> u64 {
        self.counts.get(&kind).map_or(0, |counts| counts.flipped)
    }

    /// 按先验平滑后的 P(flip)，单位基点；默认先验为 Beta(1,1)（拉普拉斯）。
    ///
    /// 无样本时返回先验均值；先验总强度为 0 时退化为纯频率，仍无样本则返回 5000。
    pub fn probability_bp(&self, kind: ObligationKind) -> u32 {
        let counts = self.counts.get(&kind).copied().unwrap_or_default();
        let prior = self.prior(kind);
        let denominator =
            u128::from(counts.resolved) + u128::from(prior.alpha) + u128::from(prior.beta);
        if denominator == 0 {
            return 5_000;
        }
        let numerator = (u128::from(counts.flipped) + u128::from(prior.alpha)) * 10_000;
        (numerator / denominator) as u32
    }

    pub fn prior(&self, kind: ObligationKind) -> FlipPrior {
        self.priors.get(&kind).copied().unwrap_or_default()
    }

    pub fn with_prior(mut self, kind: ObligationKind, prior: FlipPrior) -> Self {
        self.priors.insert(kind, prior);
        self
    }

    /// 先验在平滑总权重中的占比（基点），用于观察漂移下先验是否压过样本。
    pub fn prior_share_bp(&self, kind: ObligationKind) -> u32 {
        let prior = self.prior(kind);
        let strength = u64::from(prior.alpha) + u64::from(prior.beta);
        let total = self.resolved(kind) + strength;
        if total == 0 {
            return 0;
        }
        (strength * 10_000 / total) as u32
    }

    /// 用校准语料估每个 kind 的先验均值，并以 `strength` 作为伪计数总量。
    ///
    /// 返回只含先验、不含观测计数的 rates；`strength = 0` 时不设先验（回退均匀）。
    /// 未见过的 kind 保持均匀先验。
    pub fn calibrated_from_trace(log: &TraceLog, strength: u32) -> Self {
        let observed = Self::from_trace(log);
        let mut rates = Self::default();
        if strength == 0 {
            return rates;
        }
        for (kind, counts) in &observed.counts {
            if counts.resolved == 0 {
                continue;
            }
            let alpha = ((u128::from(counts.flipped) * u128::from(strength)
                + u128::from(counts.resolved) / 2)
                / u128::from(counts.resolved)) as u32;
            let alpha = alpha.min(strength);
            rates
                .priors
                .insert(*kind, FlipPrior::new(alpha, strength - alpha));
        }
        rates
    }

    pub fn score(&self, obligation: &AdmissionObligation, policy: &ScorePolicy) -> u64 {
        let numerator =
            u128::from(self.probability_bp(obligation.kind)) * u128::from(obligation.priority);
        let denominator = u128::from(policy.cost(obligation));
        u64::try_from(numerator / denominator).unwrap_or(u64::MAX)
    }

    /// 从轨迹学习：只看已 Fulfilled、且 task 前 U 后 T/F 的样本。
    pub fn from_trace(log: &TraceLog) -> Self {
        let mut rates = Self::default();
        for (kind, flipped) in trace_samples(log) {
            rates.record(kind, flipped);
        }
        rates
    }

    /// 只保留最近 `window` 个样本的入轨；窗口按 kind 独立计算。
    pub fn windowed_from_trace(log: &TraceLog, window: usize) -> Result<Self, Error> {
        let mut rates = Self::windowed(window)?;
        for (kind, flipped) in trace_samples(log) {
            rates.record(kind, flipped);
        }
        Ok(rates)
    }
}

/// 按事件顺序抽取可计数的翻转样本。
fn trace_samples(log: &TraceLog) -> Vec<(ObligationKind, bool)> {
    let judgments: Vec<(u64, &str, EvidenceVerdict)> = log
        .events()
        .filter_map(|event| match event.payload.as_ref()? {
            TracePayload::Judgment {
                subject, verdict, ..
            } => Some((event.seq, subject.as_str(), *verdict)),
            _ => None,
        })
        .collect();

    let mut samples = Vec::new();
    for event in log.events() {
        let Some(TracePayload::Obligation {
            outcome: ObligationOutcome::Fulfilled,
            obligation,
        }) = event.payload.as_ref()
        else {
            continue;
        };
        let task = obligation.task.as_str();
        let Some(before) = judgments
            .iter()
            .filter(|(seq, subject, _)| *subject == task && *seq < event.seq)
            .max_by_key(|(seq, _, _)| *seq)
        else {
            continue;
        };
        if before.2 != EvidenceVerdict::Neither {
            continue;
        }
        let Some(after) = judgments
            .iter()
            .filter(|(seq, subject, _)| *subject == task && *seq > event.seq)
            .min_by_key(|(seq, _, _)| *seq)
        else {
            continue;
        };
        samples.push((
            obligation.kind,
            matches!(after.2, EvidenceVerdict::Proved | EvidenceVerdict::Refuted),
        ));
    }
    samples
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScorePolicy {
    pub risk_weight: u64,
    pub wall_weight: u64,
    pub human_weight: u64,
}

impl Default for ScorePolicy {
    fn default() -> Self {
        Self {
            risk_weight: 10,
            wall_weight: 10,
            human_weight: 1_000,
        }
    }
}

impl ScorePolicy {
    fn cost(&self, obligation: &AdmissionObligation) -> u64 {
        obligation
            .cost
            .tokens
            .saturating_add(obligation.cost.wall.saturating_mul(self.wall_weight))
            .saturating_add(u64::from(obligation.cost.risk).saturating_mul(self.risk_weight))
            .saturating_add(if obligation.cost.human {
                self.human_weight
            } else {
                0
            })
            .max(1)
    }
}

/// 评分主序：概率 × 优先级 / 成本；同分按截止期、token 成本、ID。
pub(crate) fn order_obligations_by_score(
    left: &AdmissionObligation,
    right: &AdmissionObligation,
    rates: &FlipRates,
    policy: &ScorePolicy,
) -> Ordering {
    rates
        .score(right, policy)
        .cmp(&rates.score(left, policy))
        .then_with(|| deadline_order(left.deadline, right.deadline))
        .then_with(|| left.cost.tokens.cmp(&right.cost.tokens))
        .then_with(|| left.id.cmp(&right.id))
}

fn deadline_order(left: Option<u64>, right: Option<u64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
