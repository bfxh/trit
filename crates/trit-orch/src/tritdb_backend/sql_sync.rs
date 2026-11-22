//! 轨迹事件 → `trit_trace` 表的增量物化：只插 `chain_seq` 更新的行，不重建表。

use std::collections::BTreeMap;
use std::path::Path;

use tritdb_sql::{QueryResult, SqlEngine};

use crate::Error;

const TABLE: &str = "trit_trace";
const CREATE: &str = "CREATE TABLE IF NOT EXISTS trit_trace (\
     trace_id TEXT, seq INT, chain_seq INT, kind TEXT, actor TEXT, \
     observed_at INT, recorded_at INT, event_time INT, payload_hash TEXT, payload TEXT);";

pub(super) fn sync_events(
    engine: &mut SqlEngine,
    events: &[crate::TraceEvent],
) -> Result<(), Error> {
    engine
        .execute_script(CREATE)
        .map_err(|error| super::error(Path::new(TABLE), error))?;
    let watermark = read_watermark(engine)?;

    for chunk in events.chunks(64) {
        let pending: Vec<&crate::TraceEvent> = chunk
            .iter()
            .filter(|event| event.chain_seq > watermark.get(&event.trace_id).copied().unwrap_or(0))
            .collect();
        if pending.is_empty() {
            continue;
        }
        let script = insert_script(&pending);
        engine
            .execute_script(&script)
            .map_err(|error| super::error(Path::new(TABLE), error))?;
    }
    Ok(())
}

/// 表里每个 trace 已物化到的 `chain_seq`。
fn read_watermark(engine: &mut SqlEngine) -> Result<BTreeMap<String, u64>, Error> {
    let mut watermark = BTreeMap::new();
    let results = engine
        .execute_script("SELECT trace_id, max(chain_seq) AS m FROM trit_trace GROUP BY trace_id")
        .map_err(|error| super::error(Path::new(TABLE), error))?;
    for result in results {
        if let QueryResult::Rows { rows, .. } = result {
            for row in rows {
                if row.len() < 2 {
                    continue;
                }
                if let Ok(max) = row[1].display().parse::<u64>() {
                    watermark.insert(row[0].display(), max);
                }
            }
        }
    }
    Ok(watermark)
}

fn insert_script(pending: &[&crate::TraceEvent]) -> String {
    let mut script = format!("INSERT INTO {TABLE} VALUES ");
    for (index, event) in pending.iter().enumerate() {
        if index > 0 {
            script.push(',');
        }
        let payload = event.payload.as_ref().map_or_else(
            || "NULL".to_string(),
            |payload| quote(&serde_json::to_string(payload).expect("payload")),
        );
        let event_time = event
            .event_time
            .map_or_else(|| "NULL".to_string(), |event_time| event_time.to_string());
        script.push_str(&format!(
            "({},{},{},{},{},{},{},{},{},{})",
            quote(&event.trace_id),
            event.seq,
            event.chain_seq,
            quote(event.kind.as_str()),
            quote(&format!("{:?}", event.actor)),
            event.observed_at,
            event.recorded_at,
            event_time,
            quote(&event.payload_hash),
            payload
        ));
    }
    script.push(';');
    script
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
