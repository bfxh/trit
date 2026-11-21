//! 轨迹级六条不变量：这是轨迹域的公开契约，不是内部工具。
//!
//! `invariants` 模块管的是**一次判定**内部的自洽；这里管的是**整条轨迹**作为审计证据的
//! 可信性。仓库文档没有把六条逐条成文（`check-invariants` 只覆盖判定层三条 + 结构三条），
//! 所以这里按 `trace.rs` 类型系统可表达的**最强形式**给出定义，每条标注出处；语义一旦
//! 发布就按 `TraceInvariant::as_str` 的稳定字符串锁死，改名等于破坏对外契约。
//!
//! ## 六条契约
//!
//! 1. **`seq_contiguous`（序号无缺口）**——事件序号从 1 开始连续。
//!    缺口 = 事件丢失或被删，折叠出的任何"当时状态"都建立在残缺证据上，且缺口本身看不出来。
//!    出处：`TraceLog::gaps`（"只报告内部与开头缺口，不猜末尾"）与 `TraceEventKind::TraceGap` 的存在。
//! 2. **`causation_resolvable`（因果引用不悬空）**——`causation_id` 指向的事件必须存在于
//!    同一日志，且不能指向自身。悬空引用让"为什么做这件事"的链条断在审计里，回放时无法
//!    区分"前置事件被删"与"引用写错"。出处：`TraceEvent::causation_id: Option<EventId>` 的引用契约。
//! 3. **`event_time_not_after_observation`（事实不晚于获知）**——`event_time` 已知时必须
//!    ≤ `observed_at`。事实不可能在编排获知它之后才发生；倒挂意味着时钟被污染或字段语义被顶替
//!    （拿 `observed_at` 冒充 `event_time` 就是典型倒挂源）。
//!    出处：trace.rs 模块文档——"`event_time` 是事实发生时间，`observed_at` 是编排获知时间，两者不互相替代"。
//! 4. **`recorded_not_before_observation`（落盘不早于获知）**——`recorded_at` ≥ `observed_at`。
//!    宿主写盘只能发生在获知之后，倒挂即审计时间戳不可信。出处：`recorded_at` 字段文档
//!    （"宿主写盘时间；仅供审计，不参与排序"）。
//! 5. **`hash_chain_linked`（防篡改链闭合）**——按 seq 序，首条 `prev_hash` 为空（创世事件没有
//!    前驱），其余每条 `prev_hash` 等于前一条的 `payload_hash`。断链 = 有事件被替换、插入或重排。
//!    出处：`TraceEvent::{prev_hash, payload_hash}` 字段——追加式审计链的标准结构；哈希算法本身
//!    尚未钉死（字段当前为空串），本条只验**链接完整性**，算法钉死后自动升级为逐字节防篡改。
//! 6. **`fold_revision_aligned`（折叠状态对齐）**——按 seq 折叠后，plan revision 与 snapshot
//!    revision 同时存在时必须相等；不等则"当时按哪个计划核对这份快照"不可回答，回放会 fail-closed。
//!    出处：`TraceLog::latest_step` 的 `RevisionMismatch` 语义。
//!
//! ## 违反后果
//!
//! 任何一条被违反，这条轨迹就不能再当作"可回放出当时决定"的证据：1/5 意味着证据被删改，
//! 2/6 意味着因果与状态对不上，3/4 意味着时间线自相矛盾。检查器**不信任 `TraceLog` 内部
//! 结构**，六条全部从 `events()` 重新推导；六条独立运行、独立报告，缺一条都要在
//! [`TraceInvariant::ALL`] 里见名知意。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::trace::{TraceEvent, TraceLog, TracePayload};

/// 轨迹级不变量名。取值同时是 CI 日志里的稳定字符串，改名等于破坏对外契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceInvariant {
    /// 序号从 1 开始连续，无内部与开头缺口。
    SeqContiguous,
    /// `causation_id` 必须指向本日志中已存在的另一条事件。
    CausationResolvable,
    /// `event_time` 已知时不晚于 `observed_at`。
    EventTimeNotAfterObservation,
    /// `recorded_at` 不早于 `observed_at`。
    RecordedNotBeforeObservation,
    /// 按 seq 序，`prev_hash` 逐条闭合到前一条的 `payload_hash`。
    HashChainLinked,
    /// 折叠后的 plan revision 与 snapshot revision 相等。
    FoldRevisionAligned,
}

impl TraceInvariant {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SeqContiguous => "seq_contiguous",
            Self::CausationResolvable => "causation_resolvable",
            Self::EventTimeNotAfterObservation => "event_time_not_after_observation",
            Self::RecordedNotBeforeObservation => "recorded_not_before_observation",
            Self::HashChainLinked => "hash_chain_linked",
            Self::FoldRevisionAligned => "fold_revision_aligned",
        }
    }

    /// 全部六条，按声明顺序。CI 可以据此断言"一条都没漏"。
    pub const ALL: [TraceInvariant; 6] = [
        Self::SeqContiguous,
        Self::CausationResolvable,
        Self::EventTimeNotAfterObservation,
        Self::RecordedNotBeforeObservation,
        Self::HashChainLinked,
        Self::FoldRevisionAligned,
    ];
}

/// 一条违例。带 seq 与期望/实际值，直接定位到事件，不给一句空话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceViolation {
    pub invariant: TraceInvariant,
    /// 违规事件的 seq；缺口报缺口起点，折叠对齐报最后一条快照的 seq。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub expected: String,
    pub actual: String,
}

