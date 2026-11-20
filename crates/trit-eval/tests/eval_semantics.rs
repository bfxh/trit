//! 求值层的契约：位并行必须等于逐点求值，真值表行号必须能被解码回同一份赋值，
//! 化简必须保语义。
//!
//! 这三条任何一条错了，"结论来自穷举"这句话就不成立。

use std::collections::HashMap;

use trit_core::algebra::Algebra;
use trit_core::bits::TriBit;
use trit_core::limits::Limits;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_eval::eval::{
    Assignment, AssignmentCodecError, assignment_index, decode_assignment, equivalent,
    eval_bitwise, try_assignment_index, try_decode_assignment,
};
use trit_eval::{
    equivalent_within, eval, eval_within, simplify, simplify_with, truth_table, truth_table_within,
};

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

fn parse(src: &str) -> trit_core::ast::Expr {
    Parser::parse(src).expect("parse")
}

fn var_map(vars: &[String]) -> HashMap<String, usize> {
    vars.iter()
        .enumerate()
        .map(|(i, v)| (v.clone(), i))
        .collect()
}

/// 把 `index` 的每一位放进 `TriBit` 的同一个 bit 位置。
fn planes_for(vars: &[String], index: u64) -> Vec<TriBit> {
    let asg = decode_assignment(index, vars);
    vars.iter()
        .map(|name| match asg.get(name) {
            Some(Ternary::False) => TriBit::constant(Ternary::False),
            Some(Ternary::True) => TriBit::constant(Ternary::True),
            _ => TriBit::constant(Ternary::Unknown),
        })
        .collect()
}

// ---------- 位并行求值 ----------

#[test]
fn bitwise_evaluation_equals_pointwise_evaluation_on_every_assignment() {
    // 3 个变量 ⇒ 27 种赋值。位并行（每变量一张平面）必须与逐点 eval 逐位相同。
    let vars = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let map = var_map(&vars);
    for src in ["a & b | c", "!(a | b) & c", "(a ⇒ b) ⊕ c", "a ≡ (b & !c)"] {
        let expr = parse(src);
        for alg in ALGS {
            for index in 0..27u64 {
                let planes = planes_for(&vars, index);
                let bitwise = eval_bitwise(&expr, &planes, &map, alg)
                    .checked_get(0)
                    .expect("single bit");
                let asg = decode_assignment(index, &vars);
                let pointwise = eval(&expr, &asg, alg);
                assert_eq!(
                    bitwise, pointwise,
                    "ORCH_EVAL_BITWISE {src} {alg:?} index={index}"
                );
            }
        }
    }
}

#[test]
fn a_variable_missing_from_the_map_evaluates_to_unknown() {
    // 映射里没有的变量不是 False、也不是报错——它是"不知道"。
    let expr = parse("ghost");
    let map = HashMap::new();
    let planes = Vec::new();
    assert_eq!(
        eval_bitwise(&expr, &planes, &map, Algebra::Kleene)
            .checked_get(0)
            .expect("single bit"),
        Ternary::Unknown,
        "ORCH_EVAL_UNMAPPED"
    );
    // 常量不依赖平面。
    assert_eq!(
        eval_bitwise(&parse("1"), &[], &map, Algebra::Kleene)
            .checked_get(0)
            .expect("single bit"),
        Ternary::True
    );
}

#[test]
fn a_missing_plane_is_unknown_not_a_panic() {
    let vars = vec!["a".to_string()];
    let map = var_map(&vars);
    // 映射说有 a，但平面表是空的：必须退回 Unknown。
    assert_eq!(
        eval_bitwise(&parse("a"), &[], &map, Algebra::Kleene)
            .checked_get(0)
            .expect("single bit"),
        Ternary::Unknown,
        "ORCH_EVAL_MISSING_PLANE"
    );
}

// ---------- 真值表与赋值编码 ----------

