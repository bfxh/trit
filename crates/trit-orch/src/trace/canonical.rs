//! 无克隆的规范化序列化：直接借用事件字段，避免为了算摘要复制整个 payload。

use serde::Serialize;

use super::{Actor, EventId, LogicalTime, SpanId, TraceEvent, TraceEventKind, TracePayload};

#[derive(Serialize)]
struct HashView<'a> {
    schema: u32,
    id: &'a str,
    trace_id: &'a str,
    span_id: &'a str,
    parent: &'a Option<SpanId>,
    seq: u64,
    kind: TraceEventKind,
    payload: &'a Option<TracePayload>,
    causation_id: &'a Option<EventId>,
    correlation_id: &'a str,
    logical: LogicalTime,
    event_time: &'a Option<u64>,
    observed_at: u64,
    recorded_at: u64,
    actor: &'a Actor,
    payload_hash: &'a str,
    prev_hash: &'a str,
    chain_seq: u64,
}

impl TraceEvent {
    /// 规范化 payload 字节：`payload_hash` / `prev_hash` 自身不参与计算。
    ///
    /// 与 `serde_json::to_vec(&self-with-cleared-hashes)` 逐字节一致，但不再克隆
    /// 事件（大 payload 时省掉一次深拷贝）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let empty = "";
        let view = HashView {
            schema: self.schema,
            id: &self.id,
            trace_id: &self.trace_id,
            span_id: &self.span_id,
            parent: &self.parent,
            seq: self.seq,
            kind: self.kind,
            payload: &self.payload,
            causation_id: &self.causation_id,
            correlation_id: &self.correlation_id,
            logical: self.logical,
            event_time: &self.event_time,
            observed_at: self.observed_at,
            recorded_at: self.recorded_at,
            actor: &self.actor,
            payload_hash: empty,
            prev_hash: empty,
            chain_seq: self.chain_seq,
        };
        serde_json::to_vec(&view).expect("trace event is always serializable")
    }
}
