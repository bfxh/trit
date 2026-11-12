//! 精确计数的独立复核：用暴力枚举对拍 `minimal_repairs` / `completions`。
//!
//! 暴力实现只依赖公开的 `evaluate`，与被测编码（CNF/DPLL）没有共享代码。

use trit_clean::{Column, Fill, Rule, Table, Ty, Value, completions, evaluate, minimal_repairs};

fn make_table(columns: Vec<(&str, Ty)>, rows: &[&[Value]]) -> Table {
    let mut table = Table::new(
        columns
            .into_iter()
            .map(|(name, ty)| Column::new(name, ty))
            .collect(),
    );
    for row in rows {
        table.push_row(row.to_vec()).unwrap();
    }
    table
}

fn domains_of(candidates: &[(usize, usize)], domains: &[(usize, Vec<Value>)]) -> Vec<Vec<Value>> {
    candidates
        .iter()
        .map(|(_, col)| {
            domains
                .iter()
                .find(|(c, _)| c == col)
                .map(|(_, d)| d.clone())
                .unwrap_or_else(|| panic!("候选列 {col} 没有域"))
        })
        .collect()
}

/// 暴力枚举：返回 (consistent, depth, count)，语义与 `minimal_repairs` 对齐。
fn brute_repairs(
    table: &Table,
    rules: &[Rule],
    candidates: &[(usize, usize)],
    domains: &[(usize, Vec<Value>)],
) -> (bool, usize, u128) {
    if !evaluate(table, rules).has_violation() {
        return (true, 0, 1);
    }
    let doms = domains_of(candidates, domains);
    let total: u128 = doms.iter().map(|d| d.len() as u128).product();
    let mut best = usize::MAX;
    let mut count = 0u128;
    for index in 0..total {
        let mut rest = index;
        let mut candidate = table.clone();
        let mut distance = 0usize;
        for (i, (row, col)) in candidates.iter().enumerate() {
            let domain = &doms[i];
            let pick = (rest % domain.len() as u128) as usize;
            rest /= domain.len() as u128;
            let value = &domain[pick];
            if &candidate.rows[*row][*col] != value {
                distance += 1;
            }
            candidate.rows[*row][*col] = value.clone();
        }
        if evaluate(&candidate, rules).has_violation() {
            continue;
        }
        if distance < best {
            best = distance;
            count = 1;
        } else if distance == best {
            count += 1;
        }
    }
    if best == usize::MAX {
        (false, 0, 0)
    } else {
        (false, best, count)
    }
}

#[test]
fn two_violations_need_two_changes() {
    let table = make_table(
        vec![("age", Ty::Int), ("tag", Ty::Text)],
        &[&[Value::Int(-5), Value::Null]],
    );
    let rules = vec![
        Rule::Check {
            col: 0,
            op: trit_clean::CmpOp::Ge,
            lit: Value::Int(0),
        },
        Rule::NotNull { col: 1 },
    ];
    let candidates = [(0usize, 0usize), (0, 1)];
    let domains = [
        (0usize, vec![Value::Int(-5), Value::Int(0), Value::Int(5)]),
        (1, vec![Value::Text("a".into()), Value::Text("b".into())]),
    ];
    let got = minimal_repairs(&table, &rules, &candidates, &domains).unwrap();
    let (consistent, depth, count) = brute_repairs(&table, &rules, &candidates, &domains);
    assert!(!consistent);
    assert_eq!((got.depth, got.count), (depth, count), "got={got:?}");
    assert_eq!((depth, count), (2, 4));
}

#[test]
fn unique_conflict_without_a_free_value_has_no_repair() {
    // [1, 1, 2] 且候选域只有 {1,2}：无论怎么改都还会撞上某个 1/2 ⇒ 无修复。
    let table = make_table(
        vec![("x", Ty::Int)],
        &[&[Value::Int(1)], &[Value::Int(1)], &[Value::Int(2)]],
    );
    let rules = vec![Rule::Unique { col: 0 }];
    let candidates = [(1usize, 0usize), (2, 0)];
    let domains = [(0usize, vec![Value::Int(1), Value::Int(2)])];
    let got = minimal_repairs(&table, &rules, &candidates, &domains).unwrap();
    let (consistent, depth, count) = brute_repairs(&table, &rules, &candidates, &domains);
    assert!(!consistent);
    assert_eq!((got.depth, got.count), (depth, count), "got={got:?}");
    assert_eq!((depth, count), (0, 0));
}

