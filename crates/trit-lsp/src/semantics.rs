//! 深层三值诊断：专抓「只在二值布尔下成立」的写法。
//!
//! 三套语义（K3 / Ł3 / B3）对 `X` 的处理不同（CONTRIBUTING 的红线）：
//! - **矛盾律** `a ∧ ¬a = 0` 在三套语义下都**不成立**：`a = X` 时结果为 `X`；
//! - **排中律** `a ∨ ¬a = 1` 同样不成立：`a = X` 时结果为 `X`；
//! - 同一表达式在不同语义下可能给出**不同结果**（如 Ł3 的 `a ⇒ a = 1`，K3 是 `X`）。
//!
//! 这些诊断在编辑期指出"这里写了布尔化简"或"这里还没固定语义"，
//! 判定一律复用 `trit-core` / `trit-eval`（语义只有一份，不在 LSP 里抄第二份）。
//!
//! 边界：语义分歧只对拍 **K3 vs Ł3**——B3 的毒值吸收使任何含 X 的求值
//! 几乎必然与它们不同，对拍三个语义会退化为噪音；B3 的规则差异靠
//! 悬停里的真值表呈现。

use trit_core::{Algebra, Expr};
use trit_eval::eval::{Assignment, eval};

use crate::analysis::{self, Diagnostic};
use crate::lex::{self, Token};

/// 语义分歧穷举的变量数上限（3^4 = 81 行；超出不再对拍，属声明过的边界）。
const MAX_DIVERGE_VARS: usize = 4;

/// 一个「操作数」的词法形态：`x`、`!x`、`(x)`、`(!x)`。
struct Operand {
    name: String,
    negated: bool,
    /// 匹配结束后的 token 下标（不含）。
    end: usize,
    /// 起始字节。
    start: usize,
    /// 结束字节（不含）。
    stop: usize,
}

fn read_operand(tokens: &[Token], i: usize) -> Option<Operand> {
    let token = tokens.get(i)?;
    match token.kind {
        crate::lex::Kind::Ident if analysis::is_variable(token) => Some(Operand {
            name: token.text.clone(),
            negated: false,
            end: i + 1,
            start: token.start,
            stop: token.end,
        }),
        crate::lex::Kind::Operator if lex::operator(&token.text).map(|op| op.arity) == Some(1) => {
            let name = tokens.get(i + 1)?;
            if !analysis::is_variable(name) {
                return None;
            }
            Some(Operand {
                name: name.text.clone(),
                negated: true,
                end: i + 2,
                start: token.start,
                stop: name.end,
            })
        }
        crate::lex::Kind::LParen => {
            // `(x)` 或 `(!x)`。
            let inner = tokens.get(i + 1)?;
            let (operand, negated) = match inner.kind {
                crate::lex::Kind::Ident => (inner, false),
                crate::lex::Kind::Operator
                    if lex::operator(&inner.text).map(|op| op.arity) == Some(1) =>
                {
                    (tokens.get(i + 2)?, true)
                }
                _ => return None,
            };
            // `(x)` 的右括号在 i+2、`(!x)` 的在 i+3。
            let rparen = if negated { i + 3 } else { i + 2 };
            if !analysis::is_variable(operand)
                || tokens.get(rparen)?.kind != crate::lex::Kind::RParen
            {
                return None;
            }
            let end = rparen + 1;
            Some(Operand {
                name: operand.text.clone(),
                negated,
                end,
                start: token.start,
                stop: tokens[i + 3].end,
            })
        }
        _ => None,
    }
}

/// 矛盾律 / 排中律陷阱：`a & !a`、`!a & a`、`a | !a`、`!a | a`，
/// 两侧各允许一层括号（`(a) & (!a)`）。返回警告诊断（按起始字节去重）。
pub fn law_lints(tokens: &[Token]) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        let Some(left) = read_operand(tokens, i) else {
            i += 1;
            continue;
        };
        // 任何「不是陷阱」的路径都必须推进 i（至少跳过已匹配的操作数），
        // 否则 `a ! b` 这类序列会原地重扫死循环。
        let Some(op) = tokens.get(left.end) else {
            i = left.end;
            continue;
        };
        let Some(info) = lex::operator(&op.text) else {
            i = left.end;
            continue;
        };
        let law = match info.primary() {
            "&" => "contradiction",
            "|" => "excluded_middle",
            _ => {
                i = left.end;
                continue;
            }
        };
        let Some(right) = read_operand(tokens, left.end + 1) else {
            i = left.end;
            continue;
        };
        if left.name != right.name || left.negated == right.negated {
            i = left.end;
            continue;
        }
        if out.iter().any(|d| d.start == left.start) {
            i = right.end;
            continue;
        }
        let message = if law == "contradiction" {
            format!(
                "矛盾律在三套语义下都不成立：`{} {} !{}` 在 a = X 时结果是 X（K3/Ł3/B3 一致），\
不是 False。若本意是互斥条件，请显式写全三分支。",
                left.name,
                info.primary(),
                left.name
            )
        } else {
            format!(
                "排中律在三套语义下都不成立：`{} {} !{}` 在 a = X 时结果是 X（K3/Ł3/B3 一致），\
不是 True。不要把它当恒真条件消掉。",
                left.name,
                info.primary(),
                left.name
            )
        };
        out.push(Diagnostic {
            message,
            severity: crate::analysis::SEV_WARNING,
            start: left.start,
            end: right.stop,
        });
        i = right.end;
    }
    out
}

