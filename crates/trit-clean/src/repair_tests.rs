//! `repair` 的单元测试（从 repair.rs 拆出，控制单文件行数预算）。

use super::*;
use crate::dsl::CmpOp;
use crate::eval::evaluate;
use crate::value::{Column, Ty};

fn table(rows: Vec<Vec<Value>>) -> Table {
    let mut t = Table::new(vec![
        Column::new("age", Ty::Int),
        Column::new("tag", Ty::Text),
    ]);
    for r in rows {
        t.push_row(r).unwrap();
    }
    t
}

#[test]
fn completion_ratio_matches_manual_count() {
    let t = table(vec![vec![Value::Null, Value::Text("a".into())]]);
    let rules = vec![Rule::Check {
        col: 0,
        op: CmpOp::Ge,
        lit: Value::Int(2),
    }];
    let fill = Fill::new(0, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let got = completions(&t, &rules, std::slice::from_ref(&fill)).unwrap();
    assert_eq!(got.total, 3);
    assert_eq!(got.consistent, 2);
    assert!((got.ratio() - 2.0 / 3.0).abs() < 1e-12);
}

#[test]
fn completion_respects_not_null_and_domain() {
    let t = table(vec![vec![Value::Null, Value::Text("a".into())]]);
    let rules = vec![
        Rule::NotNull { col: 0 },
        Rule::Domain {
            col: 0,
            values: vec![Value::Int(1), Value::Int(2)],
        },
    ];
    let fill = Fill::new(0, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let got = completions(&t, &rules, std::slice::from_ref(&fill)).unwrap();
    assert_eq!(
        got,
        Completion {
            consistent: 2,
            total: 3
        }
    );

    let strict = vec![Rule::NotNull { col: 0 }];
    let t2 = table(vec![vec![Value::Null, Value::Text("a".into())]]);
    let got = completions(&t2, &strict, &[]).unwrap();
    assert_eq!(
        got,
        Completion {
            consistent: 0,
            total: 1
        }
    );
}

#[test]
fn unique_completion_forbids_equal_values() {
    let t = table(vec![
        vec![Value::Int(1), Value::Text("a".into())],
        vec![Value::Null, Value::Text("b".into())],
    ]);
    let rules = vec![Rule::Unique { col: 0 }];
    let fill = Fill::new(1, 0, vec![Value::Int(1), Value::Int(2)]);
    let got = completions(&t, &rules, std::slice::from_ref(&fill)).unwrap();
    assert_eq!(
        got,
        Completion {
            consistent: 1,
            total: 2
        }
    );
}

#[test]
fn too_many_variables_is_structured_error() {
    let t = table(vec![vec![Value::Null, Value::Text("a".into())]]);
    let rules = vec![];
    let fills: Vec<Fill> = vec![Fill::new(
        0,
        0,
        (0..=MAX_VARS as i64).map(Value::Int).collect(),
    )];
    let err = completions(&t, &rules, &fills).unwrap_err();
    assert!(matches!(err, RepairError::TooManyVariables { .. }), "{err}");
}

#[test]
fn minimal_repair_finds_first_distance() {
    let t = table(vec![
        vec![Value::Int(1), Value::Text("a".into())],
        vec![Value::Int(1), Value::Text("b".into())],
    ]);
    let rules = vec![Rule::Unique { col: 0 }];
    let got = minimal_repairs(
        &t,
        &rules,
        &[(1, 0)],
        &[(0, vec![Value::Int(1), Value::Int(2)])],
    )
    .unwrap();
    assert!(!got.consistent);
    assert_eq!(got.depth, 1);
    assert_eq!(got.count, 1);
}

#[test]
fn consistent_table_needs_no_repair() {
    let t = table(vec![vec![Value::Int(1), Value::Text("a".into())]]);
    let rules = vec![Rule::NotNull { col: 0 }];
    let got = minimal_repairs(&t, &rules, &[(0, 0)], &[]).unwrap();
    assert!(got.consistent);
    assert_eq!(got.depth, 0);
    assert_eq!(got.count, 1);
}

#[test]
fn repair_reports_no_repair_when_domain_too_small() {
    let t = table(vec![vec![Value::Int(1), Value::Text("a".into())]]);
    let rules = vec![Rule::Check {
        col: 0,
        op: CmpOp::Ge,
        lit: Value::Int(5),
    }];
    let got = minimal_repairs(
        &t,
        &rules,
        &[(0, 0)],
        &[(0, vec![Value::Int(1), Value::Int(2)])],
    )
    .unwrap();
    assert!(!got.consistent);
    assert_eq!(got.depth, 0);
    assert_eq!(got.count, 0);
}

#[test]
fn auto_scope_repairs_unique_conflict() {
    // [1, 1, 2]：改任一行的 1 为 3 都能解开冲突 ⇒ 最小修复深度 1、条数 2。
    let t = table(vec![
        vec![Value::Int(1), Value::Text("a".into())],
        vec![Value::Int(1), Value::Text("b".into())],
        vec![Value::Int(2), Value::Text("c".into())],
    ]);
    let rules = vec![Rule::Unique { col: 0 }];
    let domains = [(0usize, vec![Value::Int(1), Value::Int(2), Value::Int(3)])];
    let got = minimal_repairs_auto(&t, &rules, &domains).unwrap();
    assert!(!got.consistent);
    assert_eq!((got.depth, got.count), (1, 2), "got={got:?}");
}

#[test]
fn report_has_violation_detects_false_only() {
    let t = table(vec![vec![Value::Null, Value::Text("a".into())]]);
    let rules = vec![Rule::Check {
        col: 0,
        op: CmpOp::Ge,
        lit: Value::Int(0),
    }];
    assert!(!evaluate(&t, &rules).has_violation());
}
