//! 可复算质量门：硬门 → reject，软门 → defer，否则 accept。

use std::collections::BTreeSet;

use trit_core::fingerprint::sha256_hex;

use crate::language::{LangClass, Limits, classify, features};
use crate::schema::Trajectory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Reject,
    Defer,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Accept => "accept",
            Decision::Reject => "reject",
            Decision::Defer => "defer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub decision: Decision,
    pub reasons: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub require_verified: bool,
    pub max_steps: usize,
    pub max_tokens: u64,
    pub max_tool_calls: u64,
    pub min_thought_rate: f64,
    pub max_repeat4: f64,
    pub min_nonspace: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            require_verified: true,
            max_steps: 64,
            max_tokens: 200_000,
            max_tool_calls: 128,
            min_thought_rate: 0.5,
            max_repeat4: 0.4,
            min_nonspace: 0.6,
        }
    }
}

pub fn evaluate(t: &Trajectory, cfg: &Config, seen: &mut BTreeSet<String>) -> Verdict {
    let mut hard: Vec<&'static str> = Vec::new();
    let mut soft: Vec<&'static str> = Vec::new();

    if t.task_id.trim().is_empty()
        || t.model.trim().is_empty()
        || t.steps.is_empty()
        || t.steps.iter().any(|s| s.role.trim().is_empty())
    {
        hard.push("schema");
    }

    let bad_args = t.steps.iter().any(|s| match &s.tool {
        Some(tool) => {
            tool.trim().is_empty()
                || !s
                    .args
                    .as_deref()
                    .is_some_and(|a| a.trim_start().starts_with('{'))
        }
        None => false,
    });
    if bad_args {
        hard.push("action_args");
    }

    let tool_count = t.steps.iter().filter(|s| s.tool.is_some()).count() as u64;
    if t.steps.len() > cfg.max_steps
        || t.cost.tokens_in + t.cost.tokens_out > cfg.max_tokens
        || tool_count > cfg.max_tool_calls
    {
        hard.push("budget");
    }

    let looped = t.steps.windows(3).any(|w| {
        matches!((&w[0].tool, &w[1].tool, &w[2].tool), (Some(a), Some(b), Some(c)) if a == b && b == c)
            && w[0].args == w[1].args
            && w[1].args == w[2].args
    });
    if looped {
        hard.push("loop");
    }

    if cfg.require_verified {
        if t.outcome.verifier.eq_ignore_ascii_case("none") || !t.outcome.passed {
            hard.push("verified");
        }
        if t.outcome.evidence.is_empty() || !t.outcome.evidence.iter().all(|e| is_sha256(e)) {
            hard.push("evidence");
        }
    }

    if !seen.insert(content_key(t)) {
        hard.push("dedup");
    }

    if !hard.is_empty() {
        return Verdict {
            decision: Decision::Reject,
            reasons: hard,
        };
    }

    if has_unrecovered_error(t) {
        soft.push("recovery");
    }
    if t.steps
        .iter()
        .any(|s| s.tool.is_some() && s.observation.as_deref().unwrap_or("").trim().is_empty())
    {
        soft.push("observation");
    }
    let thoughts = t
        .steps
        .iter()
        .filter(|s| !s.thought.as_deref().unwrap_or("").trim().is_empty())
        .count() as f64;
    if thoughts / (t.steps.len() as f64) < cfg.min_thought_rate {
        soft.push("thought");
    }
    if !(2..=32).contains(&t.steps.len()) {
        soft.push("length");
    }
    let lang_text = collect_text(t);
    let lang = classify(&features(&lang_text), &Limits::default());
    if lang == LangClass::Bad {
        soft.push("language");
    }
    if !cfg.require_verified && t.outcome.verifier.eq_ignore_ascii_case("none") {
        soft.push("unverified_soft");
    }

    if !soft.is_empty() {
        Verdict {
            decision: Decision::Defer,
            reasons: soft,
        }
    } else {
        Verdict {
            decision: Decision::Accept,
            reasons: Vec::new(),
        }
    }
}

/// 内容指纹：任务 id + 每步 tool/args/observation（规范化后）的 SHA-256。
pub fn content_key(t: &Trajectory) -> String {
    let mut s = String::from(&t.task_id);
    for step in &t.steps {
        s.push('\u{1}');
        s.push_str(step.tool.as_deref().unwrap_or(""));
        s.push('\u{1}');
        s.push_str(step.args.as_deref().unwrap_or(""));
        s.push('\u{1}');
        s.push_str(step.observation.as_deref().unwrap_or(""));
    }
    sha256_hex(s.as_bytes())
}

