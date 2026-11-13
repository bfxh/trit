//! 影子学习：在回放语料上评估候选策略，只推荐 `unsafe_extra` 为零的策略。
//!
//! 学习只产出推荐，不改内核决定；同一语料与候选集合必须得到同一结果。

use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{Error, ShadowPolicy, ShadowReport, TraceStep, policy::builtin, replay};

/// 单个策略在整段语料上的回放评估。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEvaluation {
    pub report: ShadowReport,
}

impl PolicyEvaluation {
    pub fn from_report(report: ShadowReport) -> Self {
        Self { report }
    }

    pub fn policy(&self) -> &str {
        &self.report.policy
    }

    pub fn is_safe(&self) -> bool {
        self.report.is_safe()
    }

    /// 与内核完全一致的步数占比（basis points）。
    pub fn agreement_bp(&self) -> u32 {
        if self.report.steps == 0 {
            return 0;
        }
        ((self.report.agreed_steps as u64 * 10_000) / self.report.steps as u64) as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyLearnDeferReason {
    EmptyCorpus,
    NoCandidates,
    NoSafeCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyLearnDeferral {
    pub reason: PolicyLearnDeferReason,
    pub evaluations: Vec<PolicyEvaluation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRecommendation {
    pub chosen: PolicyEvaluation,
    /// 因存在危险额外派发而被排除的策略名（按名字排序）。
    pub excluded_unsafe: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyLearning {
    Recommended(PolicyRecommendation),
    Deferred(PolicyLearnDeferral),
}

/// 在语料上逐策略回放；评估顺序跟随候选顺序。
pub fn evaluate_policies(
    steps: &[TraceStep],
    policies: &[&dyn ShadowPolicy],
) -> Result<Vec<PolicyEvaluation>, Error> {
    policies
        .iter()
        .map(|policy| replay(*policy, steps).map(PolicyEvaluation::from_report))
        .collect()
}

pub fn learn_policy(
    steps: &[TraceStep],
    policies: &[&dyn ShadowPolicy],
) -> Result<PolicyLearning, Error> {
    Ok(choose_policy(&evaluate_policies(steps, policies)?))
}

pub fn learn_builtin_policies(
    steps: &[TraceStep],
    names: &[&str],
) -> Result<PolicyLearning, Error> {
    let owned = resolve_builtin(names)?;
    let refs: Vec<&dyn ShadowPolicy> = owned.iter().map(|policy| policy.as_ref()).collect();
    learn_policy(steps, &refs)
}

fn resolve_builtin(names: &[&str]) -> Result<Vec<Box<dyn ShadowPolicy>>, Error> {
    names
        .iter()
        .map(|name| {
            builtin(name).ok_or_else(|| Error::UnknownShadowPolicy {
                name: (*name).to_owned(),
            })
        })
        .collect()
}

/// 增量学习器：跨批次累积同一组候选策略的评估，安全历史不会被新批次洗掉。
///
/// `windowed` 模式只看最近若干步，安全性随窗口有界；默认模式永久保留安全历史。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowLearner {
    reports: BTreeMap<String, ShadowReport>,
    #[serde(default)]
    batches: VecDeque<Batch>,
    #[serde(default)]
    window: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Batch {
    steps: usize,
    reports: BTreeMap<String, ShadowReport>,
}

impl ShadowLearner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 只看最近 `window` 步的评测器；窗口按整批丢弃，实际保留不超过窗口步数。
    ///
    /// 与默认累计模式不同，窗口模式的安全历史也会随旧批次遗忘。
    pub fn windowed(window: usize) -> Result<Self, Error> {
        if window == 0 {
            return Err(Error::LearnWindowZero);
        }
        Ok(Self {
            window: Some(window),
            ..Self::default()
        })
    }

    pub fn window(&self) -> Option<usize> {
        self.window
    }

    /// 累积一批语料。同一学习器只能观察同一组策略；候选集合变化必须先 `reset`。
    pub fn observe(
        &mut self,
        steps: &[TraceStep],
        policies: &[&dyn ShadowPolicy],
    ) -> Result<(), Error> {
        self.observe_weighted(steps, policies, 1)
    }

    /// 带样本权重的累积：权重 `w` 等价于把该批语料回放 `w` 次。
    pub fn observe_weighted(
        &mut self,
        steps: &[TraceStep],
        policies: &[&dyn ShadowPolicy],
        weight: u32,
    ) -> Result<(), Error> {
        if weight == 0 || policies.is_empty() {
            return Ok(());
        }
        let evaluations = evaluate_policies(steps, policies)?;
        let mut batch: BTreeMap<String, ShadowReport> = BTreeMap::new();
        for evaluation in evaluations {
            let name = evaluation.policy().to_owned();
            if batch.contains_key(&name) {
                return Err(Error::DuplicateShadowPolicy { name });
            }
            batch.insert(name, evaluation.report);
        }
        let actual: Vec<String> = batch.keys().cloned().collect();
        let expected: Vec<String> = self.reports.keys().cloned().collect();
        if !expected.is_empty() && expected != actual {
            return Err(Error::LearnPolicySetChanged { expected, actual });
        }
        self.absorb_observation(&batch, weight);
        Ok(())
    }

    pub fn observe_builtin(&mut self, steps: &[TraceStep], names: &[&str]) -> Result<(), Error> {
        self.observe_builtin_weighted(steps, names, 1)
    }

    pub fn observe_builtin_weighted(
        &mut self,
        steps: &[TraceStep],
        names: &[&str],
        weight: u32,
    ) -> Result<(), Error> {
        let owned = resolve_builtin(names)?;
        let refs: Vec<&dyn ShadowPolicy> = owned.iter().map(|policy| policy.as_ref()).collect();
        self.observe_weighted(steps, &refs, weight)
    }

    pub fn report(&self, policy: &str) -> Option<&ShadowReport> {
        self.reports.get(policy)
    }

    /// 合并另一台宿主的学习状态；两边必须观察同一组策略。
    ///
    /// 假设两边的语料互不重叠；重复合并同一批语料会重复计数。
    /// 合并后每条策略的累计报告等价于“先回放本机语料、再回放对方语料”。
    pub fn merge(&mut self, other: &ShadowLearner) -> Result<(), Error> {
        self.merge_weighted(other, 1)
    }

    /// 带权重的合并：权重 `w` 等价于把对方语料再回放 `w` 次。
    pub fn merge_weighted(&mut self, other: &ShadowLearner, weight: u32) -> Result<(), Error> {
        if weight == 0 || other.reports.is_empty() {
            return Ok(());
        }
        if self.reports.is_empty() && self.batches.is_empty() {
            *self = other.clone();
            if weight == 1 {
                return Ok(());
            }
            self.repeat(other, weight - 1);
            return Ok(());
        }
        if self.window != other.window {
            return Err(Error::LearnWindowMismatch {
                expected: self.window,
                actual: other.window,
            });
        }
        let expected: Vec<String> = self.reports.keys().cloned().collect();
        let actual: Vec<String> = other.reports.keys().cloned().collect();
        if expected != actual {
            return Err(Error::LearnPolicySetChanged { expected, actual });
        }
        self.repeat(other, weight);
        Ok(())
    }

    fn absorb_observation(&mut self, batch: &BTreeMap<String, ShadowReport>, weight: u32) {
        let steps = batch.values().next().map_or(0, |report| report.steps);
        for _ in 0..weight {
            if self.window.is_some() && steps > 0 {
                self.batches.push_back(Batch {
                    steps,
                    reports: batch.clone(),
                });
            }
            merge_batch(&mut self.reports, batch);
        }
        self.trim();
    }

    fn repeat(&mut self, other: &ShadowLearner, times: u32) {
        match self.window {
            Some(_) => {
                for _ in 0..times {
                    self.batches.extend(other.batches.iter().cloned());
                }
                self.rebuild();
                self.trim();
            }
            None => {
                for _ in 0..times {
                    merge_batch(&mut self.reports, &other.reports);
                }
            }
        }
    }

    fn rebuild(&mut self) {
        let mut reports = BTreeMap::new();
        for batch in &self.batches {
            merge_batch(&mut reports, &batch.reports);
        }
        self.reports = reports;
    }

    fn trim(&mut self) {
        let Some(window) = self.window else {
            return;
        };
        let mut total: usize = self.batches.iter().map(|batch| batch.steps).sum();
        if total <= window {
            return;
        }
        while total > window {
            let Some(front) = self.batches.pop_front() else {
                break;
            };
            total -= front.steps;
        }
        self.rebuild();
    }

    pub fn evaluations(&self) -> Vec<PolicyEvaluation> {
        self.reports
            .values()
            .cloned()
            .map(PolicyEvaluation::from_report)
            .collect()
    }

    pub fn learn(&self) -> PolicyLearning {
        choose_policy(&self.evaluations())
    }

    pub fn reset(&mut self) {
        self.reports.clear();
        self.batches.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }
}

fn merge_batch(into: &mut BTreeMap<String, ShadowReport>, batch: &BTreeMap<String, ShadowReport>) {
    for (policy, report) in batch {
        match into.get_mut(policy) {
            Some(existing) => merge_report(existing, report),
            None => {
                into.insert(policy.clone(), report.clone());
            }
        }
    }
}

fn merge_report(into: &mut ShadowReport, from: &ShadowReport) {
    let offset = into.steps;
    into.steps += from.steps;
    into.agreed_steps += from.agreed_steps;
    into.order_mismatches += from.order_mismatches;
    into.stop_requests += from.stop_requests;
    into.rejected.extend(offset_entries(&from.rejected, offset));
    into.unsafe_extra
        .extend(offset_entries(&from.unsafe_extra, offset));
    into.skipped.extend(offset_entries(&from.skipped, offset));
}

/// 合并后把批次内序号平移到累计序号，保证与拼接语料直接回放的结果一致。
fn offset_entries(entries: &[String], offset: usize) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry.split_once(':') {
            Some((index, rest)) => match index.parse::<usize>() {
                Ok(index) => format!("{}:{rest}", index + offset),
                Err(_) => format!("{offset}:{entry}"),
            },
            None => format!("{offset}:{entry}"),
        })
        .collect()
}

/// 从评估结果里选最优安全策略：一致率优先，其次少漏派 / 少争议，最后按名字定序。
pub fn choose_policy(evaluations: &[PolicyEvaluation]) -> PolicyLearning {
    if evaluations.is_empty() {
        return PolicyLearning::Deferred(PolicyLearnDeferral {
            reason: PolicyLearnDeferReason::NoCandidates,
            evaluations: Vec::new(),
        });
    }
    if evaluations
        .iter()
        .all(|evaluation| evaluation.report.steps == 0)
    {
        return PolicyLearning::Deferred(PolicyLearnDeferral {
            reason: PolicyLearnDeferReason::EmptyCorpus,
            evaluations: evaluations.to_vec(),
        });
    }

    let mut excluded_unsafe: Vec<String> = evaluations
        .iter()
        .filter(|evaluation| !evaluation.is_safe())
        .map(|evaluation| evaluation.policy().to_owned())
        .collect();
    excluded_unsafe.sort_unstable();
    excluded_unsafe.dedup();

    let winner = evaluations
        .iter()
        .filter(|evaluation| evaluation.is_safe())
        .max_by(|left, right| compare(left, right));
    match winner {
        Some(chosen) => PolicyLearning::Recommended(PolicyRecommendation {
            chosen: chosen.clone(),
            excluded_unsafe,
        }),
        None => PolicyLearning::Deferred(PolicyLearnDeferral {
            reason: PolicyLearnDeferReason::NoSafeCandidate,
            evaluations: evaluations.to_vec(),
        }),
    }
}

fn compare(left: &PolicyEvaluation, right: &PolicyEvaluation) -> Ordering {
    left.agreement_bp()
        .cmp(&right.agreement_bp())
        .then_with(|| right.report.skipped.len().cmp(&left.report.skipped.len()))
        .then_with(|| {
            right
                .report
                .order_mismatches
                .cmp(&left.report.order_mismatches)
        })
        .then_with(|| right.report.rejected.len().cmp(&left.report.rejected.len()))
        .then_with(|| right.report.stop_requests.cmp(&left.report.stop_requests))
        .then_with(|| right.policy().cmp(left.policy()))
}