impl TraceViolation {
    fn at(
        invariant: TraceInvariant,
        seq: Option<u64>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            invariant,
            seq,
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    /// 一行可读证书：`invariant seq=… expected=… actual=…`。
    pub fn brief(&self) -> String {
        let mut line = self.invariant.as_str().to_owned();
        if let Some(seq) = self.seq {
            line.push_str(" seq=");
            line.push_str(&seq.to_string());
        }
        line.push_str(" expected=");
        line.push_str(&self.expected);
        line.push_str(" actual=");
        line.push_str(&self.actual);
        line
    }
}

/// 一次检查的结果。空 = 干净。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceReport {
    pub violations: Vec<TraceViolation>,
}

impl TraceReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.violations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    /// 按不变量分组计数，供面板与 CI 摘要。
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for violation in &self.violations {
            *counts.entry(violation.invariant.as_str()).or_insert(0) += 1;
        }
        counts
    }

    /// CI 入口：干净则 `Ok(())`，否则把报告原样返回（保留全部证书，不只第一条）。
    pub fn assert_clean(self) -> Result<(), TraceReport> {
        if self.is_clean() { Ok(()) } else { Err(self) }
    }

    pub fn brief(&self) -> String {
        if self.is_clean() {
            return "trace invariants: clean".to_owned();
        }
        let counts = self
            .counts()
            .into_iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "trace invariants: {} violated [{counts}]",
            self.violations.len()
        )
    }
}

/// 检查一条轨迹日志：六条不变量全部独立运行、独立报告。
///
/// 检查器不信任 `TraceLog` 内部结构，全部从 `events()` 重新推导；`TraceLog` 自己的
/// 闸门（seq 冲突、schema 漂移、trace 归属）在追加时已经 fail-closed，这里补的是
/// 追加闸门管不到的轨迹级性质。
pub fn check(log: &TraceLog) -> TraceReport {
    let mut report = TraceReport::default();
    let events: Vec<&TraceEvent> = log.events().collect();

    // 1. 序号无缺口：`gaps` 只报内部与开头缺口，不猜末尾。
    for (start, end) in log.gaps() {
        report.violations.push(TraceViolation::at(
            TraceInvariant::SeqContiguous,
            Some(start),
            format!("seq {start}..={end} 存在"),
            "缺失",
        ));
    }

    // 2. 因果引用不悬空。
    let ids: BTreeSet<&str> = events.iter().map(|event| event.id.as_str()).collect();
    for event in &events {
        let Some(causation) = event.causation_id.as_deref() else {
            continue;
        };
        if !ids.contains(causation) {
            report.violations.push(TraceViolation::at(
                TraceInvariant::CausationResolvable,
                Some(event.seq),
                "causation_id 指向本日志中的事件",
                format!("悬空引用 {causation}"),
            ));
        } else if causation == event.id.as_str() {
            report.violations.push(TraceViolation::at(
                TraceInvariant::CausationResolvable,
                Some(event.seq),
                "causation_id 指向另一条事件",
                format!("自引用 {causation}"),
            ));
        }
    }

    // 3/4. 时间线因果：事实不晚于获知，落盘不早于获知。
    for event in &events {
        if let Some(at) = event.event_time
            && at > event.observed_at
        {
            report.violations.push(TraceViolation::at(
                TraceInvariant::EventTimeNotAfterObservation,
                Some(event.seq),
                format!("event_time <= observed_at={}", event.observed_at),
                format!("event_time={at}"),
            ));
        }
        if event.recorded_at < event.observed_at {
            report.violations.push(TraceViolation::at(
                TraceInvariant::RecordedNotBeforeObservation,
                Some(event.seq),
                format!("recorded_at >= observed_at={}", event.observed_at),
                format!("recorded_at={}", event.recorded_at),
            ));
        }
    }

    // 5. 防篡改链：首条 prev_hash 为空，其余逐条闭合。
    let mut previous: Option<&TraceEvent> = None;
    for event in &events {
        match previous {
            None => {
                if !event.prev_hash.is_empty() {
                    report.violations.push(TraceViolation::at(
                        TraceInvariant::HashChainLinked,
                        Some(event.seq),
                        "创世事件 prev_hash 为空",
                        format!("prev_hash={}", event.prev_hash),
                    ));
                }
            }
            Some(prior) => {
                if event.prev_hash != prior.payload_hash {
                    report.violations.push(TraceViolation::at(
                        TraceInvariant::HashChainLinked,
                        Some(event.seq),
                        format!("prev_hash == 前一条 payload_hash={}", prior.payload_hash),
                        format!("prev_hash={}", event.prev_hash),
                    ));
                }
            }
        }
        previous = Some(event);
    }

    // 6. 折叠对齐：手工重推，定位到最后一次 plan 与最后一次 snapshot。
    let mut plan: Option<(u64, u64)> = None;
    let mut snapshot: Option<(u64, u64)> = None;
    for event in &events {
        match event.payload.as_ref() {
            Some(TracePayload::Plan { revision, .. }) => plan = Some((*revision, event.seq)),
            Some(TracePayload::Snapshot { snapshot: state }) => {
                snapshot = Some((state.revision, event.seq))
            }
            _ => {}
        }
    }
    if let (Some((expected, _)), Some((actual, seq))) = (plan, snapshot)
        && expected != actual
    {
        report.violations.push(TraceViolation::at(
            TraceInvariant::FoldRevisionAligned,
            Some(seq),
            format!("snapshot revision == plan revision {expected}"),
            format!("snapshot revision {actual}"),
        ));
    }

    report
}
