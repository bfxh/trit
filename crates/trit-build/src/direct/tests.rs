//! `direct` 的语义与渲染判据：表对拍 `trit_eval`、IR 全赋值对拍、CSE、渲染形状。
use super::*;
use trit_core::Parser;
use trit_eval::eval::{Assignment, eval};

mod eval;
use eval::eval_ops;

/// 用 `trit_eval` 独立算 3×3 表：与生成物里的表逐格相同才算语义一致。
#[test]
fn generated_tables_match_eval() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar] {
        for (source, function) in [
            (
                "a & b",
                Algebra::and as fn(Algebra, Ternary, Ternary) -> Ternary,
            ),
            ("a | b", Algebra::or),
            ("a ^ b", Algebra::strong_add),
            ("a => b", Algebra::implies),
            ("a <=> b", Algebra::equiv),
        ] {
            let expr = Parser::parse(source).expect("解析");
            let vars = expr.variables();
            let table = binary_table(alg, function);
            for row in 0..9u64 {
                let mut assignment = Assignment::new();
                let mut rest = row;
                for var in &vars {
                    let digit = rest % 3;
                    rest /= 3;
                    assignment.set(var.clone(), VALUES[digit as usize]);
                }
                let expected = eval(&expr, &assignment, alg);
                assert_eq!(
                    table[(row % 3) as usize * 3 + (row / 3) as usize],
                    code_of(expected),
                    "{alg:?} {source} 在 a={} b={} 不一致",
                    code(VALUES[(row % 3) as usize]),
                    code(VALUES[(row / 3) as usize])
                );
            }
        }
    }
}

/// 三种代数的 `not` 在 -1/0/1 上都等价于算术取负；这是 C 侧写 `-x` 的依据。
#[test]
fn negation_is_unary_minus_in_every_algebra() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar] {
        for value in VALUES {
            assert_eq!(code(alg.not(value)), -code(value), "{alg:?} {value:?}");
        }
    }
}
/// IR 执行结果必须与 `trit_eval` 全赋值一致（3^n 穷举）。
#[test]
fn lowered_ir_matches_eval_for_every_assignment() {
    let sources = [
        "a & b | !c",
        "((a => b) <=> (!a | b)) ^ c",
        "(a ^ b) & (c | !d)",
        "1",
        "a",
    ];
    for source in sources {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let ops = lower(&expr, &vars).expect("降级");
        let count = 3u64.pow(vars.len() as u32);
        for alg in [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar] {
            for code_value in 0..count {
                let mut assignment = Assignment::new();
                let mut digits = Vec::new();
                let mut rest = code_value;
                for var in &vars {
                    let digit = rest % 3;
                    rest /= 3;
                    digits.push(VALUES[digit as usize]);
                    assignment.set(var.clone(), VALUES[digit as usize]);
                }
                assert_eq!(
                    eval_ops(&ops, &digits, alg),
                    eval(&expr, &assignment, alg),
                    "{alg:?} {source} code={code_value}"
                );
            }
        }
    }
}

/// 渲染出来的形状：函数签名/无 3^n 表/根节点直接返回。
#[test]
fn render_is_linear_not_a_truth_table() {
    let expr = Parser::parse("a & b | !c").expect("解析");
    let vars = expr.variables();
    let source = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert!(
        source.contains("int8_t trit_direct(const int8_t *v, unsigned n)"),
        "{source}"
    );
    assert!(source.contains("static const int8_t T[]"), "{source}");
    assert!(source.contains("-v["), "{source}");
    assert!(!source.contains("T_NOT"), "{source}");
    assert!(
        !source.contains("TRIT_TABLE"),
        "直算后端不该有 3^n 表：{source}"
    );
    assert!(source.contains("return T["), "{source}");
}

#[test]
fn variable_order_is_respected() {
    let expr = Parser::parse("b & a").expect("解析");
    let vars = vec!["a".to_string(), "b".to_string()];
    let source = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    // 后序：b 先取 v[1]，a 取 v[0]；两个单用输入并进根返回。
    assert!(source.contains("return T[3*v[1]+v[0]+4];"), "{source}");
    assert!(!source.contains("trit_bin("), "{source}");
}

#[test]
fn identical_subexpressions_are_computed_once() {
    let expr = Parser::parse("(a & b) | (a & b)").expect("解析");
    let vars = expr.variables();
    let ops = lower(&expr, &vars).expect("降级");
    assert_eq!(ops.len(), 4, "CSE 没生效：{ops:?}");
    let source = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert_eq!(
        source.matches("T[3*").count(),
        1,
        "同一个与式只该算一次：{source}"
    );
    assert!(!source.contains("trit_bin("), "{source}");
}

fn code_of(value: Ternary) -> i8 {
    code(value)
}
