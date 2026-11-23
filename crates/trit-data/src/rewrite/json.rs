//! 轨迹写回 JSON：字段与 `Trajectory::from_json` 对称，重写产物可再次过门。

use trit_learn::json::Json;

use crate::schema::Trajectory;

pub fn to_json(t: &Trajectory) -> Json {
    let mut steps = Vec::with_capacity(t.steps.len());
    for s in &t.steps {
        let args = s
            .args
            .as_deref()
            .and_then(|a| Json::parse(a).ok())
            .unwrap_or_else(Json::obj);
        let mut step = Json::obj()
            .put("i", s.index as u64)
            .put("role", s.role.clone())
            .put(
                "action",
                Json::obj()
                    .put("tool", s.tool.clone().unwrap_or_default())
                    .put("args", args),
            )
            .put("error", s.error);
        if let Some(x) = &s.thought {
            step = step.put("thought", x.clone());
        }
        if let Some(x) = &s.observation {
            step = step.put("observation", x.clone());
        }
        steps.push(step);
    }
    let mut out = Json::obj()
        .put("schema", t.schema)
        .put("task_id", t.task_id.clone())
        .put("model", t.model.clone())
        .put("steps", Json::arr(steps))
        .put(
            "outcome",
            Json::obj()
                .put("verifier", t.outcome.verifier.clone())
                .put("passed", t.outcome.passed)
                .put(
                    "evidence",
                    Json::arr(t.outcome.evidence.iter().cloned().map(Json::str).collect()),
                )
                .put("final", t.outcome.final_text.clone()),
        )
        .put(
            "cost",
            Json::obj()
                .put("tokens_in", t.cost.tokens_in)
                .put("tokens_out", t.cost.tokens_out)
                .put("tool_calls", t.cost.tool_calls)
                .put("ms", t.cost.ms),
        )
        .put(
            "labels",
            Json::arr(t.labels.iter().cloned().map(Json::str).collect()),
        );
    if let Some(x) = &t.source {
        out = out.put("source", x.clone());
    }
    if let Some(x) = &t.license {
        out = out.put("license", x.clone());
    }
    if let Some(x) = &t.harness {
        out = out.put("harness", x.clone());
    }
    out
}
