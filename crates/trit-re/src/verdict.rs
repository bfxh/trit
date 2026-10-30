//! 三值判定与四值冲突。
//!
//! 每个假设写成 `trit-core` 的表达式，交给 `trit-eval` 在 Kleene 语义下求值：
//! 证据缺失（`Unknown`）会顺着表达式传播，**不会被悄悄当成"没命中"**。
//!
//! 每个假设还可以附带一个"良性解释"表达式。若"假设成立"和"良性解释成立"
//! 同时为真，结论是 Belnap 四值的 `B`（两种解释都有证据）——这正是签名过的
//! 程序又被加壳时应该呈现的状态，而不是硬压成真或假。

use std::collections::BTreeMap;

use trit_belnap::B4;
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

use crate::witness::Facts;

/// 一个有名字的待判假设。
#[derive(Debug, Clone)]
pub struct Hypothesis {
    /// 稳定 id。
    pub id: &'static str,
    /// 人读标题。
    pub title: &'static str,
    /// 假设表达式。
    pub expr: Expr,
    /// 良性解释；与假设同时成立时给出 `B`。
    pub benign: Option<Expr>,
}

/// 判定结果。
#[derive(Debug, Clone)]
pub struct Judgment {
    /// 假设 id。
    pub id: &'static str,
    /// 假设标题。
    pub title: &'static str,
    /// Kleene 三值结论。
    pub value: Ternary,
    /// 四值结论（`B` = 冲突，`N` = 两边都没证据）。
    pub four: B4,
    /// 支持假设的事实。
    pub supporting: Vec<String>,
    /// 反驳假设的事实。
    pub refuting: Vec<String>,
    /// 证据不足的事实。
    pub undecided: Vec<String>,
}

impl Judgment {
    /// 一句话证据摘要。
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.supporting.is_empty() {
            parts.push(format!("支持: {}", self.supporting.join(", ")));
        }
        if !self.refuting.is_empty() {
            parts.push(format!("反驳: {}", self.refuting.join(", ")));
        }
        if !self.undecided.is_empty() {
            parts.push(format!("未知: {}", self.undecided.join(", ")));
        }
        if parts.is_empty() {
            "无相关证据".to_string()
        } else {
            parts.join("；")
        }
    }
}

fn var(name: &'static str) -> Expr {
    Expr::Var(name.to_string())
}

#[cfg(test)]
fn not(expr: Expr) -> Expr {
    Expr::logical_not(expr)
}

fn and(left: Expr, right: Expr) -> Expr {
    Expr::and(left, right)
}

fn or(left: Expr, right: Expr) -> Expr {
    Expr::or(left, right)
}

fn any4(a: Expr, b: Expr, c: Expr, d: Expr) -> Expr {
    or(or(a, b), or(c, d))
}

/// 内置假设集。id 会进台账，改动即破坏下游消费者，按契约对待。
pub fn default_hypotheses() -> Vec<Hypothesis> {
    // 「加壳」只看结构迹象，不把签名写进来：签名属于"良性解释"，
    // 两者同时成立才是四值的 B（签过名但结构像被打包 = 可疑篡改）。
    let packed = any4(
        var("section.high_entropy"),
        var("packer.section_name"),
        var("entry.outside_code"),
        var("overlay.high_entropy"),
    );
    let injection = and(
        and(var("imports.inj.alloc"), var("imports.inj.write")),
        var("imports.inj.exec"),
    );
    // 只用「运行期装载 + 网络」：IsDebuggerPresent / GetProcAddress 在正常
    // Windows 程序里几乎必然出现（notepad.exe 实测两者都有），带上反调试会全量误报。
    let staged_loader = and(var("imports.loader"), var("imports.network"));
    let credential_theft = and(var("imports.credential"), var("imports.network"));
    let needs_review = any4(
        var("packed"),
        var("injection"),
        var("staged_loader"),
        var("credential_theft"),
    );

    vec![
        Hypothesis {
            id: "packed",
            title: "结构像加壳/压缩（高熵、壳节名、入口异常或高熵叠加段）",
            expr: packed.clone(),
            benign: Some(var("authenticode.present")),
        },
        Hypothesis {
            id: "injection",
            title: "远程注入能力（申请+写+执行三件套齐备）",
            expr: injection,
            benign: None,
        },
        Hypothesis {
            id: "staged_loader",
            title: "分阶段装载（运行期装载 + 网络）",
            expr: staged_loader,
            benign: None,
        },
        Hypothesis {
            id: "credential_theft",
            title: "凭据窃取面（凭据 API + 网络外发）",
            expr: credential_theft,
            benign: None,
        },
        Hypothesis {
            id: "needs_review",
            title: "需要人工复核",
            expr: needs_review,
            benign: None,
        },
    ]
}

