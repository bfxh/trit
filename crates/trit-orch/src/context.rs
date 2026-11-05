//! 长上下文压缩闸门：压缩会不可逆地改写活动上下文，所以它是一次**副作用**，
//! 放行与否由三值证据决定，而不是由摘要模型的自信决定。
//!
//! 三档按破坏性递增：`Keep` < `Prune`（只替换可回读/可重取的工具输出）
//! < `Summarize`（有损改写历史）。低档只要**未被否证**就不升级到高档。

use serde::{Deserialize, Serialize};
use trit_core::value::Ternary;

use crate::Error;

/// 阈值比例下限：低于此值就是「几乎每次都压」，摘要连摘要会立刻劣化。
pub const MIN_THRESHOLD_BP: u32 = 5_000;
/// 阈值比例上限：高于此值留给摘要自身的预算不足，容易硬溢出。
pub const MAX_THRESHOLD_BP: u32 = 9_500;
/// 未显式给阈值时沿用上游默认 0.75（dsh-compact `policy.js` 的 DEFAULT_CONFIG）。
pub const DEFAULT_THRESHOLD_BP: u32 = 7_500;

/// 供应商或宿主是否报告了上下文溢出。报错本身不构成放行依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overflow {
    #[default]
    NotReported,
    Reported,
}

/// 允许执行的动作档，按破坏性递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Keep,
    Prune,
    Summarize,
}

/// 不放行的具体原因；每个块都对应一条出口义务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionBlock {
    /// 实测 tokens 低于阈值，且没人报溢出。
    BelowThreshold,
    /// 测不到 tokens（provider 未回 usage 或宿主未接表）。
    UnmeasuredPressure,
    /// 实测低于阈值、但不低于半窗，同时供应商报溢出：两边口径互相矛盾。
    ReportDisagrees,
    /// 供应商报溢出而实测不足半窗：按误报处理，不动历史。
    SuspectedFalseOverflow,
    /// 可回读性未证实，剪枝会静默丢内容。
    RecallUnreachable,
    /// 承重事实尚未落盘，有损摘要会丢掉唯一副本。
    FactsNotFlushed,
    /// 逐字约束块不在稳定前缀里。
    ConstraintsUnpinned,
    /// 上次压缩以来没有新的落盘进展，再压就是压自己的摘要。
    NoDurableProgress,
}

/// 让 Unknown 变可执行结论的最小动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Obligation {
    MeasureTokens,
    ReconcileWindow,
    VerifyRecall,
    FlushFacts,
    PinConstraints,
    RecheckAfterProgress,
}

impl CompactionBlock {
    pub fn kind(self) -> &'static str {
        match self {
            CompactionBlock::BelowThreshold => "below_threshold",
            CompactionBlock::UnmeasuredPressure => "unmeasured_pressure",
            CompactionBlock::ReportDisagrees => "report_disagrees",
            CompactionBlock::SuspectedFalseOverflow => "suspected_false_overflow",
            CompactionBlock::RecallUnreachable => "recall_unreachable",
            CompactionBlock::FactsNotFlushed => "facts_not_flushed",
            CompactionBlock::ConstraintsUnpinned => "constraints_unpinned",
            CompactionBlock::NoDurableProgress => "no_durable_progress",
        }
    }

    pub fn obligation(self) -> Obligation {
        match self {
            CompactionBlock::BelowThreshold => Obligation::MeasureTokens,
            CompactionBlock::UnmeasuredPressure => Obligation::MeasureTokens,
            CompactionBlock::ReportDisagrees => Obligation::ReconcileWindow,
            CompactionBlock::SuspectedFalseOverflow => Obligation::MeasureTokens,
            CompactionBlock::RecallUnreachable => Obligation::VerifyRecall,
            CompactionBlock::FactsNotFlushed => Obligation::FlushFacts,
            CompactionBlock::ConstraintsUnpinned => Obligation::PinConstraints,
            CompactionBlock::NoDurableProgress => Obligation::RecheckAfterProgress,
        }
    }
}

/// 一次压缩决策能看到的**全部事实**。缺失的字段必须是 Unknown，不能默认成 False。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFacts {
    /// 上一次真实路由请求的 prompt tokens；`None` 表示测不到。
    #[serde(default)]
    pub measured_tokens: Option<u64>,
    pub context_window: u64,
    /// 触发阈值，基点（0.51 -> 5100）。整数以免把口径误差带进判定。
    #[serde(default = "default_threshold_bp")]
    pub threshold_bp: u32,
    /// 当前可剪枝（可回读或可重取）的工具输出 tokens。
    #[serde(default)]
    pub prunable_tokens: u64,
    #[serde(default)]
    pub overflow: Overflow,
    /// 被压span里的承重事实是否已有上下文之外的副本。
    pub durable_flushed: Ternary,
    /// 被压内容能否按 seq/引用读回原字节。
    pub recall_reachable: Ternary,
    /// 逐字约束块（用户原话、红线）是否在不会被改写的稳定前缀里。
    pub constraints_pinned: Ternary,
    /// 上一次压缩之后是否有新的落盘进展（节奏门）。
    pub progress_since_compaction: Ternary,
}

fn default_threshold_bp() -> u32 {
    DEFAULT_THRESHOLD_BP
}

