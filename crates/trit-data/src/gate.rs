//! 可复算质量门：硬门 → reject，软门 → defer，否则 accept。
use std::collections::BTreeSet;
use crate::content::LabelFilter;
use crate::gate_keys::{has_unrecovered_error, is_sha256};
use crate::lang_policy::{hard_reason, soft_reason};
use crate::language::{Limits, classify, features};
use crate::schema::Trajectory;
use crate::script::{primary_language, trajectory_text};
pub use crate::gate_keys::content_key;
pub use crate::lang_policy::LangPolicy;
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
    pub formats: LabelFilter,
    pub bits: LabelFilter,
    pub code_quality: bool,
    pub code_quality_limits: crate::code_quality::Limits,
    pub edu_min: f64,
    pub language_gate: bool,
    pub lang_allow: Option<Vec<String>>,
    pub lang_deny: Vec<String>,
    pub lang_policy: LangPolicy,
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
            formats: LabelFilter::default(),
            bits: LabelFilter::default(),
            code_quality: false,
            code_quality_limits: crate::code_quality::Limits::default(),
            edu_min: 0.0,
            language_gate: true,
            lang_allow: None,
            lang_deny: Vec::new(),
            lang_policy: LangPolicy::Defer,
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

    // 语言分类要能进硬门，所以必须在 `hard` 判空之前算；关掉语言门时不算分类。
    let lang_text = trajectory_text(t);
    let lang_class = cfg
        .language_gate
        .then(|| classify(&features(&lang_text), &Limits::default()));
    if let Some(r) = hard_reason(cfg.lang_policy, lang_class) {
        hard.push(r);
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
        if let Some(r) = soft_reason(cfg.lang_policy, lang_class) {
            soft.push(r);
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

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