#[derive(Debug, Default, Clone, Copy)]
struct Polarity {
    positive: u32,
    negative: u32,
}

fn collect_polarity(expr: &Expr, negated: bool, out: &mut BTreeMap<String, Polarity>) {
    match expr {
        Expr::Const(_) => {}
        Expr::Var(name) => {
            let slot = out.entry(name.clone()).or_default();
            if negated {
                slot.negative += 1;
            } else {
                slot.positive += 1;
            }
        }
        Expr::Not(inner) => collect_polarity(inner, !negated, out),
        // 只用 ∧/∨/¬ 构造假设；蕴含/等价下极性分析是近似的，这里保守地按同号展开。
        Expr::And(a, b)
        | Expr::Or(a, b)
        | Expr::Xor(a, b)
        | Expr::Implies(a, b)
        | Expr::Equiv(a, b) => {
            collect_polarity(a, negated, out);
            collect_polarity(b, negated, out);
        }
    }
}

/// 对全部假设求值。
///
/// 假设按声明顺序求值，结果会**回注**赋值表，所以后面的假设可以引用前面的
/// 判定 id（例如 `needs_review` 引用 `packed`）。四值也一并回注，冲突可以向上传播。
pub fn evaluate(hypotheses: &[Hypothesis], facts: &Facts) -> Vec<Judgment> {
    let mut assignment = trit_eval::Assignment::new();
    for witness in facts.iter() {
        assignment.set(witness.id, witness.value);
    }
    let mut four_assignment = facts.belnap_assignment();
    let mut derived: BTreeMap<String, Ternary> = BTreeMap::new();

    let mut judgments = Vec::with_capacity(hypotheses.len());
    for hypothesis in hypotheses {
        let value = trit_eval::eval(&hypothesis.expr, &assignment, Algebra::Kleene);
        let four_expr = trit_belnap::eval(&hypothesis.expr, &four_assignment);
        let four = match &hypothesis.benign {
            Some(benign) => {
                let benign_value = trit_eval::eval(benign, &assignment, Algebra::Kleene);
                B4::from_rails(four_expr.is_designated(), benign_value == Ternary::True)
            }
            None => four_expr,
        };

        let mut polarity = BTreeMap::new();
        collect_polarity(&hypothesis.expr, false, &mut polarity);
        if let Some(benign) = &hypothesis.benign {
            collect_polarity(benign, false, &mut polarity);
        }

        let mut supporting = Vec::new();
        let mut refuting = Vec::new();
        let mut undecided = Vec::new();
        for (name, pol) in polarity {
            let fact = facts.get(&name).or_else(|| derived.get(&name).copied());
            match fact {
                Some(Ternary::Unknown) | None => undecided.push(name),
                Some(Ternary::True) => {
                    if pol.negative > 0 && pol.positive == 0 {
                        refuting.push(name);
                    } else if pol.positive > 0 && pol.negative == 0 {
                        supporting.push(name);
                    } else {
                        // 同一变量正反都出现：三种解释都可能，标成未知更安全。
                        undecided.push(name);
                    }
                }
                Some(Ternary::False) => {
                    if pol.negative > 0 && pol.positive == 0 {
                        supporting.push(name);
                    } else if pol.positive > 0 && pol.negative == 0 {
                        refuting.push(name);
                    } else {
                        undecided.push(name);
                    }
                }
            }
        }

        assignment.set(hypothesis.id, value);
        four_assignment.insert(hypothesis.id.to_string(), four);
        derived.insert(hypothesis.id.to_string(), value);
        judgments.push(Judgment {
            id: hypothesis.id,
            title: hypothesis.title,
            value,
            four,
            supporting,
            refuting,
            undecided,
        });
    }
    judgments
}

