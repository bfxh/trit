//! 轨迹事件与单 trace 事件日志。
//!
//! 事件按 `seq` 排序；`event_time` 是事实发生时间，`observed_at` 是编排获知时间，
//! 两者不互相替代。`TraceLog` 只做单 trace 的追加、去重、冲突与缺口判定。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    AdmissionObligation, Attempt, Claim, CompactionDecision, DeliveryReceipt, Effect, Error,
    EvidenceVerdict, HandoffPacket, Ledger, Limits, Plan, Reconcile, Selection, Snapshot, TaskSpec,
    Ternary, TraceStep,
};

pub type TraceId = String;
pub type SpanId = String;
pub type EventId = String;

/// 判定推进的逻辑时间；不能换算成秒。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogicalTime {
    pub revision: u64,
    pub epoch: u64,
    pub fence: u64,
    pub round: u64,
    pub attempt: u64,
}

/// 混合逻辑时钟：`(物理毫秒, 计数, 写者)`，字典序即全序。
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Hlc {
    pub physical_ms: u64,
    pub counter: u32,
    pub writer: u32,
}

impl Hlc {
    pub const ZERO: Self = Self {
        physical_ms: 0,
        counter: 0,
        writer: 0,
    };

    pub const fn new(physical_ms: u64, counter: u32, writer: u32) -> Self {
        Self {
            physical_ms,
            counter,
            writer,
        }
    }
}

/// 事件的行为主体。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Actor {
    Agent(String),
    Human(String),
    Host,
}

/// 认领事件的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimAction {
    Acquired,
    Renewed,
    Released,
    Expired,
}

/// 通讯回执事件的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptAction {
    Recorded,
    Advanced,
    Rejected,
    Expired,
}

/// 义务事件的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationOutcome {
    Scheduled,
    Skipped,
    Fulfilled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub action: ClaimAction,
    pub claim: Claim,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRecord {
    pub action: ReceiptAction,
    pub receipt: DeliveryReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    pub packet: HandoffPacket,
    pub receiver_state_hash: String,
    pub gate: Ternary,
    pub obligation: Option<AdmissionObligation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationRecord {
    pub outcome: ObligationOutcome,
    pub obligation: AdmissionObligation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectRecord {
    pub attempt: Attempt,
    pub state: Effect,
}

/// 轨迹事件类型。当前只定义稳定分类，不绑定具体 payload。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKind {
    PlanPatched,
    SnapshotCaptured,
    LimitsCaptured,
    LedgerCaptured,
    JudgmentDerived,
    TaskSelected,
    TaskHeld,
    TaskDenied,
    SelectionCaptured,
    EffectPrepared,
    EffectSending,
    EffectSent,
    EffectConfirmed,
    EffectRefuted,
    EffectUnknown,
    EffectReconciled,
    ClaimAcquired,
    ClaimRenewed,
    ClaimReleased,
    ClaimExpired,
    ReceiptRecorded,
    ReceiptAdvanced,
    ReceiptRejected,
    ReceiptExpired,
    HandoffGated,
    ObligationScheduled,
    ObligationFulfilled,
    ObligationSkipped,
    CompactionDecided,
    FinishRefused,
    FinishConfirmed,
    TraceGap,
}

impl TraceEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanPatched => "plan_patched",
            Self::SnapshotCaptured => "snapshot_captured",
            Self::LimitsCaptured => "limits_captured",
            Self::LedgerCaptured => "ledger_captured",
            Self::JudgmentDerived => "judgment_derived",
            Self::TaskSelected => "task_selected",
            Self::TaskHeld => "task_held",
            Self::TaskDenied => "task_denied",
            Self::SelectionCaptured => "selection_captured",
            Self::EffectPrepared => "effect_prepared",
            Self::EffectSending => "effect_sending",
            Self::EffectSent => "effect_sent",
            Self::EffectConfirmed => "effect_confirmed",
            Self::EffectRefuted => "effect_refuted",
            Self::EffectUnknown => "effect_unknown",
            Self::EffectReconciled => "effect_reconciled",
            Self::ClaimAcquired => "claim_acquired",
            Self::ClaimRenewed => "claim_renewed",
            Self::ClaimReleased => "claim_released",
            Self::ClaimExpired => "claim_expired",
            Self::ReceiptRecorded => "receipt_recorded",
            Self::ReceiptAdvanced => "receipt_advanced",
            Self::ReceiptRejected => "receipt_rejected",
            Self::ReceiptExpired => "receipt_expired",
            Self::HandoffGated => "handoff_gated",
            Self::ObligationScheduled => "obligation_scheduled",
            Self::ObligationFulfilled => "obligation_fulfilled",
            Self::ObligationSkipped => "obligation_skipped",
            Self::CompactionDecided => "compaction_decided",
            Self::FinishRefused => "finish_refused",
            Self::FinishConfirmed => "finish_confirmed",
            Self::TraceGap => "trace_gap",
        }
    }
}