impl Default for ContextFacts {
    /// `Ternary::default()` 是 False，四道安全门必须显式初始化成 Unknown：
    /// 没证据不等于证据表明安全。
    fn default() -> Self {
        Self {
            measured_tokens: None,
            context_window: 0,
            threshold_bp: DEFAULT_THRESHOLD_BP,
            prunable_tokens: 0,
            overflow: Overflow::NotReported,
            durable_flushed: Ternary::Unknown,
            recall_reachable: Ternary::Unknown,
            constraints_pinned: Ternary::Unknown,
            progress_since_compaction: Ternary::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionDecision {
    pub tier: Tier,
    /// True 才允许执行；Unknown 表示待补证；False 表示明确不需要或不该做。
    pub admission: Ternary,
    /// 压力判定本身，便于宿主区分「不需要压」和「想压但没批准」。
    pub need: Ternary,
    pub threshold_tokens: u64,
    pub excess_tokens: u64,
    pub blocks: Vec<CompactionBlock>,
    pub obligations: Vec<Obligation>,
}

/// 闸门主入口：纯函数，不读盘、不调模型。
pub fn compaction_gate(facts: &ContextFacts) -> Result<CompactionDecision, Error> {
    if facts.context_window == 0 {
        return Err(Error::ContextConfig {
            message: "context_window 必须大于 0，否则阈值判定没有分母".into(),
        });
    }
    if facts.threshold_bp < MIN_THRESHOLD_BP || facts.threshold_bp > MAX_THRESHOLD_BP {
        return Err(Error::ContextConfig {
            message: format!(
                "threshold_bp {} 越界，允许区间 [{MIN_THRESHOLD_BP}, {MAX_THRESHOLD_BP}]",
                facts.threshold_bp
            ),
        });
    }
    let threshold_tokens = threshold_tokens(facts)?;
    let mut blocks = Vec::new();
    let need = need_verdict(facts, threshold_tokens, &mut blocks);
    if need != Ternary::True {
        return Ok(decision(
            Tier::Keep,
            need,
            need,
            threshold_tokens,
            0,
            blocks,
        ));
    }
    let excess = facts
        .measured_tokens
        .unwrap_or_default()
        .saturating_sub(threshold_tokens);
    // 只有真能降到阈值以下才算「剪枝够用」；可剪枝量为 0 时不成立。
    let prune_relieves = facts.prunable_tokens > 0 && facts.prunable_tokens >= excess;
    if prune_relieves {
        match facts.recall_reachable {
            Ternary::True => {
                return Ok(decision(
                    Tier::Prune,
                    Ternary::True,
                    need,
                    threshold_tokens,
                    excess,
                    blocks,
                ));
            }
            // 未证实可回读时停在 Keep，不因为「高档看着更方便」而升级到有损摘要。
            Ternary::Unknown => {
                blocks.push(CompactionBlock::RecallUnreachable);
                return Ok(decision(
                    Tier::Keep,
                    Ternary::Unknown,
                    need,
                    threshold_tokens,
                    excess,
                    blocks,
                ));
            }
            // 已被否证：剪枝这条路走不通，才轮到摘要自己的门。
            Ternary::False => {}
        }
    }
    let mut summarize_gates = Vec::new();
    let admission = summarize_admission(facts, &mut summarize_gates);
    blocks.extend(summarize_gates);
    let tier = if admission == Ternary::True {
        Tier::Summarize
    } else {
        Tier::Keep
    };
    Ok(decision(
        tier,
        admission,
        need,
        threshold_tokens,
        excess,
        blocks,
    ))
}

fn summarize_admission(facts: &ContextFacts, blocks: &mut Vec<CompactionBlock>) -> Ternary {
    let mut admission = Ternary::True;
    for (verdict, block) in [
        (facts.durable_flushed, CompactionBlock::FactsNotFlushed),
        (
            facts.constraints_pinned,
            CompactionBlock::ConstraintsUnpinned,
        ),
        (
            facts.progress_since_compaction,
            CompactionBlock::NoDurableProgress,
        ),
    ] {
        if verdict != Ternary::True {
            blocks.push(block);
        }
        admission = admission & verdict;
    }
    admission
}

fn need_verdict(
    facts: &ContextFacts,
    threshold_tokens: u64,
    blocks: &mut Vec<CompactionBlock>,
) -> Ternary {
    let tokens = match facts.measured_tokens {
        None => {
            blocks.push(CompactionBlock::UnmeasuredPressure);
            return Ternary::Unknown;
        }
        Some(tokens) => tokens,
    };
    if tokens >= threshold_tokens {
        return Ternary::True;
    }
    if facts.overflow != Overflow::Reported {
        blocks.push(CompactionBlock::BelowThreshold);
        return Ternary::False;
    }
    if suspected_false_overflow(tokens, facts.context_window) {
        blocks.push(CompactionBlock::SuspectedFalseOverflow);
        return Ternary::False;
    }
    blocks.push(CompactionBlock::ReportDisagrees);
    Ternary::Unknown
}

/// 实测不足半窗却报溢出：按供应商侧误报处理（与 dsh-compact `engine.js:41` 同口径）。
fn suspected_false_overflow(tokens: u64, context_window: u64) -> bool {
    tokens.saturating_mul(2) < context_window
}

fn threshold_tokens(facts: &ContextFacts) -> Result<u64, Error> {
    facts
        .context_window
        .checked_mul(u64::from(facts.threshold_bp))
        .and_then(|scaled| scaled.checked_div(10_000))
        .ok_or_else(|| Error::ContextConfig {
            message: "context_window * threshold_bp 溢出 u64".into(),
        })
}

fn decision(
    tier: Tier,
    admission: Ternary,
    need: Ternary,
    threshold_tokens: u64,
    excess_tokens: u64,
    blocks: Vec<CompactionBlock>,
) -> CompactionDecision {
    let mut obligations: Vec<Obligation> = Vec::new();
    for block in &blocks {
        let obligation = block.obligation();
        if !obligations.contains(&obligation) {
            obligations.push(obligation);
        }
    }
    CompactionDecision {
        tier,
        admission,
        need,
        threshold_tokens,
        excess_tokens,
        blocks,
        obligations,
    }
}
