//! 恒等式化简判据：三种代数 × 三个值穷举格律；形状上别名到操作数、吸收律落常量。
use super::*;
use crate::direct::emit;
use crate::direct::lower::lower;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};
mod absorb;
mod complement;
mod idem;
mod lattice;
mod poison;
const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];
#[test]
fn laws_hold_for_every_value_and_algebra() {
    for alg in ALGS {
        for value in VALUES {
            assert_eq!(alg.and(value, Ternary::True), value, "{alg:?}");
            assert_eq!(alg.or(value, Ternary::False), value, "{alg:?}");
            assert_eq!(alg.not(alg.not(value)), value, "{alg:?}");
            if !matches!(alg, Algebra::Bochvar) {
                assert_eq!(alg.and(value, Ternary::False), Ternary::False, "{alg:?}");
                assert_eq!(alg.or(value, Ternary::True), Ternary::True, "{alg:?}");
            }
        }
    }
    // 负结果：B3 的 U 传染，所以吸收律必须按代数分派。
    assert_eq!(
        Algebra::Bochvar.and(Ternary::Unknown, Ternary::False),
        Ternary::Unknown
    );
    assert_eq!(
        Algebra::Bochvar.or(Ternary::Unknown, Ternary::True),
        Ternary::Unknown
    );
}
/// 单位律与对合律：结果别名到操作数，IR 里不新增语句。
#[test]
fn identity_laws_alias_to_the_operand() {
    assert_eq!(simplified("a & 1"), vec![Op::Var(0)]);
    assert_eq!(simplified("a | 0"), vec![Op::Var(0)]);
    assert_eq!(simplified("!(!a)"), vec![Op::Var(0)]);
    assert_eq!(simplified("(a & 1) | 0"), vec![Op::Var(0)]);
}
/// 渲染面：恒等式化简后根节点直接返回变量，不落临时语句、不注册算子表。
#[test]
fn simplified_c_has_one_statement_and_no_tables() {
    for source in ["a & 1", "a | 0", "!(!a)"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
        assert_eq!(c.matches("int t").count(), 0, "{source}: {c}");
        assert!(!c.contains("C T[]"), "{source}: {c}");
        assert!(c.contains("?0:v[0];"), "{source}: {c}");
    }
}

fn lowered(source: &str) -> Vec<Op> {
    let expr = Parser::parse(source).expect("解析");
    let vars = expr.variables();
    lower(&expr, &vars).expect("降级")
}

fn simplified(source: &str) -> Vec<Op> {
    simplified_alg(source, Algebra::Kleene)
}

fn simplified_alg(source: &str, alg: Algebra) -> Vec<Op> {
    simplify(lowered(source), alg)
}
