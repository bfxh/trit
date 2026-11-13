//! 多主机轨迹归并：因果边决定先后，HLC 只给并发事件破平。
//!
//! 同一 trace 内按 seq 连接；显式 `causation_id` 再加因果边。只有入度为 0 的
//! 事件能被输出，因此 HLC 再小也不能越过未结算的前驱。

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::{Error, Hlc, TraceEvent, TraceLog};

/// 合并多个 trace 日志，返回满足因果约束的全局顺序。
///
/// 重复 event id 且内容相同则去重；内容不同判冲突。无法拓扑排序时判因果环。
/// 没有 HLC 的事件按 `Hlc::ZERO` 参与破平，仍受 seq/因果边约束。
pub fn merge_logs(logs: &[&TraceLog]) -> Result<Vec<TraceEvent>, Error> {
    let mut unique: BTreeMap<String, TraceEvent> = BTreeMap::new();
    for log in logs {
        for event in log.events() {
            match unique.get(&event.id) {
                Some(existing) if existing == event => {}
                Some(_) => {
                    return Err(Error::TraceEventConflict {
                        id: event.id.clone(),
                    });
                }
                None => {
                    unique.insert(event.id.clone(), event.clone());
                }
            }
        }
    }
    let events: Vec<TraceEvent> = unique.into_values().collect();

    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_trace_seq: BTreeMap<(String, u64), usize> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        by_id.insert(event.id.clone(), index);
        let key = (event.trace_id.clone(), event.seq);
        if let Some(&previous) = by_trace_seq.get(&key) {
            if events[previous].id != event.id {
                return Err(Error::TraceSeqConflict { seq: event.seq });
            }
        } else {
            by_trace_seq.insert(key, index);
        }
    }

    let mut children = vec![Vec::new(); events.len()];
    let mut indegree = vec![0usize; events.len()];
    for (index, event) in events.iter().enumerate() {
        if event.seq > 0
            && let Some(&previous) = by_trace_seq.get(&(event.trace_id.clone(), event.seq - 1))
        {
            add_edge(previous, index, &mut children, &mut indegree);
        }
        if let Some(cause) = &event.causation_id
            && let Some(&from) = by_id.get(cause)
        {
            add_edge(from, index, &mut children, &mut indegree);
        }
    }

    let mut ready = BinaryHeap::new();
    for (index, event) in events.iter().enumerate() {
        if indegree[index] == 0 {
            ready.push(Reverse(ready_key(event)));
        }
    }

    let mut ordered = Vec::with_capacity(events.len());
    while let Some(Reverse((_, _, _, id))) = ready.pop() {
        let Some(&index) = by_id.get(&id) else {
            continue;
        };
        ordered.push(events[index].clone());
        for &child in &children[index] {
            indegree[child] = indegree[child]
                .checked_sub(1)
                .expect("each validated edge is removed once");
            if indegree[child] == 0 {
                ready.push(Reverse(ready_key(&events[child])));
            }
        }
    }

    if ordered.len() != events.len() {
        let remaining = events
            .iter()
            .find(|event| {
                by_id
                    .get(&event.id)
                    .is_some_and(|&index| indegree[index] > 0)
            })
            .map(|event| event.id.clone())
            .unwrap_or_default();
        return Err(Error::TraceCausalityCycle { id: remaining });
    }
    Ok(ordered)
}

fn ready_key(event: &TraceEvent) -> (Hlc, String, u64, String) {
    (
        event.hlc.unwrap_or(Hlc::ZERO),
        event.trace_id.clone(),
        event.seq,
        event.id.clone(),
    )
}

fn add_edge(from: usize, to: usize, children: &mut [Vec<usize>], indegree: &mut [usize]) {
    if children[from].contains(&to) {
        return;
    }
    children[from].push(to);
    indegree[to] += 1;
}