/// 事件携带的完整状态载荷；折叠时“后到的完整状态覆盖先到的”。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TracePayload {
    Plan {
        revision: u64,
        tasks: Vec<TaskSpec>,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Limits {
        limits: Limits,
    },
    Ledger {
        ledger: Ledger,
    },
    Selection {
        revision: u64,
        selected: Vec<String>,
    },
    Claim {
        action: ClaimAction,
        claim: Claim,
    },
    Receipt {
        action: ReceiptAction,
        receipt: DeliveryReceipt,
    },
    Handoff {
        record: Box<HandoffRecord>,
    },
    Compaction {
        decision: CompactionDecision,
    },
    Obligation {
        outcome: ObligationOutcome,
        obligation: AdmissionObligation,
    },
    Effect {
        attempt: Attempt,
        state: Effect,
    },
    EffectReconciled {
        attempt: Attempt,
        observed: Reconcile,
        from: Effect,
        to: Effect,
    },
    Judgment {
        subject: String,
        verdict: EvidenceVerdict,
        obligations: Vec<AdmissionObligation>,
    },
    Finish {
        counts: FinishCounts,
    },
}

/// 完成事件的计数快照；`FinishConfirmed` 时三个未定/冲突/在途计数必须为 0。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishCounts {
    pub confirmed: usize,
    pub refuted: usize,
    pub undetermined: usize,
    pub conflicts: usize,
    pub in_doubt: usize,
}

/// 脱敏策略：按键名和值前缀识别 secret；不匹配的内容原样保留。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RedactionPolicy {
    pub sensitive_keys: Vec<String>,
    pub sensitive_prefixes: Vec<String>,
    pub replacement: String,
}

impl Default for RedactionPolicy {
    fn default() -> Self {
        Self {
            sensitive_keys: [
                "secret",
                "token",
                "password",
                "authorization",
                "api_key",
                "apikey",
                "credential",
            ]
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
            sensitive_prefixes: [
                "sk-",
                "ghp_",
                "gho_",
                "github_pat_",
                "AKIA",
                "Bearer ",
                "-----BEGIN",
            ]
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
            replacement: "[REDACTED]".into(),
        }
    }
}

/// 一条追加式轨迹事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub schema: u32,
    pub id: EventId,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent: Option<SpanId>,
    pub seq: u64,
    pub kind: TraceEventKind,
    #[serde(default)]
    pub payload: Option<TracePayload>,
    pub causation_id: Option<EventId>,
    pub correlation_id: String,
    pub logical: LogicalTime,
    /// 跨主机破平用的 HLC；单主机轨迹可以没有。
    #[serde(default)]
    pub hlc: Option<Hlc>,
    /// 事实发生时间；未知就是 `None`，不能用 `observed_at` 顶替。
    pub event_time: Option<u64>,
    /// 编排观察到这条事实的时间。
    pub observed_at: u64,
    /// 宿主写盘时间；仅供审计，不参与排序。
    pub recorded_at: u64,
    pub actor: Actor,
    pub payload_hash: String,
    pub prev_hash: String,
}

impl TraceEvent {
    pub const CURRENT_SCHEMA: u32 = 1;

