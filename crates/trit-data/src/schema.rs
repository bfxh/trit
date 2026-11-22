//! 轨迹的统一 schema：JSONL 一行一条，字段见 `docs/plans/AGENT-DATA-PLAN.md`。

use trit_learn::json::Json;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cost {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tool_calls: u64,
    pub ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub index: usize,
    pub role: String,
    pub tool: Option<String>,
    /// 规范化后的参数 JSON（对象）。
    pub args: Option<String>,
    pub observation: Option<String>,
    pub thought: Option<String>,
    pub error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub verifier: String,
    pub passed: bool,
    pub evidence: Vec<String>,
    pub final_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trajectory {
    pub schema: u64,
    pub task_id: String,
    pub source: Option<String>,
    pub license: Option<String>,
    pub model: String,
    pub harness: Option<String>,
    pub steps: Vec<Step>,
    pub outcome: Outcome,
    pub cost: Cost,
    pub labels: Vec<String>,
}

impl Trajectory {
    pub fn from_json(j: &Json) -> Result<Self, String> {
        let task_id = j.get_str("task_id").unwrap_or_default().trim().to_string();
        if task_id.is_empty() {
            return Err("缺 task_id".into());
        }
        let model = j.get_str("model").unwrap_or_default().trim().to_string();
        if model.is_empty() {
            return Err("缺 model".into());
        }
        let raw_steps = j
            .get("steps")
            .and_then(Json::as_array)
            .ok_or("缺 steps 数组")?;
        if raw_steps.is_empty() {
            return Err("steps 为空".into());
        }
        let mut steps = Vec::with_capacity(raw_steps.len());
        for (idx, s) in raw_steps.iter().enumerate() {
            let role = s.get_str("role").unwrap_or_default().trim().to_string();
            if role.is_empty() {
                return Err(format!("steps[{idx}] 缺 role"));
            }
            let action = s.get("action");
            let tool = action
                .and_then(|a| a.get_str("tool"))
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty());
            let args = action.and_then(|a| a.get("args")).map(Json::dump);
            steps.push(Step {
                index: s
                    .get("i")
                    .and_then(Json::as_u64)
                    .map(|v| v as usize)
                    .unwrap_or(idx),
                role,
                tool,
                args,
                observation: s.get_str("observation").map(|x| x.to_string()),
                thought: s.get_str("thought").map(|x| x.to_string()),
                error: s.get("error").and_then(Json::as_bool).unwrap_or(false),
            });
        }
        let outcome = j.get("outcome");
        let verifier = outcome
            .and_then(|o| o.get_str("verifier"))
            .unwrap_or("none")
            .trim()
            .to_string();
        let verifier = if verifier.is_empty() {
            "none".to_string()
        } else {
            verifier
        };
        let passed = outcome
            .and_then(|o| o.get("passed"))
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let evidence = outcome
            .and_then(|o| o.get("evidence"))
            .and_then(Json::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Json::as_str)
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let final_text = outcome
            .and_then(|o| o.get_str("final"))
            .unwrap_or_default()
            .to_string();
        let cost = j.get("cost");
        let num = |k: &str| {
            cost.and_then(|c| c.get(k))
                .and_then(Json::as_u64)
                .unwrap_or(0)
        };
        let labels = j
            .get("labels")
            .and_then(Json::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Json::as_str)
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Trajectory {
            schema: j.get("schema").and_then(Json::as_u64).unwrap_or(1),
            task_id,
            source: j.get_str("source").map(|s| s.to_string()),
            license: j.get_str("license").map(|s| s.to_string()),
            model,
            harness: j.get_str("harness").map(|s| s.to_string()),
            steps,
            outcome: Outcome {
                verifier,
                passed,
                evidence,
                final_text,
            },
            cost: Cost {
                tokens_in: num("tokens_in"),
                tokens_out: num("tokens_out"),
                tool_calls: num("tool_calls"),
                ms: num("ms"),
            },
            labels,
        })
    }
}