#[test]
fn truth_table_row_index_decodes_to_the_same_assignment() {
    // 行号是三进制编码：False=0 / True=1 / Unknown=2。
    // 表里的第 i 行必须正好是 decode_assignment(i, vars) 那一份赋值。
    let vars = vec!["a".to_string(), "b".to_string()];
    for src in ["a & b", "a | b", "a ⊕ b"] {
        for alg in ALGS {
            let table = truth_table(&parse(src), alg).expect("table");
            assert_eq!(table.variables, vars);
            assert_eq!(table.values.len(), 9, "ORCH_EVAL_TABLE_ROWS");
            for (index, value) in table.values.iter().enumerate() {
                let asg = decode_assignment(index as u64, &vars);
                assert_eq!(
                    *value,
                    eval(&parse(src), &asg, alg),
                    "ORCH_EVAL_TABLE_ROW {src} {alg:?} row={index}"
                );
            }
        }
    }
}

#[test]
fn assignment_codec_is_a_bijection_over_the_whole_domain() {
    let vars = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    for index in 0..27u64 {
        let asg = decode_assignment(index, &vars);
        assert_eq!(
            assignment_index(&asg, &vars),
            Some(index),
            "ORCH_EVAL_CODEC {index}"
        );
        assert_eq!(try_decode_assignment(index, &vars).expect("in domain"), asg);
    }
}

#[test]
fn the_codec_refuses_duplicates_and_out_of_domain_values() {
    let dup = vec!["a".to_string(), "a".to_string()];
    let mut asg = Assignment::new();
    asg.set("a", Ternary::True);
    assert!(matches!(
        try_assignment_index(&asg, &dup),
        Err(AssignmentCodecError::DuplicateVariable(_))
    ));
    assert!(matches!(
        try_decode_assignment(3, &dup),
        Err(AssignmentCodecError::DuplicateVariable(_))
    ));
    assert!(
        matches!(
            try_decode_assignment(100, &["a".to_string()]),
            Err(AssignmentCodecError::OutOfDomain { .. })
        ),
        "ORCH_EVAL_OUT_OF_DOMAIN"
    );
    // 便捷包装在错误时给 None，而不是 0（0 是合法编号）。
    assert_eq!(assignment_index(&asg, &dup), None);
}

#[test]
fn missing_bindings_are_encoded_as_unknown() {
    // 编码器把未绑定变量当 Unknown（digit=2），与 eval 的口径一致。
    let vars = vec!["a".to_string()];
    let asg = Assignment::new();
    assert_eq!(assignment_index(&asg, &vars), Some(2), "ORCH_EVAL_UNBOUND");
    assert_eq!(decode_assignment(2, &vars).get("a"), Some(Ternary::Unknown));
}

#[test]
fn assignment_set_overwrites_in_place_and_keeps_order() {
    let mut asg = Assignment::new();
    asg.set("b", Ternary::True)
        .set("a", Ternary::False)
        .set("b", Ternary::Unknown);
    assert_eq!(asg.len(), 2);
    assert_eq!(
        asg.variables(),
        vec!["b".to_string(), "a".to_string()],
        "重复 set 不该追加"
    );
    assert_eq!(asg.get("b"), Some(Ternary::Unknown));
    assert_eq!(asg.get("missing"), None);
    assert!(!asg.is_empty());
}

// ---------- 预算 ----------

#[test]
fn budgets_are_enforced_before_enumeration() {
    let expr = parse("a & b");
    let tight_vars = Limits {
        max_vars: 1,
        ..Limits::default()
    };
    assert!(truth_table_within(&expr, Algebra::Kleene, tight_vars).is_err());
    assert!(equivalent_within(&expr, &parse("b & a"), Algebra::Kleene, tight_vars).is_err());

    let tight_rows = Limits {
        max_rows: 8,
        ..Limits::default()
    };
    assert!(
        truth_table_within(&expr, Algebra::Kleene, tight_rows).is_err(),
        "9 行超过 8"
    );
    assert!(
        eval_within(
            &expr,
            &Assignment::new(),
            Algebra::Kleene,
            Limits::default()
        )
        .is_ok()
    );
}