    pub fn new(
        trace_id: impl Into<String>,
        seq: u64,
        kind: TraceEventKind,
        correlation_id: impl Into<String>,
        observed_at: u64,
    ) -> Self {
        let trace_id = trace_id.into();
        Self {
            schema: Self::CURRENT_SCHEMA,
            id: format!("{trace_id}:{seq}"),
            trace_id,
            span_id: format!("span-{seq}"),
            parent: None,
            seq,
            kind,
            payload: None,
            causation_id: None,
            correlation_id: correlation_id.into(),
            logical: LogicalTime::default(),
            hlc: None,
            event_time: None,
            observed_at,
            recorded_at: observed_at,
            actor: Actor::Host,
            payload_hash: String::new(),
            prev_hash: String::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_payload(mut self, payload: TracePayload) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_event_time(mut self, event_time: u64) -> Self {
        self.event_time = Some(event_time);
        self
    }

    pub fn with_causation_id(mut self, id: impl Into<String>) -> Self {
        self.causation_id = Some(id.into());
        self
    }

    pub fn with_logical(mut self, logical: LogicalTime) -> Self {
        self.logical = logical;
        self
    }

    pub fn with_hlc(mut self, hlc: Hlc) -> Self {
        self.hlc = Some(hlc);
        self
    }

    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    /// 丢掉完整载荷，但保留事件骨架与对原载荷的哈希承诺。
    pub fn redacted(&self) -> Self {
        let mut event = self.clone();
        if let Some(payload) = event.payload.take() {
            event.payload_hash = payload_digest(&payload);
        }
        event
    }

    /// 按策略脱敏载荷内容；结构不再可逆时失败关闭，直接丢载荷。
    pub fn redacted_with(&self, policy: &RedactionPolicy) -> Self {
        let Some(payload) = self.payload.as_ref() else {
            return self.clone();
        };
        let payload_hash = payload_digest(payload);
        let mut value = match serde_json::to_value(payload) {
            Ok(value) => value,
            Err(_) => return self.redacted(),
        };
        if !redact_value(&mut value, policy) {
            let mut event = self.clone();
            event.payload_hash = payload_hash;
            return event;
        }
        match serde_json::from_value::<TracePayload>(value) {
            Ok(redacted) => {
                let mut event = self.clone();
                event.payload = Some(redacted);
                event.payload_hash = payload_hash;
                event
            }
            Err(_) => {
                let mut event = self.clone();
                event.payload = None;
                event.payload_hash = payload_hash;
                event
            }
        }
    }
}

/// 事件流折叠出的状态；`None` 表示该类完整状态事件尚未出现。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceFold {
    pub revision: Option<u64>,
    pub tasks: Option<Vec<TaskSpec>>,
    pub snapshot: Option<Snapshot>,
    pub limits: Option<Limits>,
    pub ledger: Option<Ledger>,
}

impl TraceFold {
    fn apply(&mut self, event: &TraceEvent) {
        match event.payload.as_ref() {
            Some(TracePayload::Plan { revision, tasks }) => {
                self.revision = Some(*revision);
                self.tasks = Some(tasks.clone());
            }
            Some(TracePayload::Snapshot { snapshot }) => {
                self.snapshot = Some(snapshot.clone());
            }
            Some(TracePayload::Limits { limits }) => {
                self.limits = Some(limits.clone());
            }
            Some(TracePayload::Ledger { ledger }) => {
                self.ledger = Some(ledger.clone());
            }
            _ => {}
        }
    }
}

/// 协调面与效果面的审计折叠；只收集已记录的事实，不重建内部锁表。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationFold {
    pub claims: Vec<ClaimRecord>,
    pub receipts: Vec<ReceiptRecord>,
    pub handoffs: Vec<HandoffRecord>,
    pub compactions: Vec<CompactionDecision>,
    pub obligations: Vec<ObligationRecord>,
    pub effects: Vec<EffectRecord>,
}

impl CoordinationFold {
    fn apply(&mut self, event: &TraceEvent) {
        match event.payload.as_ref() {
            Some(TracePayload::Claim { action, claim }) => self.claims.push(ClaimRecord {
                action: *action,
                claim: claim.clone(),
            }),
            Some(TracePayload::Receipt { action, receipt }) => {
                self.receipts.push(ReceiptRecord {
                    action: *action,
                    receipt: receipt.clone(),
                });
            }
            Some(TracePayload::Handoff { record }) => self.handoffs.push(record.as_ref().clone()),
            Some(TracePayload::Compaction { decision }) => self.compactions.push(decision.clone()),
            Some(TracePayload::Obligation {
                outcome,
                obligation,
            }) => {
                self.obligations.push(ObligationRecord {
                    outcome: *outcome,
                    obligation: obligation.clone(),
                });
            }
            Some(TracePayload::Effect { attempt, state }) => self.effects.push(EffectRecord {
                attempt: attempt.clone(),
                state: *state,
            }),
            _ => {}
        }
    }
}

