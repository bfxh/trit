//! 可复算质量门：硬门 → reject，软门 → defer，否则 accept。

use std::collections::BTreeSet;

use trit_core::fingerprint::sha256_hex;

use crate::content::LabelFilter;
use crate::language::{LangClass, Limits, classify, features};
use crate::schema::Trajectory;
use crate::script::{primary_language, trajectory_text};

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
    pub code_filter: bool,
    pub natural_filter: bool,
    pub code_threshold: f64,
    pub style: LabelFilter,
    pub semantics: LabelFilter,
    pub language_gate: bool,
    pub lang_allow: Option<Vec<String>>,
    pub lang_deny: Vec<String>,
    pub dedup: bool,
    pub loop_gate: bool,
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
            code_filter: false,
            natural_filter: false,
            code_threshold: 0.5,
            style: LabelFilter::default(),
            semantics: LabelFilter::default(),
            language_gate: true,
            lang_allow: None,
            lang_deny: Vec::new(),
            dedup: true,
            loop_gate: true,
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

    if cfg.loop_gate {
        let looped = t.steps.windows(3).any(|w| {
            matches!((&w[0].tool, &w[1].tool, &w[2].tool), (Some(a), Some(b), Some(c)) if a == b && b == c)
                && w[0].args == w[1].args
                && w[1].args == w[2].args
        });
        if looped {
            hard.push("loop");
        }
    }

    if cfg.require_verified {
        if t.outcome.verifier.eq_ignore_ascii_case("none") || !t.outcome.passed {
            hard.push("verified");
        }
        if t.outcome.evidence.is_empty() || !t.outcome.evidence.iter().all(|e| is_sha256(e)) {
            hard.push("evidence");
        }
    }

    if cfg.dedup && !seen.insert(content_key(t)) {
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
    let lang_text = trajectory_text(t);
    crate::gate_content::apply(cfg, &lang_text, &mut soft);
    if cfg.language_gate {
        let lang = primary_language(&lang_text);
        if cfg.lang_deny.iter().any(|x| x == lang) {
            soft.push("lang_denied");
        } else if cfg
            .lang_allow
            .as_ref()
            .is_some_and(|allow| !allow.iter().any(|x| x == lang))
        {
            soft.push("lang_not_allowed");
        }
        if classify(&features(&lang_text), &Limits::default()) == LangClass::Bad {
            soft.push("language");
        }
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

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
