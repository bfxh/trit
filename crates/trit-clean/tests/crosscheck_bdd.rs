//! BDD × DPLL 交叉校验的集成用例：同一份编码，两条独立计数链必须同结论。

use trit_clean::{CmpOp, Column, Fill, Rule, Table, Ty, Value, completions, crosscheck};

fn table(columns: &[(&str, Ty)], rows: &[&[Value]]) -> Table {
    let mut t = Table::new(
        columns
            .iter()
            .map(|(name, ty)| Column::new(name, *ty))
            .collect(),
    );
    for row in rows {
        t.push_row(row.to_vec()).unwrap();
    }
    t
}

#[test]
fn bdd_matches_dpll_on_rule_matrix() {
    let t = table(
        &[("age", Ty::Int), ("tag", Ty::Text)],
        &[
            &[Value::Null, Value::Text("a".into())],
            &[Value::Int(2), Value::Text("b".into())],
            &[Value::Int(5), Value::Text("c".into())],
        ],
    );
    let rules = vec![
        Rule::Check {
            col: 0,
            op: CmpOp::Le,
            lit: Value::Int(4),
        },
        Rule::Unique { col: 0 },
        Rule::NotNull { col: 1 },
    ];
    let fills = vec![
        Fill::new(0, 0, vec![Value::Int(1), Value::Int(2), Value::Int(6)]),
        Fill::new(1, 1, vec![Value::Text("x".into())]),
    ];
    let exact = completions(&t, &rules, &fills).unwrap();
    assert_eq!(exact.total, 3);
    let report = crosscheck(&t, &rules, &fills).unwrap();
    assert_eq!(report.dpll, exact.consistent);
    assert_eq!(
        report.agrees(),
        Some(true),
        "BDD 与 DPLL 必须一致: {report:?}"
    );
    assert!(report.bdd_nodes.unwrap_or(0) > 0);
}

#[test]
fn bdd_matches_dpll_without_fills() {
    let t = table(&[("x", Ty::Int)], &[&[Value::Int(1)], &[Value::Int(1)]]);
    let rules = vec![Rule::Unique { col: 0 }];
    let report = crosscheck(&t, &rules, &[]).unwrap();
    assert_eq!(report.dpll, 0, "已确定的 UNIQUE 冲突没有一致完成");
    assert_eq!(report.agrees(), Some(true));
}

#[test]
fn bdd_matches_dpll_on_domain_restriction() {
    let t = table(&[("x", Ty::Int)], &[&[Value::Null]]);
    let rules = vec![Rule::Domain {
        col: 0,
        values: vec![Value::Int(1), Value::Int(2)],
    }];
    let fills = vec![Fill::new(
        0,
        0,
        vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
    )];
    let exact = completions(&t, &rules, &fills).unwrap();
    let report = crosscheck(&t, &rules, &fills).unwrap();
    assert_eq!(exact.consistent, 2);
    assert_eq!(report.agrees(), Some(true));
}