/// 一条因果链：根 -> 目标；缺失的 causation 引用单独列出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalChain {
    pub target: EventId,
    pub events: Vec<TraceEvent>,
    pub missing: Vec<EventId>,
}

/// 选择回放的差异报告。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub steps: usize,
    pub agreed: usize,
    pub divergences: Vec<ReplayDivergence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDivergence {
    pub seq: u64,
    pub kind: ReplayDivergenceKind,
    pub recorded: Vec<String>,
    pub replayed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayDivergenceKind {
    PolicyOnly,
    KernelChanged,
    MissingEvent,
    ClockSkew,
    Nondeterminism,
    SchemaDrift,
    DataLoss,
}

/// 轨迹指标。事件时间、观测时间、逻辑时间三个口径分开，不互相顶替。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceMetrics {
    pub events: usize,
    /// 没有 `event_time` 的事件数；不能用 `observed_at` 冒充。
    pub event_time_missing: usize,
    /// `observed_at` 的最小/最大跨度。
    pub observation_span: Option<u64>,
    /// `recorded_at` 晚于 `observed_at` 的最大值（写入滞后）。
    pub trace_lag_max: Option<u64>,
    /// `recorded_at < observed_at` 的事件数（时钟回拨/宿主抖动）。
    pub clock_skew_events: usize,
    /// 效果 Prepared -> Confirmed 的最大**事件时间**延迟。
    pub effect_latency_event_time: Option<u64>,
    /// 同一段效果延迟的**观测时间**版本。
    pub effect_latency_observed: Option<u64>,
    /// 回执 Recorded -> Acted 的最大观测延迟。
    pub handoff_latency_observed: Option<u64>,
    /// 仍处于 Sending/Sent/Unknown 的效果数。
    pub in_doubt: usize,
    /// 在途效果距离 `now` 的最大年龄。
    pub in_doubt_age_max: Option<u64>,
    /// `SelectionCaptured` 事件数。
    pub selections: usize,
    pub logical_revision_max: Option<u64>,
    pub logical_epoch_max: Option<u64>,
    pub logical_fence_max: Option<u64>,
    pub logical_round_max: Option<u64>,
}

/// 一次追加的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppendOutcome {
    Recorded,
    /// 事件合法，但 seq 小于当前最大 seq；允许写入，标记迟到。
    LateButRecorded,
    /// 同 event id 已存在，幂等忽略。
    Duplicate,
}

/// 单 trace 的事件日志。跨 trace 的事件必须由另一个 `TraceLog` 接收。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceLog {
    trace_id: TraceId,
    events: BTreeMap<u64, TraceEvent>,
    ids: BTreeSet<EventId>,
}