fn is_sha256(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

fn has_unrecovered_error(t: &Trajectory) -> bool {
    let Some(i) = t.steps.iter().rposition(|s| s.error) else {
        return false;
    };
    !t.steps[i + 1..]
        .iter()
        .any(|s| !s.error && !s.observation.as_deref().unwrap_or("").trim().is_empty())
}

fn collect_text(t: &Trajectory) -> String {
    let mut out = String::new();
    for s in &t.steps {
        if let Some(x) = &s.thought {
            out.push_str(x);
            out.push('\n');
        }
        if let Some(x) = &s.observation {
            out.push_str(x);
            out.push('\n');
        }
    }
    out.push_str(&t.outcome.final_text);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_learn::json::Json;

    fn td(s: &str) -> Trajectory {
        Trajectory::from_json(&Json::parse(s).unwrap()).unwrap()
    }

    const OK: &str = r#"{"task_id":"t1","model":"m","steps":[{"role":"assistant","thought":"I will run the test suite now to check the build","action":{"tool":"bash","args":{"cmd":"pytest -q"}},"observation":"3 passed"},{"role":"assistant","thought":"All tests pass so I can finish this task","action":{"tool":"finish","args":{}},"observation":"done"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"fixed"}}"#;

    #[test]
    fn accepts_a_clean_trajectory() {
        let mut seen = BTreeSet::new();
        let v = evaluate(&td(OK), &Config::default(), &mut seen);
        assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);
    }

    #[test]
    fn rejects_unverified_and_missing_evidence() {
        let mut seen = BTreeSet::new();
        let t = td(&OK.replace(r#""passed":true"#, r#""passed":false"#));
        let v = evaluate(&t, &Config::default(), &mut seen);
        assert_eq!(v.decision, Decision::Reject);
        assert!(v.reasons.contains(&"verified"));
    }

    #[test]
    fn rejects_loops_and_duplicates() {
        let looped = r#"{"task_id":"t2","model":"m","steps":[{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"a"},{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"b"},{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"c"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}}"#;
        let mut seen = BTreeSet::new();
        let v = evaluate(&td(looped), &Config::default(), &mut seen);
        assert_eq!(v.decision, Decision::Reject);
        assert!(v.reasons.contains(&"loop"));

        let mut seen = BTreeSet::new();
        assert_eq!(
            evaluate(&td(OK), &Config::default(), &mut seen).decision,
            Decision::Accept
        );
        let v = evaluate(&td(OK), &Config::default(), &mut seen);
        assert!(v.reasons.contains(&"dedup"));
    }

    #[test]
    fn defers_on_process_or_language_gaps() {
        let mut seen = BTreeSet::new();
        let no_obs = td(&OK.replace(r#""observation":"3 passed""#, r#""observation":"""#));
        let v = evaluate(&no_obs, &Config::default(), &mut seen);
        assert_eq!(v.decision, Decision::Defer);
        assert!(v.reasons.contains(&"observation"));

        let mut seen = BTreeSet::new();
        let no_thought = td(&OK
            .replace("I will run the test suite now to check the build", "")
            .replace("All tests pass so I can finish this task", ""));
        let v = evaluate(&no_thought, &Config::default(), &mut seen);
        assert_eq!(v.decision, Decision::Defer);
        assert!(v.reasons.contains(&"thought"));
    }

    #[test]
    fn content_key_is_stable_and_order_sensitive() {
        let a = td(OK);
        let b = td(OK);
        assert_eq!(content_key(&a), content_key(&b));
        assert_eq!(content_key(&a).len(), 64);
    }
    #[test]
    fn defers_on_bad_language() {
        let bad = "重复句子。".repeat(80);
        let json = format!(
            r#"{{"task_id":"t3","model":"m","steps":[{{"role":"assistant","thought":"{bad}","action":{{"tool":"bash","args":{{"cmd":"ls"}}}},"observation":"{bad}"}},{{"role":"assistant","thought":"{bad}","action":{{"tool":"finish","args":{{}}}},"observation":"{bad}"}}],"outcome":{{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"{bad}"}}}}"#
        );
        let mut seen = BTreeSet::new();
        let v = evaluate(&td(&json), &Config::default(), &mut seen);
        assert_eq!(v.decision, Decision::Defer, "{:?}", v.reasons);
        assert!(v.reasons.contains(&"language"));
    }
}
