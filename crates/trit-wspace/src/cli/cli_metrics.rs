//! CLI `cost` / `history`：延迟雷与改良历史。

use serde_json::json;

use crate::audit::AuditLog;
use crate::cost::cost;
use crate::history::summary;

use super::{
    emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, read_source, value,
};

pub(super) fn cost_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "cost <artifact|-> --oracle <json|-> [--top K]")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let top = opt_usize(&parsed, "top", 8)?;
    let report = cost(&net, &oracle, top).map_err(err)?;
    emit(&json!({
        "samples": report.samples, "total_multiplies": report.total,
        "mean": report.mean, "max": report.max, "min": report.min,
        "per_layer": report.per_layer,
        "top": report.top.iter().map(|sample| json!({
            "sample": sample.sample,
            "multiplies": sample.multiplies,
        })).collect::<Vec<_>>(),
    }))
}

pub(super) fn history_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let source = value(&parsed, "log")?;
    let text = read_source(source)?;
    let log = AuditLog::from_jsonl(&text).map_err(err)?;
    let summary = summary(&log);
    let timestamps = crate::history_time::extract(&text);
    let present: Vec<u64> = timestamps.iter().flatten().copied().collect();
    let first = present.first().copied();
    let last = present.last().copied();
    emit(&json!({
        "entries": summary.entries, "applied": summary.applied,
        "rolled_back": summary.rolled_back, "skipped": summary.skipped,
        "edits": summary.edits, "first_ts": first, "last_ts": last,
        "by_name": summary.by_name.iter().map(|(name, count)| json!({
            "name": name,
            "count": count,
        })).collect::<Vec<_>>(),
        "events": summary.events.iter().map(|event| json!({
            "seq": event.seq,
            "name": event.name,
            "applied": event.applied,
            "rolled_back": event.rolled_back,
            "edits": event.edits,
        })).collect::<Vec<_>>(),
    }))
}