// ---------- 等价判定 ----------

#[test]
fn equivalence_uses_the_union_of_variables_and_reports_a_counterexample() {
    let left = parse("a & b");
    let right = parse("c");
    let result = equivalent(&left, &right, Algebra::Kleene).expect("ok");
    assert!(!result.equivalent);
    assert_eq!(result.variable_count, 3, "ORCH_EVAL_UNION_VARS");
    let counterexample = result.counterexample_assignment.expect("must have one");
    assert_eq!(counterexample.len(), 3, "反例要覆盖全部变量");
    assert_ne!(
        eval(&left, &counterexample, Algebra::Kleene),
        eval(&right, &counterexample, Algebra::Kleene),
        "反例必须真的让两侧不同"
    );
}

#[test]
fn equivalent_expressions_have_no_counterexample() {
    let result = equivalent(&parse("a & b"), &parse("b & a"), Algebra::Kleene).expect("ok");
    assert!(result.equivalent, "ORCH_EVAL_EQUIV");
    assert!(result.counterexample_assignment.is_none());
}

// ---------- 化简必须保语义 ----------

#[test]
fn simplification_preserves_the_truth_table() {
    // 化简是重写：只要有一处不保语义，后面所有"化简后再判"的结论都会错。
    let sources = [
        "a & 1",
        "a | 0",
        "a & !a",
        "a | !a",
        "!!a",
        "a ⇒ b",
        "(a ⇒ b) & (b ⇒ c)",
        "a ⊕ a",
        "!(a | b)",
        "0 & (a | b)",
    ];
    // 不能直接比两张表的 values：表的行号由"变量出现顺序"决定，而重写会改变顺序
    // （De Morgan 展开、操作数交换）。所以逐点比语义，再单独约束变量集。
    for src in sources {
        for alg in ALGS {
            let original = parse(src);
            let simplified = simplify(&original, alg);
            let before = truth_table(&original, alg).expect("before");
            let vars = before.variables.clone();
            let after_vars = truth_table(&simplified, alg).expect("after").variables;
            for name in &after_vars {
                assert!(
                    vars.contains(name),
                    "化简不能引入新变量：{src} {alg:?} 引入 {name}"
                );
            }
            for index in 0..3usize.pow(vars.len() as u32) {
                let asg = decode_assignment(index as u64, &vars);
                assert_eq!(
                    eval(&original, &asg, alg),
                    eval(&simplified, &asg, alg),
                    "ORCH_EVAL_SIMPLIFY_SEMANTIC {src} {alg:?} -> {} at {}",
                    simplified.to_infix(),
                    index
                );
            }
        }
    }
}

#[test]
fn constant_fold_switch_has_a_narrow_reachable_surface() {
    // 三值里"恒常量"很难出现：a & !a 在 a=X 时是 X、a | !a 在 a=X 时也是 X，
    // 所以 3^n 常量检测对含变量的式子几乎永远返回 None。
    // 这不是缺陷，但它意味着这个开关的实际作用面很窄——记录下来，
    // 免得有人把它当成强优化来依赖。
    use trit_eval::simplify::constant_value;
    for alg in ALGS {
        assert_eq!(
            constant_value(&parse("a & !a"), alg),
            None,
            "ORCH_EVAL_NO_EXCLUDED_MIDDLE {alg:?}：矛盾律在三值下不成立，不是常量"
        );
        assert_eq!(
            constant_value(&parse("a | !a"), alg),
            None,
            "ORCH_EVAL_NO_EXCLUDED_MIDDLE {alg:?}：排中律在三值下不成立，不是常量"
        );
    }
    // 无变量的式子才是常量检测真正的落点。
    assert_eq!(
        constant_value(&parse("1 & !0"), Algebra::Kleene),
        Some(Ternary::True)
    );
    // 吸收律 a|1 → 1 属于局部重写，与 fold_constants 无关。
    let opts = trit_eval::SimplifyOptions {
        fold_constants: false,
        ..Default::default()
    };
    assert_eq!(
        simplify_with(&parse("a | 1"), Algebra::Kleene, opts).to_infix(),
        "1"
    );
}

