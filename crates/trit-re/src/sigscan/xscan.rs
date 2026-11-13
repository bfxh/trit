//! 扫描核心（接缝实现）：命中收集 + 表达式求值 + 规则裁决。
//!
//! 旧声明式条件走 [`super::eval`]；`Condition::Expr` 走 [`super::xeval`]。
//! 三值纪律不变：截断/封顶/预算耗尽的「未命中」折 `Unknown`。

use super::expr::Expr;
use super::rule::Rule;
use super::xval::{Val, truth};
use super::{Pattern, eval};
use super::{RuleVerdict, SCAN_BUDGET, SigReport, find_hex, find_text};
use super::{pe2, xeval, xfacts::StrFact};
use trit_core::value::Ternary;

pub(crate) fn scan_rules(
    rules: &[Rule],
    data: &[u8],
    truncated: bool,
    entrypoint: Option<u64>,
    pe: Option<&crate::pe::PeInfo>,
) -> SigReport {
    let mut out = Vec::new();
    let mut rule_values: Vec<(String, Val)> = Vec::new();
    for rule in rules {
        let mut occurrences: Vec<(String, Vec<u64>)> = Vec::new();
        let mut facts: Vec<StrFact> = Vec::new();
        let mut complete = true;
        let mut has_unscannable = false;
        for (_id, pattern) in &rule.patterns {
            let (hits, pattern_complete, fixed_len) = match pattern {
                Pattern::Hex(hp) => {
                    let (h, c) = find_hex(data, hp);
                    (h, c, None)
                }
                Pattern::Text(tp) => {
                    let (h, c) = find_text(data, tp);
                    (h, c, None)
                }
                Pattern::Regex(rp) => {
                    let (h, c) = super::regex::find_all(data, rp, SCAN_BUDGET);
                    (h, c, None)
                }
                Pattern::Base64(tps) => {
                    let (h, c) = super::b64::find_union(data, tps);
                    (h, c, None)
                }
                Pattern::Unscannable(_) => {
                    has_unscannable = true;
                    (Vec::new(), true, None)
                }
            };
            complete &= pattern_complete;
            occurrences.push((_id.clone(), hits.clone()));
            facts.push(StrFact {
                hits,
                exact: pattern_complete,
                fixed_len,
                complete: pattern_complete && !truncated,
            });
        }
        let ctx = xeval::XCtx {
            data,
            truncated,
            entrypoint,
            tree: &pe2::build(data),
            facts: &facts,
            strings: &[],
            rules: &rule_values,
            imports: &[],
        };
        let met = match &rule.condition {
            super::Condition::Expr(expr) => judge_expr(expr, &ctx),
            other => eval(
                other,
                &occurrences,
                &eval::EvalCtx {
                    data,
                    truncated,
                    binding: None,
                    entrypoint,
                    pe,
                },
            ),
        };
        let value = if has_unscannable {
            Ternary::Unknown
        } else {
            match met {
                Ternary::True => Ternary::True,
                Ternary::Unknown => Ternary::Unknown,
                Ternary::False if truncated || !complete => Ternary::Unknown,
                Ternary::False => Ternary::False,
            }
        };
        let hits = rule.report_hits(&occurrences);
        rule_values.push((rule.name.clone(), val_of(value)));
        out.push(RuleVerdict {
            rule: rule.name.clone(),
            value,
            hits,
        });
    }
    let mut value = Ternary::False;
    for r in &out {
        value = match (value, r.value) {
            (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
            (Ternary::Unknown, _) | (_, Ternary::Unknown) => Ternary::Unknown,
            _ => Ternary::False,
        };
    }
    SigReport { rules: out, value }
}

fn judge_expr(expr: &Expr, ctx: &xeval::XCtx) -> Ternary {
    let mut env = xeval::Env::default();
    match truth(&xeval::eval(expr, ctx, &mut env)) {
        Some(true) => Ternary::True,
        Some(false) => Ternary::False,
        None => Ternary::Unknown,
    }
}

fn val_of(t: Ternary) -> Val {
    match t {
        Ternary::True => Val::Int(1),
        Ternary::False => Val::Int(0),
        Ternary::Unknown => Val::Unknown,
    }
}