impl TraceLog {
    pub fn new(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            events: BTreeMap::new(),
            ids: BTreeSet::new(),
        }
    }

    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    pub fn append(&mut self, event: TraceEvent) -> Result<AppendOutcome, Error> {
        if event.schema != TraceEvent::CURRENT_SCHEMA {
            return Err(Error::TraceSchema {
                found: event.schema,
                expected: TraceEvent::CURRENT_SCHEMA,
            });
        }
        if event.trace_id != self.trace_id {
            return Err(Error::TraceMismatch {
                expected: self.trace_id.clone(),
                found: event.trace_id,
            });
        }
        if self.ids.contains(&event.id) {
            return Ok(AppendOutcome::Duplicate);
        }
        if self.events.contains_key(&event.seq) {
            return Err(Error::TraceSeqConflict { seq: event.seq });
        }

        let late = self
            .events
            .last_key_value()
            .is_some_and(|(&max_seq, _)| event.seq < max_seq);
        self.ids.insert(event.id.clone());
        self.events.insert(event.seq, event);
        Ok(if late {
            AppendOutcome::LateButRecorded
        } else {
            AppendOutcome::Recorded
        })
    }

    /// 按 `seq` 升序迭代；乱序追加不会改变这个顺序。
    pub fn events(&self) -> impl Iterator<Item = &TraceEvent> {
        self.events.values()
    }

    /// 缺失的 seq 闭区间。只报告内部与开头缺口，不猜末尾。
    pub fn gaps(&self) -> Vec<(u64, u64)> {
        let mut gaps = Vec::new();
        let mut expected = 1;
        for &seq in self.events.keys() {
            if seq > expected {
                gaps.push((expected, seq - 1));
            }
            expected = seq.saturating_add(1);
        }
        gaps
    }

    /// 按 seq 折叠完整状态事件；后到的完整状态覆盖先到的。
    pub fn fold(&self) -> TraceFold {
        let mut fold = TraceFold::default();
        for event in self.events.values() {
            fold.apply(event);
        }
        fold
    }

    /// 按 seq 收集协调面与效果面的审计记录。
    pub fn coordination(&self) -> CoordinationFold {
        let mut fold = CoordinationFold::default();
        for event in self.events.values() {
            fold.apply(event);
        }
        fold
    }

    /// 按三套时钟分别计算指标；不同口径不互相替代。
    pub fn metrics(&self, now: u64) -> TraceMetrics {
        #[derive(Default)]
        struct EffectWindow {
            prepared_event_time: Option<u64>,
            prepared_observed: Option<u64>,
            confirmed_event_time: Option<u64>,
            confirmed_observed: Option<u64>,
        }

        #[derive(Default)]
        struct ReceiptWindow {
            recorded_observed: Option<u64>,
            acted_observed: Option<u64>,
        }

        let mut metrics = TraceMetrics::default();
        let mut min_observed: Option<u64> = None;
        let mut max_observed: Option<u64> = None;
        let mut effect_windows: BTreeMap<(String, u64), EffectWindow> = BTreeMap::new();
        let mut effect_last: BTreeMap<String, (Effect, u64)> = BTreeMap::new();
        let mut receipt_windows: BTreeMap<String, ReceiptWindow> = BTreeMap::new();

        for event in self.events.values() {
            metrics.events += 1;
            if event.event_time.is_none() {
                metrics.event_time_missing += 1;
            }
            min_observed = Some(
                min_observed.map_or(event.observed_at, |value: u64| value.min(event.observed_at)),
            );
            max_observed = Some(
                max_observed.map_or(event.observed_at, |value: u64| value.max(event.observed_at)),
            );
            if event.recorded_at >= event.observed_at {
                let lag = event.recorded_at - event.observed_at;
                metrics.trace_lag_max = Some(
                    metrics
                        .trace_lag_max
                        .map_or(lag, |value: u64| value.max(lag)),
                );
            } else {
                metrics.clock_skew_events += 1;
            }
            if event.logical.revision > 0 {
                metrics.logical_revision_max =
                    max_option(metrics.logical_revision_max, event.logical.revision);
            }
            if event.logical.epoch > 0 {
                metrics.logical_epoch_max =
                    max_option(metrics.logical_epoch_max, event.logical.epoch);
            }
            if event.logical.fence > 0 {
                metrics.logical_fence_max =
                    max_option(metrics.logical_fence_max, event.logical.fence);
            }
            if event.logical.round > 0 {
                metrics.logical_round_max =
                    max_option(metrics.logical_round_max, event.logical.round);
            }

            match event.payload.as_ref() {
                Some(TracePayload::Effect { attempt, state }) => {
                    let window = effect_windows
                        .entry((attempt.operation.clone(), attempt.attempt))
                        .or_default();
                    match state {
                        Effect::Prepared => {
                            window.prepared_event_time =
                                window.prepared_event_time.or(event.event_time);
                            window.prepared_observed =
                                window.prepared_observed.or(Some(event.observed_at));
                        }
                        Effect::Confirmed => {
                            window.confirmed_event_time =
                                window.confirmed_event_time.or(event.event_time);
                            window.confirmed_observed =
                                window.confirmed_observed.or(Some(event.observed_at));
                        }
                        _ => {}
                    }
                    effect_last.insert(attempt.operation.clone(), (*state, event.observed_at));
                }
                Some(TracePayload::Receipt { action, receipt }) => {
                    let window = receipt_windows
                        .entry(receipt.idempotency_key.clone())
                        .or_default();
                    if *action == ReceiptAction::Recorded {
                        window.recorded_observed =
                            window.recorded_observed.or(Some(event.observed_at));
                    }
                    if *action == ReceiptAction::Advanced && receipt.is_acted() {
                        window.acted_observed = window.acted_observed.or(Some(event.observed_at));
                    }
                }
                Some(TracePayload::Selection { .. }) => metrics.selections += 1,
                _ => {}
            }
        }

        if let (Some(min), Some(max)) = (min_observed, max_observed) {
            metrics.observation_span = Some(max.saturating_sub(min));
        }
        for window in effect_windows.values() {
            if let (Some(start), Some(end)) =
                (window.prepared_event_time, window.confirmed_event_time)
                && end >= start
            {
                let latency = end - start;
                metrics.effect_latency_event_time = Some(
                    metrics
                        .effect_latency_event_time
                        .map_or(latency, |value: u64| value.max(latency)),
                );
            }
            if let (Some(start), Some(end)) = (window.prepared_observed, window.confirmed_observed)
                && end >= start
            {
                let latency = end - start;
                metrics.effect_latency_observed = Some(
                    metrics
                        .effect_latency_observed
                        .map_or(latency, |value: u64| value.max(latency)),
                );
            }
        }
        for window in receipt_windows.values() {
            if let (Some(start), Some(end)) = (window.recorded_observed, window.acted_observed)
                && end >= start
            {
                let latency = end - start;
                metrics.handoff_latency_observed = Some(
                    metrics
                        .handoff_latency_observed
                        .map_or(latency, |value: u64| value.max(latency)),
                );
            }
        }
        for (state, observed_at) in effect_last.values() {
            if matches!(state, Effect::Sending | Effect::Sent | Effect::Unknown) {
                metrics.in_doubt += 1;
                let age = now.saturating_sub(*observed_at);
                metrics.in_doubt_age_max = Some(
                    metrics
                        .in_doubt_age_max
                        .map_or(age, |value: u64| value.max(age)),
                );
            }
        }
        metrics
    }

    /// 折叠到指定 seq（含）为止的状态；`seq` 之后的事件不参与。
    pub fn state_at(&self, seq: u64) -> TraceFold {
        let mut fold = TraceFold::default();
        for (&event_seq, event) in &self.events {
            if event_seq > seq {
                break;
            }
            fold.apply(event);
        }
        fold
    }

    /// 折叠出指定 seq 时点的 `TraceStep`。
    pub fn step_at(&self, seq: u64) -> Result<TraceStep, Error> {
        self.step_from_fold(self.state_at(seq))
    }

    /// 折叠出最近一次选批回放所需的 `TraceStep`。
    ///
    /// `plan/snapshot/limits` 缺失时失败关闭；缺失 ledger 视为空台账。
    pub fn latest_step(&self) -> Result<TraceStep, Error> {
        self.step_from_fold(self.fold())
    }

    fn step_from_fold(&self, fold: TraceFold) -> Result<TraceStep, Error> {
        let revision = fold.revision.ok_or_else(|| Error::TraceIncomplete {
            missing: "plan".into(),
        })?;
        let tasks = fold.tasks.ok_or_else(|| Error::TraceIncomplete {
            missing: "plan tasks".into(),
        })?;
        let snapshot = fold.snapshot.ok_or_else(|| Error::TraceIncomplete {
            missing: "snapshot".into(),
        })?;
        let limits = fold.limits.ok_or_else(|| Error::TraceIncomplete {
            missing: "limits".into(),
        })?;
        if snapshot.revision != revision {
            return Err(Error::RevisionMismatch {
                expected: revision,
                actual: snapshot.revision,
            });
        }
        Ok(TraceStep {
            revision,
            tasks,
            snapshot,
            limits,
            ledger: fold.ledger.unwrap_or_default(),
        })
    }

    /// 沿 `causation_id` 回溯到根；缺失引用放进 `missing`，成环判红。
    pub fn why(&self, event_id: &str) -> Result<CausalChain, Error> {
        let target = self
            .events
            .values()
            .find(|event| event.id == event_id)
            .ok_or_else(|| Error::TraceEventMissing {
                id: event_id.to_owned(),
            })?;
        let mut events = Vec::new();
        let mut missing = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = target;
        loop {
            if !seen.insert(current.id.clone()) {
                return Err(Error::TraceCausalityCycle {
                    id: current.id.clone(),
                });
            }
            events.push(current.clone());
            let Some(cause) = current.causation_id.as_deref() else {
                break;
            };
            match self.events.values().find(|event| event.id == cause) {
                Some(event) => current = event,
                None => {
                    missing.push(cause.to_owned());
                    break;
                }
            }
        }
        events.reverse();
        Ok(CausalChain {
            target: event_id.to_owned(),
            events,
            missing,
        })
    }

    /// 重放所有 `SelectionCaptured` 事件，对比记录与重算结果。
    pub fn replay_selections(&self) -> Result<ReplayReport, Error> {
        let mut report = ReplayReport::default();
        for event in self.events.values() {
            let Some(TracePayload::Selection { revision, selected }) = event.payload.as_ref()
            else {
                continue;
            };
            report.steps += 1;
            let step = match self.step_at(event.seq) {
                Ok(step) => step,
                Err(error) => {
                    report.divergences.push(ReplayDivergence {
                        seq: event.seq,
                        kind: replay_error_kind(&error),
                        recorded: selected.clone(),
                        replayed: Vec::new(),
                    });
                    continue;
                }
            };
            if step.revision != *revision {
                report.divergences.push(ReplayDivergence {
                    seq: event.seq,
                    kind: ReplayDivergenceKind::DataLoss,
                    recorded: selected.clone(),
                    replayed: Vec::new(),
                });
                continue;
            }
            let replayed = match Plan::new(step.revision, step.tasks.clone()).and_then(|plan| {
                plan.select_with_effects(&step.snapshot, &step.limits, &step.ledger)
            }) {
                Ok(selection) => selection.selected,
                Err(error) => {
                    report.divergences.push(ReplayDivergence {
                        seq: event.seq,
                        kind: replay_error_kind(&error),
                        recorded: selected.clone(),
                        replayed: Vec::new(),
                    });
                    continue;
                }
            };
            if &replayed == selected {
                report.agreed += 1;
            } else {
                report.divergences.push(ReplayDivergence {
                    seq: event.seq,
                    kind: ReplayDivergenceKind::KernelChanged,
                    recorded: selected.clone(),
                    replayed,
                });
            }
        }
        Ok(report)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

fn payload_digest(payload: &TracePayload) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn redact_value(value: &mut serde_json::Value, policy: &RedactionPolicy) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for (key, value) in map.iter_mut() {
                let key_lower = key.to_ascii_lowercase();
                if policy
                    .sensitive_keys
                    .iter()
                    .any(|sensitive| key_lower.contains(&sensitive.to_ascii_lowercase()))
                {
                    *value = serde_json::Value::String(policy.replacement.clone());
                    changed = true;
                } else {
                    changed |= redact_value(value, policy);
                }
            }
            changed
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= redact_value(item, policy);
            }
            changed
        }
        serde_json::Value::String(text)
            if policy
                .sensitive_prefixes
                .iter()
                .any(|prefix| text.contains(prefix)) =>
        {
            *text = policy.replacement.clone();
            true
        }
        _ => false,
    }
}