#[cfg(test)]
mod tests {
    use super::{Hypothesis, default_hypotheses, evaluate, var};
    use crate::witness::Facts;
    use trit_belnap::B4;
    use trit_core::value::Ternary;

    fn judgment<'a>(judgments: &'a [super::Judgment], id: &str) -> &'a super::Judgment {
        judgments.iter().find(|judgment| judgment.id == id).unwrap()
    }

    #[test]
    fn missing_evidence_stays_unknown() {
        let facts = Facts::new();
        let judgments = evaluate(&default_hypotheses(), &facts);
        for judgment in &judgments {
            assert_eq!(judgment.value, Ternary::Unknown, "{}", judgment.id);
            assert_eq!(judgment.four, B4::N, "{}", judgment.id);
        }
    }

    #[test]
    fn packed_true_but_signed_is_a_conflict() {
        let mut facts = Facts::new();
        facts.set_bool("section.high_entropy", true, "7.91");
        facts.set_bool("authenticode.present", true, "signedData");
        // 其余三个析取项必须为假，否则无法定位到高熵这一支。
        facts.set_bool("packer.section_name", false, "");
        facts.set_bool("entry.outside_code", false, "");
        facts.set_bool("overlay.high_entropy", false, "");
        let judgments = evaluate(&default_hypotheses(), &facts);
        let packed = judgment(&judgments, "packed");
        assert_eq!(packed.value, Ternary::True);
        assert_eq!(packed.four, B4::B);
    }

    #[test]
    fn loader_plus_anti_debug_is_not_staged_loader() {
        // notepad.exe 实测同时导入 GetProcAddress 与 IsDebuggerPresent，
        // 若把反调试算进 staged_loader，正常程序会全量误报。
        let mut facts = Facts::new();
        facts.set_bool("imports.loader", true, "");
        facts.set_bool("imports.anti_debug", true, "");
        facts.set_bool("imports.network", false, "");
        let judgments = evaluate(&default_hypotheses(), &facts);
        assert_eq!(judgment(&judgments, "staged_loader").value, Ternary::False);
    }

    #[test]
    fn unknown_entropy_makes_packed_unknown_even_with_packer_name() {
        let mut facts = Facts::new();
        facts.set("section.high_entropy", Ternary::Unknown, "节数据越界");
        facts.set_bool("authenticode.present", false, "");
        facts.set_bool("packer.section_name", false, "");
        facts.set_bool("entry.outside_code", false, "");
        facts.set_bool("overlay.high_entropy", false, "");
        let judgments = evaluate(&default_hypotheses(), &facts);
        // Unknown ∧ True = Unknown；其余项为 False，Kleene 或运算给 Unknown。
        assert_eq!(judgment(&judgments, "packed").value, Ternary::Unknown);
    }

    #[test]
    fn polaried_variable_lands_in_support_list() {
        let hypothesis = Hypothesis {
            id: "t",
            title: "t",
            expr: super::not(var("signed")),
            benign: None,
        };
        let mut facts = Facts::new();
        facts.set_bool("signed", false, "");
        let judgments = evaluate(&[hypothesis], &facts);
        assert_eq!(judgments[0].value, Ternary::True);
        assert_eq!(judgments[0].supporting, vec!["signed".to_string()]);
        assert!(judgments[0].refuting.is_empty());
    }

    #[test]
    fn conflicting_polarity_is_reported_as_undecided() {
        let hypothesis = Hypothesis {
            id: "t",
            title: "t",
            expr: super::and(var("a"), super::not(var("a"))),
            benign: None,
        };
        let mut facts = Facts::new();
        facts.set_bool("a", true, "");
        let judgments = evaluate(&[hypothesis], &facts);
        assert_eq!(judgments[0].undecided, vec!["a".to_string()]);
    }
}