/// 语义分歧：穷举（变量 ≤ `MAX_DIVERGE_VARS`，当前 4）所有赋值，
/// 对 K3 / Ł3 各求值一次；存在分歧时返回带具体反例的消息。
pub fn semantic_divergence(expr: &Expr) -> Option<String> {
    let vars = expr.variables();
    if vars.is_empty() || vars.len() > MAX_DIVERGE_VARS {
        return None;
    }
    for code in 0..3usize.pow(vars.len() as u32) {
        let mut assignment = Assignment::new();
        let mut shown: Vec<(String, char)> = Vec::new();
        for (index, name) in vars.iter().enumerate() {
            let digit = (code / 3usize.pow(index as u32)) % 3;
            let value = match digit {
                0 => trit_core::value::Ternary::False,
                1 => trit_core::value::Ternary::Unknown,
                _ => trit_core::value::Ternary::True,
            };
            assignment.set(name, value);
            shown.push((name.clone(), value.as_char()));
        }
        let kleene = eval(expr, &assignment, Algebra::Kleene);
        let lukasiewicz = eval(expr, &assignment, Algebra::Lukasiewicz);
        if kleene != lukasiewicz {
            let detail: Vec<String> = shown
                .into_iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            return Some(format!(
                "语义分歧：K3 与 Ł3 对该表达式给出不同结果。例：{} 时 K3={}、Ł3={}。\
（B3 的毒值传播几乎总是再不同，不参与对拍。）混用语义规则前，\
先用 initializationOptions.algebra 固定本文档的语义。",
                detail.join(", "),
                kleene.as_char(),
                lukasiewicz.as_char()
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use trit_core::Parser as ExprParser;

    fn messages(text: &str) -> Vec<String> {
        analyze(text)
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn contradiction_is_flagged_in_all_spellings() {
        for text in [
            "a & !a",
            "!a & a",
            "(a) & (!a)",
            "a && !a",
            "a ∧ !a",
            "b | (a & !a)",
        ] {
            let all = messages(text);
            assert!(
                all.iter().any(|m| m.contains("矛盾律")),
                "{text} 未报矛盾律：{all:?}"
            );
        }
    }

    #[test]
    fn excluded_middle_is_flagged() {
        for text in ["a | !a", "!a | a", "(a) | (!a)", "a || !a", "a ∨ !a"] {
            let all = messages(text);
            assert!(
                all.iter().any(|m| m.contains("排中律")),
                "{text} 未报排中律：{all:?}"
            );
        }
    }

    #[test]
    fn different_variables_and_double_negation_are_clean() {
        // 不同变量、双侧同否定（等价于 ¬a 的合法写法）、常量词、异或都不该报。
        for text in ["a & !b", "!a & !a", "true & !true", "a & b", "x ^ !x"] {
            let all = messages(text);
            assert!(
                !all.iter()
                    .any(|m| m.contains("矛盾律") || m.contains("排中律")),
                "{text} 误报：{all:?}"
            );
        }
    }

    #[test]
    fn lint_message_states_the_x_case() {
        // 消息里的断言本身要对：a=X 时 a∧¬a 在三套语义下都是 X。
        let expr = ExprParser::parse("a & !a").unwrap();
        let mut assignment = Assignment::new();
        assignment.set("a", trit_core::value::Ternary::Unknown);
        for algebra in [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar] {
            assert_eq!(
                eval(&expr, &assignment, algebra),
                trit_core::value::Ternary::Unknown,
                "{algebra:?}"
            );
        }
    }

    #[test]
    fn lukasiewicz_implies_self_diverges() {
        // Ł3 的 a ⇒ a = 1，K3 是 X，B3 是 X —— 必须报语义分歧并给出反例。
        let expr = ExprParser::parse("a => a").unwrap();
        let message = semantic_divergence(&expr).expect("应报语义分歧");
        assert!(message.contains("语义分歧"), "{message}");
        assert!(message.contains("Ł3=1"), "{message}");
    }

    #[test]
    fn plain_conjunction_does_not_diverge() {
        let expr = ExprParser::parse("a & b").unwrap();
        assert_eq!(semantic_divergence(&expr), None);
    }

    #[test]
    fn divergence_skips_many_variable_expressions() {
        // 6 个变量超出穷举预算：不报（3^6 行太贵），这是声明过的边界。
        let expr = ExprParser::parse("a => b => c => d => e").unwrap();
        assert_eq!(semantic_divergence(&expr), None);
    }

    #[test]
    fn diagnostics_carry_lint_severity_and_span() {
        let analysis = analyze("a & !a");
        let lint = analysis
            .diagnostics
            .iter()
            .find(|d| d.message.contains("矛盾律"))
            .expect("应报矛盾律");
        assert_eq!(lint.severity, crate::analysis::SEV_WARNING);
        assert_eq!(&"a & !a"[lint.start..lint.end], "a & !a");
    }

    #[test]
    fn operand_reader_handles_parentheses() {
        let toks = crate::lex::tokens("(!a) & a");
        let left = read_operand(&toks, 0).expect("(!a) 应读出操作数");
        assert!(left.negated);
        assert_eq!(left.name, "a");
        assert_eq!(&"(!a) & a"[left.start..left.stop], "(!a)");
    }

    #[test]
    fn no_infinite_loop_on_unary_operator_between_operands() {
        // `a ! b`：一元算子夹在两个操作数之间——匹配到 `a` 后走的分支
        // 曾未推进下标，原地重扫死循环（每个调 analyze 的测试都被挂住）。
        let _ = law_lints(&crate::lex::tokens("a ! b"));
        let _ = analyze("a ! b");
        let _ = analyze("a => !a & a");
    }
}