fn max_option(current: Option<u64>, value: u64) -> Option<u64> {
    Some(current.map_or(value, |existing| existing.max(value)))
}

fn replay_error_kind(error: &Error) -> ReplayDivergenceKind {
    match error {
        Error::RevisionMismatch { .. } => ReplayDivergenceKind::DataLoss,
        _ => ReplayDivergenceKind::MissingEvent,
    }
}

/// 把协调面操作转换成带 seq 的轨迹事件；是否落盘由宿主决定。
#[derive(Debug, Clone)]
pub struct TraceRecorder {
    trace_id: TraceId,
    next_seq: u64,
    actor: Actor,
}

impl TraceRecorder {
    pub fn new(trace_id: impl Into<String>, start_seq: u64) -> Self {
        Self {
            trace_id: trace_id.into(),
            next_seq: start_seq,
            actor: Actor::Host,
        }
    }

    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn claim(&mut self, action: ClaimAction, claim: Claim, observed_at: u64) -> TraceEvent {
        let kind = match action {
            ClaimAction::Acquired => TraceEventKind::ClaimAcquired,
            ClaimAction::Renewed => TraceEventKind::ClaimRenewed,
            ClaimAction::Released => TraceEventKind::ClaimReleased,
            ClaimAction::Expired => TraceEventKind::ClaimExpired,
        };
        self.emit(
            kind,
            claim.id.clone(),
            TracePayload::Claim { action, claim },
            observed_at,
        )
    }

