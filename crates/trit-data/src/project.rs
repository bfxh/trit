//! 把 `trit-orch` 的 TraceEvent JSONL 投影成 `trit-data` 的轨迹 JSON。
//!
//! 投影只做**机械映射**：事件 → step，`finish_confirmed` → 验证通过，
//! `payload_hash`（空则用事件内容哈希）→ evidence。不补写不存在的 thought。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::fingerprint::sha256_hex;
use trit_learn::json::Json;

pub fn project_jsonl(text: &str) -> Result<Vec<Json>, String> {
    let mut by_trace: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let j = Json::parse(line).map_err(|e| format!("第 {} 行: {e}", i + 1))?;
        let tid = j
            .get_str("trace_id")
            .ok_or_else(|| format!("第 {} 行缺 trace_id", i + 1))?
            .to_string();
        by_trace.entry(tid).or_default().push(j);
    }
    let mut out = Vec::new();
    for (tid, mut events) in by_trace {
        events.sort_by_key(|j| j.get("chain_seq").and_then(Json::as_u64).unwrap_or(0));
        out.push(project_trace(&tid, &events));
    }
    Ok(out)
}

fn project_trace(trace_id: &str, events: &[Json]) -> Json {
    let mut steps = Vec::new();
    let mut evidence: BTreeSet<String> = BTreeSet::new();
    let mut passed = false;
    let mut last_kind = "none".to_string();
    for (i, j) in events.iter().enumerate() {
        let kind = j.get_str("kind").unwrap_or("unknown");
        let (role, who) = actor_of(j);
        let args = match j.get("payload") {
            Some(p) if matches!(p, Json::Obj(_)) => p.dump(),
            _ => "{}".to_string(),
        };
        let raw = j
            .get_str("payload_hash")
            .filter(|h| !h.is_empty())
            .map(|h| h.to_string())
            .unwrap_or_else(|| sha256_hex(j.dump().as_bytes()));
        let sha = if raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
            format!("sha256:{raw}")
        } else {
            format!("sha256:{}", sha256_hex(raw.as_bytes()))
        };
        evidence.insert(sha.clone());
        if kind == "finish_confirmed" {
            passed = true;
        }
        last_kind = kind.to_string();
        let error = matches!(
            kind,
            "finish_refused" | "trace_gap" | "task_denied" | "effect_refuted"
        );
        steps.push(
            Json::obj()
                .put("i", i as u64)
                .put("role", role)
                .put(
                    "action",
                    Json::obj()
                        .put("tool", kind)
                        .put("args", Json::parse(&args).unwrap_or(Json::obj())),
                )
                .put("observation", format!("{kind} {trace_id} {sha}"))
                .put("error", error)
                .put("actor", who),
        );
    }
    let n = events.len() as u64;
    Json::obj()
        .put("schema", 1u32)
        .put("task_id", trace_id)
        .put("source", "trit-orch:trace")
        .put("model", "trit-orch")
        .put("harness", "trit-orch")
        .put("steps", Json::arr(steps))
        .put(
            "outcome",
            Json::obj()
                .put("verifier", if passed { "trace" } else { "none" })
                .put("passed", passed)
                .put(
                    "evidence",
                    Json::arr(evidence.into_iter().map(Json::str).collect()),
                )
                .put("final", format!("{last_kind} {trace_id}")),
        )
        .put(
            "cost",
            Json::obj()
                .put("tokens_in", 0u64)
                .put("tokens_out", 0u64)
                .put("tool_calls", n)
                .put("ms", 0u64),
        )
        .put("labels", Json::arr(vec![Json::str("trace_projection")]))
}

fn actor_of(j: &Json) -> (String, String) {
    match j.get("actor") {
        Some(Json::Str(s)) => (s.to_lowercase(), s.clone()),
        Some(Json::Obj(fields)) => {
            for (k, v) in fields {
                if let Some(name) = v.as_str() {
                    return (k.to_lowercase(), name.to_string());
                }
            }
            ("agent".into(), String::new())
        }
        _ => ("unknown".into(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{Config, Decision, evaluate};
    use crate::schema::Trajectory;

    const EV1: &str = r#"{"schema":1,"id":"e1","trace_id":"t1","span_id":"s1","parent":null,"seq":1,"kind":"task_selected","payload":{"kind":"plan","revision":1,"tasks":[]},"causation_id":null,"correlation_id":"c1","logical":{"revision":1,"epoch":0,"fence":0,"round":0,"attempt":0},"event_time":null,"observed_at":100,"recorded_at":100,"actor":"Host","payload_hash":"","prev_hash":"","chain_seq":1}"#;
    const EV2: &str = r#"{"schema":1,"id":"e2","trace_id":"t1","span_id":"s2","parent":null,"seq":2,"kind":"finish_confirmed","payload":null,"causation_id":null,"correlation_id":"c1","logical":{"revision":1,"epoch":0,"fence":0,"round":0,"attempt":0},"event_time":null,"observed_at":200,"recorded_at":200,"actor":{"Agent":"worker"},"payload_hash":"","prev_hash":"","chain_seq":2}"#;

    #[test]
    fn projects_one_trace_into_one_trajectory() {
        let out = project_jsonl(&format!("{EV1}\n{EV2}\n")).unwrap();
        assert_eq!(out.len(), 1);
        let t = Trajectory::from_json(&out[0]).unwrap();
        assert_eq!(t.task_id, "t1");
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[0].tool.as_deref(), Some("task_selected"));
        assert_eq!(t.steps[1].role, "agent");
        assert!(t.outcome.passed);
        assert_eq!(t.outcome.verifier, "trace");
        assert_eq!(t.outcome.evidence.len(), 2);
        assert!(t.outcome.evidence.iter().all(|e| e.starts_with("sha256:")));
    }

    #[test]
    fn projected_trace_can_pass_gate_without_thought() {
        let out = project_jsonl(&format!("{EV1}\n{EV2}\n")).unwrap();
        let t = Trajectory::from_json(&out[0]).unwrap();
        let cfg = Config {
            min_thought_rate: 0.0,
            ..Config::default()
        };
        let mut seen = std::collections::BTreeSet::new();
        let v = evaluate(&t, &cfg, &mut seen);
        assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);
    }
}
