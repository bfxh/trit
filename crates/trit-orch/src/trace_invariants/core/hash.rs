use crate::TraceEvent;

use super::{TraceInvariant, TraceReport, TraceViolation};

/// 第 5 条：内容绑定 + 写入序链接。
///
/// 多 trace 共用同一个 store 时，单 trace 过滤后的 `chain_seq` 会有缺口；
/// 缺口两侧的 `prev_hash` 指向本日志之外的事件，不能在本日志内验证——跳过，
/// 由 `TraceEventStore::load` 对全文件做严格链校验。
pub(super) fn check(events: &[&TraceEvent], report: &mut TraceReport) {
    let written_order = events.iter().all(|event| event.chain_seq > 0);
    let mut chain: Vec<&TraceEvent> = events.to_vec();
    if written_order {
        chain.sort_by_key(|event| event.chain_seq);
    }
    let mut previous: Option<&TraceEvent> = None;
    for event in &chain {
        // 非空摘要必须与当前内容一致：只改 payload 不改 hash 会在这里被抓到。
        if !event.payload_hash.is_empty() && event.payload_hash != event.computed_payload_hash() {
            report.violations.push(TraceViolation::at(
                TraceInvariant::HashChainLinked,
                Some(event.seq),
                "payload_hash == SHA-256(canonical payload)",
                format!("payload_hash={}", event.payload_hash),
            ));
        }
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
                let adjacent =
                    !written_order || prior.chain_seq.saturating_add(1) == event.chain_seq;
                if adjacent && event.prev_hash != prior.payload_hash {
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
}