    pub fn receipt(
        &mut self,
        action: ReceiptAction,
        receipt: DeliveryReceipt,
        observed_at: u64,
    ) -> TraceEvent {
        let kind = match action {
            ReceiptAction::Recorded => TraceEventKind::ReceiptRecorded,
            ReceiptAction::Advanced => TraceEventKind::ReceiptAdvanced,
            ReceiptAction::Rejected => TraceEventKind::ReceiptRejected,
            ReceiptAction::Expired => TraceEventKind::ReceiptExpired,
        };
        self.emit(
            kind,
            receipt.idempotency_key.clone(),
            TracePayload::Receipt { action, receipt },
            observed_at,
        )
    }

    pub fn effect_reconciled(
        &mut self,
        attempt: Attempt,
        observed: Reconcile,
        from: Effect,
        to: Effect,
        observed_at: u64,
    ) -> TraceEvent {
        self.emit(
            TraceEventKind::EffectReconciled,
            attempt.operation.clone(),
            TracePayload::EffectReconciled {
                attempt,
                observed,
                from,
                to,
            },
            observed_at,
        )
    }

    pub fn judgment(
        &mut self,
        subject: &str,
        verdict: EvidenceVerdict,
        obligations: Vec<AdmissionObligation>,
        observed_at: u64,
    ) -> TraceEvent {
        self.emit(
            TraceEventKind::JudgmentDerived,
            subject.to_owned(),
            TracePayload::Judgment {
                subject: subject.to_owned(),
                verdict,
                obligations,
            },
            observed_at,
        )
    }

