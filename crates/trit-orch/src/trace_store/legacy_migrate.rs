use std::path::Path;

use crate::TraceEvent;

/// 内存迁移：按 seq 排序，写入序从 1 连续分配，逐条封存。
pub(crate) fn migrate_in_memory(mut events: Vec<TraceEvent>) -> Vec<TraceEvent> {
    events.sort_by_key(|event| event.seq);
    let mut previous = String::new();
    for (index, event) in events.iter_mut().enumerate() {
        event.chain_seq = (index + 1) as u64;
        event.seal(&previous);
        previous = event.payload_hash.clone();
    }
    events
}

/// 文件里是否已有内容（存在但为空不算）。
pub(crate) fn has_content(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}
