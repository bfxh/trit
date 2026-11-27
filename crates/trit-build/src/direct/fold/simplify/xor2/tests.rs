//! `Xor2` 常量律判据：oracle 是 `GateKind::XorClassic.eval` 的 3×3 表。
use super::super::simplify;
use crate::direct::emit_netlist;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_circuit::{GateKind, parse_verilog};
use trit_core::Algebra;
use trit_core::value::Ternary;
const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];
#[test]
fn classic_xor_table_supports_the_constant_laws() {
    for alg in ALGS {
        for value in VALUES {
            assert_eq!(
                GateKind::XorClassic.eval(&[value, Ternary::False], alg),
                value,
                "{alg:?}"
            );
            assert_eq!(
                GateKind::XorClassic.eval(&[value, Ternary::True], alg),
                alg.not(value),
                "{alg:?}"
            );
            assert_eq!(
                GateKind::XorClassic.eval(&[value, Ternary::Unknown], alg),
                Ternary::Unknown,
                "{alg:?}"
            );
        }
    }
}

#[test]
fn false_is_identity_and_unknown_absorbs() {
    for alg in ALGS {
        for ops in [
            vec![Op::Var(0), Op::Const(Ternary::False), Op::Xor2(0, 1)],
            vec![Op::Var(0), Op::Const(Ternary::False), Op::Xor2(1, 0)],
        ] {
            assert_eq!(fold(ops, alg), vec![Op::Var(0)], "{alg:?}");
        }
        for ops in [
            vec![Op::Var(0), Op::Const(Ternary::Unknown), Op::Xor2(0, 1)],
            vec![Op::Var(0), Op::Const(Ternary::Unknown), Op::Xor2(1, 0)],
        ] {
            assert_eq!(fold(ops, alg), vec![Op::Const(Ternary::Unknown)], "{alg:?}");
        }
    }
}

#[test]
fn true_turns_into_not_and_double_negation_cancels() {
    for alg in ALGS {
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::True), Op::Xor2(0, 1)],
                alg
            ),
            vec![Op::Var(0), Op::Not(0)],
            "{alg:?}"
        );
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::True), Op::Xor2(1, 0)],
                alg
            ),
            vec![Op::Var(0), Op::Not(0)],
            "{alg:?}"
        );
        assert_eq!(
            fold(
                vec![
                    Op::Var(0),
                    Op::Not(0),
                    Op::Const(Ternary::True),
                    Op::Xor2(1, 2)
                ],
                alg
            ),
            vec![Op::Var(0)],
            "{alg:?}"
        );
    }
}

#[test]
fn same_operand_stays_xor2() {
    for alg in ALGS {
        let out = fold(vec![Op::Var(0), Op::Var(0), Op::Xor2(0, 1)], alg);
        assert_eq!(out, vec![Op::Var(0), Op::Xor2(0, 0)], "{alg:?}");
    }
}

#[test]
fn netlist_xor_with_constants_does_not_register_xor2_table() {
    for design in [
        "module m(input a, output y);\n  assign y = a ^ 1'b0;\nendmodule\n",
        "module m(input a, output y);\n  assign y = a ^ 1'b1;\nendmodule\n",
    ] {
        let circuit = parse_verilog(design).expect("解析");
        let vars: Vec<String> = circuit
            .inputs
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        let c = emit_netlist(&circuit, "y", &vars, Algebra::Kleene).expect("生成");
        assert_eq!(
            c.matches("int t").count(),
            0,
            "单用输入与取反都并进根返回：{design}: {c}"
        );
        assert!(!c.contains("T["), "{design}: {c}");
        assert!(!c.contains("T_NOT"), "{design}: {c}");
        assert!(
            c.contains("?0:-") || c.contains("?0:v[0];"),
            "{design}: {c}"
        );
    }
}

#[test]
fn simplify_only_reaches_constant_when_one_operand_is_constant() {
    let ops = vec![Op::Var(0), Op::Var(1), Op::Xor2(0, 1)];
    assert_eq!(
        simplify(ops, Algebra::Kleene),
        vec![Op::Var(0), Op::Var(1), Op::Xor2(0, 1)]
    );
}