#[test]
fn constant_folding_skips_wide_expressions() {
    // 变量数超过阈值就不做 3^n 常量检测：阈值以下是 3^n，以上必须放弃。
    let wide = parse("a | b | c | d | e | f | g | h | i");
    let opts = trit_eval::SimplifyOptions {
        max_vars_for_constant_fold: 8,
        ..Default::default()
    };
    let result = simplify_with(&wide, Algebra::Kleene, opts);
    assert!(
        !matches!(result, trit_core::ast::Expr::Const(_)),
        "ORCH_EVAL_FOLD_THRESHOLD"
    );
}

#[test]
fn expand_implies_changes_shape_but_not_meaning() {
    let expr = parse("a ⇒ b");
    let opts = trit_eval::SimplifyOptions {
        expand_implies: true,
        fold_constants: false,
        ..Default::default()
    };
    let expanded = simplify_with(&expr, Algebra::Kleene, opts);
    // 展开会改变变量出现顺序，所以按语义判定（union 变量）而不是按表逐行比。
    let result = equivalent(&expr, &expanded, Algebra::Kleene).expect("ok");
    assert!(
        result.equivalent,
        "ORCH_EVAL_EXPAND_IMPLIES {}",
        expanded.to_infix()
    );
    assert_eq!(result.variable_count, 2);
}

#[test]
fn semantic_fingerprints_are_stable_but_not_evidence_of_equivalence() {
    // 指纹只用于缓存预筛选：同表达式稳定，但"指纹不同"不代表不等价
    // （变量出现顺序进出哈希）。
    let a = parse("a & b");
    let first = trit_eval::simplify::semantic_fingerprint(&a, Algebra::Kleene);
    let second = trit_eval::simplify::semantic_fingerprint(&parse("a & b"), Algebra::Kleene);
    assert_eq!(first, second, "同一式子指纹必须稳定");

    let swapped = trit_eval::simplify::semantic_fingerprint(&parse("b & a"), Algebra::Kleene);
    assert_ne!(first, swapped, "变量顺序参与指纹");
    assert!(
        equivalent(&a, &parse("b & a"), Algebra::Kleene)
            .expect("ok")
            .equivalent,
        "指纹不同却等价——所以指纹不能当等价证据：ORCH_EVAL_FINGERPRINT"
    );
}

#[test]
fn primitive_constants_are_recognized_without_enumeration() {
    assert_eq!(
        trit_eval::simplify::constant_value(&parse("1"), Algebra::Kleene),
        Some(Ternary::True)
    );
    assert_eq!(
        trit_eval::simplify::constant_value(&parse("0"), Algebra::Kleene),
        Some(Ternary::False)
    );
    assert_eq!(
        trit_eval::simplify::constant_value(&parse("X"), Algebra::Kleene),
        Some(Ternary::Unknown)
    );
    assert_eq!(
        trit_eval::simplify::constant_value(&parse("a"), Algebra::Kleene),
        None,
        "非常量必须给 None，不能猜"
    );
}

#[test]
fn simplify_and_eval_agree_on_a_fixed_assignment() {
    // 最后一道交叉验证：化简后的式子在同一赋值上给同一个值。
    let sources = ["a & (b | 1)", "!(a & !a)", "(a ⇒ a) & b"];
    let vars = vec!["a".to_string(), "b".to_string()];
    for src in sources {
        for alg in ALGS {
            let simplified = simplify(&parse(src), alg);
            for index in 0..9u64 {
                let asg = decode_assignment(index, &vars);
                assert_eq!(
                    eval(&parse(src), &asg, alg),
                    eval(&simplified, &asg, alg),
                    "ORCH_EVAL_SIMPLIFY_EVAL {src} {alg:?} index={index}"
                );
            }
        }
    }
}