#[test]
fn unique_conflict_with_a_third_value_is_one_change() {
    let table = make_table(
        vec![("x", Ty::Int)],
        &[&[Value::Int(1)], &[Value::Int(1)], &[Value::Int(2)]],
    );
    let rules = vec![Rule::Unique { col: 0 }];
    let candidates = [(1usize, 0usize), (2, 0)];
    let domains = [(0usize, vec![Value::Int(1), Value::Int(2), Value::Int(3)])];
    let got = minimal_repairs(&table, &rules, &candidates, &domains).unwrap();
    let (consistent, depth, count) = brute_repairs(&table, &rules, &candidates, &domains);
    assert!(!consistent);
    assert_eq!((got.depth, got.count), (depth, count), "got={got:?}");
    assert_eq!((depth, count), (1, 1), "只有把 1 改成 3 一种修复");
}

#[test]
fn consistent_table_matches_brute_force() {
    let table = make_table(vec![("x", Ty::Int)], &[&[Value::Int(1)]]);
    let rules = vec![Rule::NotNull { col: 0 }];
    let got = minimal_repairs(
        &table,
        &rules,
        &[(0, 0)],
        &[(0, vec![Value::Int(1), Value::Int(2)])],
    )
    .unwrap();
    assert!(got.consistent);
    assert_eq!((got.depth, got.count), (0, 1));
}

#[test]
fn impossible_repair_matches_brute_force() {
    let table = make_table(vec![("x", Ty::Int)], &[&[Value::Int(1)]]);
    let rules = vec![Rule::Check {
        col: 0,
        op: trit_clean::CmpOp::Ge,
        lit: Value::Int(5),
    }];
    let got = minimal_repairs(
        &table,
        &rules,
        &[(0, 0)],
        &[(0, vec![Value::Int(1), Value::Int(2)])],
    )
    .unwrap();
    let (consistent, depth, count) = brute_repairs(
        &table,
        &rules,
        &[(0, 0)],
        &[(0, vec![Value::Int(1), Value::Int(2)])],
    );
    assert!(!consistent);
    assert_eq!((got.depth, got.count), (depth, count));
    assert_eq!((depth, count), (0, 0));
}

#[test]
fn completion_ratio_matches_brute_force() {
    let table = make_table(
        vec![("x", Ty::Int), ("y", Ty::Text)],
        &[
            &[Value::Int(1), Value::Null],
            &[Value::Null, Value::Text("b".into())],
        ],
    );
    let rules = vec![
        Rule::Unique { col: 0 },
        Rule::Check {
            col: 0,
            op: trit_clean::CmpOp::Le,
            lit: Value::Int(2),
        },
        Rule::Check {
            col: 1,
            op: trit_clean::CmpOp::Ne,
            lit: Value::Text("b".into()),
        },
    ];
    let fills = [
        Fill::new(0, 1, vec![Value::Text("a".into()), Value::Text("b".into())]),
        Fill::new(1, 0, vec![Value::Int(1), Value::Int(3)]),
    ];
    let got = completions(&table, &rules, &fills).unwrap();

    // 暴力：按 fills 枚举域乘积，逐组合复制表并求值。
    let mut consistent = 0u128;
    let total: u128 = fills.iter().map(|f| f.domain.len() as u128).product();
    for index in 0..total {
        let mut rest = index;
        let mut candidate = table.clone();
        for fill in &fills {
            let pick = (rest % fill.domain.len() as u128) as usize;
            rest /= fill.domain.len() as u128;
            candidate.rows[fill.row][fill.col] = fill.domain[pick].clone();
        }
        if !evaluate(&candidate, &rules).has_violation() {
            consistent += 1;
        }
    }
    assert_eq!(got, completions(&table, &rules, &fills).unwrap());
    assert_eq!(got.total, total);
    assert_eq!(got.consistent, consistent, "our={got:?}");
}
