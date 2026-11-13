//! 编排指标快照：并发、token / 缓存命中、选批压力与引导遵循。
//!
//! 指标只读状态，不参与决策；墙钟耗时由宿主在外部测量，内核保持确定性。

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::{BlockReason, Limits, Phase, Resources, Snapshot, Ternary, Tick, TokenUsage};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationMetrics {
    pub tasks_total: usize,
    pub tasks_selected: usize,
    pub tasks_unknown: usize,
    pub tasks_denied: usize,
    pub tasks_in_flight: usize,
    pub workers_limit: usize,
    pub workers_occupied: usize,
    /// 已占 worker / worker 上限（基点）。
    pub concurrency_pressure_bp: u32,
    /// 选中任务 / 评估任务（基点）。
    pub selection_pressure_bp: u32,
    pub tokens_fresh: u64,
    pub tokens_cached: u64,
    pub tokens_output: u64,
    /// `cached / (fresh + cached)`（基点）；没有输入 token 时为 0。
    pub cache_hit_rate_bp: u32,
    pub observed_tokens: u64,
    pub reserved_tokens: u64,
    pub granted_tokens: u64,
    pub reserved_resources: Resources,
    pub briefs: usize,
    pub adherence_reports: usize,
    pub violations: usize,
    pub interventions: usize,
    pub held: usize,
    pub obligations: usize,
    pub guidance_violation_rate_bp: u32,
    /// 阻塞原因计数，按稳定短名聚合。
    pub blockers: BTreeMap<String, usize>,
}

impl OrchestrationMetrics {
    pub const DEFAULT_PROMETHEUS_PREFIX: &'static str = "trit_orch";

    pub fn capture(tick: &Tick, snapshot: &Snapshot, limits: &Limits) -> Self {
        let selection = &tick.selection;
        let mut tasks_unknown = 0;
        let mut tasks_denied = 0;
        let mut blockers = BTreeMap::new();
        for decision in &selection.decisions {
            match decision.admission {
                Ternary::Unknown => tasks_unknown += 1,
                Ternary::False => tasks_denied += 1,
                Ternary::True => {}
            }
            for reason in &decision.reasons {
                *blockers.entry(blocker_kind(reason).to_owned()).or_insert(0) += 1;
            }
        }

        let usage = usage_totals(snapshot);
        let input_tokens = usage.fresh.saturating_add(usage.cached);
        let cache_hit_rate_bp = if input_tokens == 0 {
            0
        } else {
            ((u128::from(usage.cached) * 10_000) / u128::from(input_tokens)) as u32
        };
        let granted_tokens = selection.decisions.iter().fold(0u64, |sum, decision| {
            sum.saturating_add(decision.granted_tokens)
        });
        let violations: usize = tick
            .adherence
            .iter()
            .map(|report| report.violations.len())
            .sum();

        Self {
            tasks_total: selection.decisions.len(),
            tasks_selected: selection.selected.len(),
            tasks_unknown,
            tasks_denied,
            tasks_in_flight: snapshot
                .observations
                .values()
                .filter(|observation| observation.phase == Phase::InFlight)
                .count(),
            workers_limit: limits.workers,
            workers_occupied: selection.occupied_workers,
            concurrency_pressure_bp: ratio_bp(selection.occupied_workers, limits.workers),
            selection_pressure_bp: ratio_bp(selection.selected.len(), selection.decisions.len()),
            tokens_fresh: usage.fresh,
            tokens_cached: usage.cached,
            tokens_output: usage.output,
            cache_hit_rate_bp,
            observed_tokens: usage
                .fresh
                .saturating_add(usage.cached)
                .saturating_add(usage.output),
            reserved_tokens: selection.reserved_tokens,
            granted_tokens,
            reserved_resources: selection.reserved_resources,
            briefs: tick.briefs.len(),
            adherence_reports: tick.adherence.len(),
            violations,
            interventions: tick.interventions.interventions.len(),
            held: tick.interventions.held.len(),
            obligations: tick.obligations.len(),
            guidance_violation_rate_bp: tick.guidance_metrics.violation_rate_bp(),
            blockers,
        }
    }

    pub fn to_prometheus(&self) -> String {
        self.to_prometheus_with(Self::DEFAULT_PROMETHEUS_PREFIX)
    }

