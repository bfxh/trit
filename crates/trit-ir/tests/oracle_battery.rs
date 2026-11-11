//! 双 oracle 验证的批量用例：每种语义下，管道输出与原式在全部赋值上逐点一致。

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_eval::Assignment;
use trit_ir::Ir;
use trit_ir::passes::{ConstantFold, Cse, Pipeline, Simplify};

fn parsed(text: &str) -> Expr {
    Parser::parse(text).unwrap()
}

fn xor_expr() -> Expr {
    Expr::Xor(
        Box::new(Expr::Var("a".into())),
        Box::new(Expr::Var("b".into())),
    )
}

fn equiv_expr() -> Expr {
    Expr::Equiv(
        Box::new(Expr::And(
            Box::new(Expr::Var("a".into())),
            Box::new(Expr::Var("b".into())),
        )),
        Box::new(Expr::Var("c".into())),
    )
}

/// 两个表达式变量表的并集（排序、去重）——比较必须落在同一组赋值上。
fn union_vars(left: &Expr, right: &Expr) -> Vec<String> {
    let mut vars = left.variables();
    vars.extend(right.variables());
    vars.sort();
    vars.dedup();
    vars
}

/// 在给定变量表的全部 3^n 赋值上求值（变量表固定，保证两侧对齐）。
fn eval_on(expr: &Expr, vars: &[String], alg: Algebra) -> Vec<Ternary> {
    let total = 3usize.pow(vars.len() as u32);
    let mut out = Vec::with_capacity(total);
    for index in 0..total {
        let mut rest = index;
        let mut assignment = Assignment::new();
        for name in vars {
            let value = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
            assignment.set(name.as_str(), value);
        }
        out.push(trit_eval::eval(expr, &assignment, alg));
    }
    out
}

fn cases() -> Vec<Expr> {
    vec![
        parsed("(a & b) | (!c)"),
        parsed("a => (b => c)"),
        parsed("(!a | b) & (!b | a)"),
        parsed("(a & 1) | (b & X)"),
        xor_expr(),
        equiv_expr(),
    ]
}

#[test]
fn verified_pipeline_matches_original_on_all_assignments() {
    for expr in cases() {
        let ir = Ir::from_expr(&expr).unwrap();
        for alg in Algebra::ALL {
            let pipeline = Pipeline::default_pipeline(alg);
            let (out, _reports) = pipeline
                .run_verified(&ir)
                .unwrap_or_else(|err| panic!("alg={alg} 验证失败：{err}"));
            let out_expr = out.to_expr().unwrap();
            let vars = union_vars(&expr, &out_expr);
            let original = eval_on(&expr, &vars, alg);
            let optimized = eval_on(&out_expr, &vars, alg);
            assert_eq!(original, optimized, "alg={alg} 输出不等价");
            assert!(
                out.node_count() <= ir.node_count(),
                "alg={alg} 管道不应增加节点：{} -> {}",
                ir.node_count(),
                out.node_count()
            );
        }
    }
}

#[test]
fn pipeline_is_deterministic() {
    let expr = parsed("(a & b) | (a & b)");
    for alg in Algebra::ALL {
        let pipeline = Pipeline::default_pipeline(alg);
        let ir = Ir::from_expr(&expr).unwrap();
        let (first, reports_a) = pipeline.run_verified(&ir).unwrap();
        let (second, reports_b) = pipeline.run_verified(&ir).unwrap();
        assert_eq!(first, second);
        assert_eq!(reports_a, reports_b);
    }
}

#[test]
fn individual_passes_are_verified_too() {
    let expr = parsed("!(!a) & (b | 0)");
    let ir = Ir::from_expr(&expr).unwrap();
    for alg in Algebra::ALL {
        for pass_name in ["fold", "simplify", "cse"] {
            let pipeline = match pass_name {
                "fold" => Pipeline::new(alg).with(ConstantFold),
                "simplify" => Pipeline::new(alg).with(Simplify),
                _ => Pipeline::new(alg).with(Cse),
            };
            let (out, _) = pipeline
                .run_verified(&ir)
                .unwrap_or_else(|err| panic!("{pass_name} alg={alg}: {err}"));
            let out_expr = out.to_expr().unwrap();
            let vars = union_vars(&expr, &out_expr);
            assert_eq!(
                eval_on(&expr, &vars, alg),
                eval_on(&out_expr, &vars, alg),
                "{pass_name} alg={alg}"
            );
        }
    }
}

#[test]
fn ir_eval_matches_expression_eval_for_every_assignment() {
    let expr = parsed("(a | b) & (!c | a)");
    let ir = Ir::from_expr(&expr).unwrap();
    let vars = ["a", "b", "c"];
    for alg in Algebra::ALL {
        for index in 0..27 {
            let mut rest = index;
            let mut assignment = Assignment::new();
            for name in vars {
                let value = match rest % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                };
                rest /= 3;
                assignment.set(name, value);
            }
            assert_eq!(
                ir.eval(&assignment, alg),
                trit_eval::eval(&expr, &assignment, alg),
                "alg={alg} index={index}"
            );
        }
    }
}
