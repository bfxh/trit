//! 轨迹事件与单 trace 事件日志。
//!
//! 事件按 `seq` 排序；`event_time` 是事实发生时间，`observed_at` 是编排获知时间，
//! 两者不互相替代。`TraceLog` 只做单 trace 的追加、去重、冲突与缺口判定。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Error, Ledger, Limits, Snapshot, TaskSpec, TraceStep};

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

/// 事件的行为主体。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Actor {
    Agent(String),
    Human(String),
    Host,
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
    ReceiptAdvanced,
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
            Self::ReceiptAdvanced => "receipt_advanced",
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
    Plan { revision: u64, tasks: Vec<TaskSpec> },
    Snapshot { snapshot: Snapshot },
    Limits { limits: Limits },
    Ledger { ledger: Ledger },
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

    pub fn with_logical(mut self, logical: LogicalTime) -> Self {
        self.logical = logical;
        self
    }

    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
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
            None => {}
        }
    }
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

    /// 折叠出最近一次选批回放所需的 `TraceStep`。
    ///
    /// `plan/snapshot/limits` 缺失时失败关闭；缺失 ledger 视为空台账。
    pub fn latest_step(&self) -> Result<TraceStep, Error> {
        let fold = self.fold();
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

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