    pub fn to_prometheus_with(&self, prefix: &str) -> String {
        let prefix = if prefix.is_empty() {
            Self::DEFAULT_PROMETHEUS_PREFIX
        } else {
            prefix
        };
        let mut out = String::new();
        for (name, help, value) in [
            (
                "tasks_total",
                "Tasks evaluated in this tick.",
                self.tasks_total as u64,
            ),
            (
                "tasks_selected",
                "Tasks selected in this tick.",
                self.tasks_selected as u64,
            ),
            (
                "tasks_unknown",
                "Tasks with Unknown admission.",
                self.tasks_unknown as u64,
            ),
            (
                "tasks_denied",
                "Tasks with False admission.",
                self.tasks_denied as u64,
            ),
            (
                "tasks_in_flight",
                "Observed in-flight tasks.",
                self.tasks_in_flight as u64,
            ),
            (
                "workers_limit",
                "Configured worker limit.",
                self.workers_limit as u64,
            ),
            (
                "workers_occupied",
                "Workers occupied after selection.",
                self.workers_occupied as u64,
            ),
            (
                "concurrency_pressure_bp",
                "Occupied workers / limit (basis points).",
                self.concurrency_pressure_bp as u64,
            ),
            (
                "selection_pressure_bp",
                "Selected tasks / evaluated tasks (basis points).",
                self.selection_pressure_bp as u64,
            ),
            (
                "tokens_fresh",
                "Fresh input tokens observed.",
                self.tokens_fresh,
            ),
            (
                "tokens_cached",
                "Cached input tokens observed.",
                self.tokens_cached,
            ),
            (
                "tokens_output",
                "Output tokens observed.",
                self.tokens_output,
            ),
            (
                "cache_hit_rate_bp",
                "Cached / (fresh + cached) input tokens (basis points).",
                self.cache_hit_rate_bp as u64,
            ),
            (
                "observed_tokens",
                "Tokens observed before selection.",
                self.observed_tokens,
            ),
            (
                "reserved_tokens",
                "Tokens reserved by selection.",
                self.reserved_tokens,
            ),
            (
                "granted_tokens",
                "Tokens granted to selected tasks.",
                self.granted_tokens,
            ),
            (
                "reserved_cpu",
                "Reserved CPU units.",
                u64::from(self.reserved_resources.cpu),
            ),
            (
                "reserved_memory",
                "Reserved memory units.",
                u64::from(self.reserved_resources.memory),
            ),
            ("briefs", "Briefs issued in this tick.", self.briefs as u64),
            (
                "adherence_reports",
                "Adherence reports in this tick.",
                self.adherence_reports as u64,
            ),
            (
                "violations",
                "Adherence violations in this tick.",
                self.violations as u64,
            ),
            (
                "interventions",
                "Interventions planned in this tick.",
                self.interventions as u64,
            ),
            ("held", "Held tasks in this tick.", self.held as u64),
            (
                "obligations",
                "Obligations collected in this tick.",
                self.obligations as u64,
            ),
            (
                "guidance_violation_rate_bp",
                "Briefs with violations / reports (basis points).",
                self.guidance_violation_rate_bp as u64,
            ),
        ] {
            let _ = writeln!(out, "# HELP {prefix}_{name} {help}");
            let _ = writeln!(out, "# TYPE {prefix}_{name} gauge");
            let _ = writeln!(out, "{prefix}_{name} {value}");
        }
        let _ = writeln!(
            out,
            "# HELP {prefix}_blocker_reasons Decision blockers by reason."
        );
        let _ = writeln!(out, "# TYPE {prefix}_blocker_reasons gauge");
        for (reason, count) in &self.blockers {
            let _ = writeln!(
                out,
                "{prefix}_blocker_reasons{{reason=\"{reason}\"}} {count}"
            );
        }
        out
    }
}

fn usage_totals(snapshot: &Snapshot) -> TokenUsage {
    let mut totals = TokenUsage::default();
    for observation in snapshot.observations.values() {
        totals.fresh = totals.fresh.saturating_add(observation.usage.fresh);
        totals.cached = totals.cached.saturating_add(observation.usage.cached);
        totals.output = totals.output.saturating_add(observation.usage.output);
    }
    totals
}

fn ratio_bp(numerator: usize, denominator: usize) -> u32 {
    if denominator == 0 {
        return 0;
    }
    ((numerator as u128 * 10_000) / denominator as u128) as u32
}

fn blocker_kind(reason: &BlockReason) -> &'static str {
    match reason {
        BlockReason::Dependency { .. } => "dependency",
        BlockReason::Authorization { .. } => "authorization",
        BlockReason::Freshness { .. } => "freshness",
        BlockReason::Claim { .. } => "claim",
        BlockReason::Effect { .. } => "effect",
        BlockReason::Conflict { .. } => "conflict",
        BlockReason::ResourceCapacity { .. } => "resource_capacity",
        BlockReason::Resources { .. } => "resources",
        BlockReason::Workers { .. } => "workers",
        BlockReason::TokenCapacity { .. } => "token_capacity",
        BlockReason::Tokens { .. } => "tokens",
        BlockReason::InFlight => "in_flight",
        BlockReason::Settled => "settled",
        BlockReason::Superseded { .. } => "superseded",
    }
}