    pub fn finish(
        &mut self,
        confirmed: bool,
        counts: FinishCounts,
        observed_at: u64,
    ) -> TraceEvent {
        let kind = if confirmed {
            TraceEventKind::FinishConfirmed
        } else {
            TraceEventKind::FinishRefused
        };
        self.emit(
            kind,
            "finish".into(),
            TracePayload::Finish { counts },
            observed_at,
        )
    }

    pub fn selection(&mut self, selection: &Selection, observed_at: u64) -> TraceEvent {
        self.emit(
            TraceEventKind::SelectionCaptured,
            format!("selection:{}", selection.revision),
            TracePayload::Selection {
                revision: selection.revision,
                selected: selection.selected.clone(),
            },
            observed_at,
        )
    }

    pub fn handoff(&mut self, record: HandoffRecord, observed_at: u64) -> TraceEvent {
        let correlation = record.packet.id.clone();
        self.emit(
            TraceEventKind::HandoffGated,
            correlation,
            TracePayload::Handoff {
                record: Box::new(record),
            },
            observed_at,
        )
    }

    pub fn compaction(&mut self, decision: CompactionDecision, observed_at: u64) -> TraceEvent {
        self.emit(
            TraceEventKind::CompactionDecided,
            "compaction".into(),
            TracePayload::Compaction { decision },
            observed_at,
        )
    }

    pub fn effect(&mut self, attempt: Attempt, state: Effect, observed_at: u64) -> TraceEvent {
        let kind = match state {
            Effect::Prepared => TraceEventKind::EffectPrepared,
            Effect::Sending => TraceEventKind::EffectSending,
            Effect::Sent => TraceEventKind::EffectSent,
            Effect::Confirmed => TraceEventKind::EffectConfirmed,
            Effect::Refuted => TraceEventKind::EffectRefuted,
            Effect::Unknown => TraceEventKind::EffectUnknown,
        };
        self.emit(
            kind,
            attempt.operation.clone(),
            TracePayload::Effect { attempt, state },
            observed_at,
        )
    }

    pub fn obligation(
        &mut self,
        outcome: ObligationOutcome,
        obligation: AdmissionObligation,
        observed_at: u64,
    ) -> TraceEvent {
        let kind = match outcome {
            ObligationOutcome::Scheduled => TraceEventKind::ObligationScheduled,
            ObligationOutcome::Skipped => TraceEventKind::ObligationSkipped,
            ObligationOutcome::Fulfilled => TraceEventKind::ObligationFulfilled,
        };
        self.emit(
            kind,
            obligation.id.clone(),
            TracePayload::Obligation {
                outcome,
                obligation,
            },
            observed_at,
        )
    }

    fn emit(
        &mut self,
        kind: TraceEventKind,
        correlation_id: String,
        payload: TracePayload,
        observed_at: u64,
    ) -> TraceEvent {
        let event = TraceEvent::new(
            self.trace_id.as_str(),
            self.next_seq,
            kind,
            correlation_id,
            observed_at,
        )
        .with_payload(payload)
        .with_actor(self.actor.clone());
        self.next_seq = self.next_seq.saturating_add(1);
        event
    }
}
