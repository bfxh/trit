//! UNIQUE 列多待定格完成计数的暴力枚举对拍（issue #628 回归）。
//!
//! 暴力参照只依赖公开的 [`evaluate`]，与被测 CNF/DPLL 编码没有共享代码。

use trit_clean::{Column, Fill, Rule, Table, Ty, Value, completions, evaluate};

fn int_table(rows: &[Vec<i64>]) -> Table {
    let mut t = Table::new(vec![Column::new("a", Ty::Int)]);
    for row in rows {
        t.push_row(row.iter().map(|&v| Value::Int(v)).collect())
            .unwrap();
    }
    t
}

/// 暴力枚举 fills 域乘积，逐组合复制表求值，返回 (consistent, total)。
fn brute_completions(table: &Table, rules: &[Rule], fills: &[Fill]) -> (u128, u128) {
    let total: u128 = fills.iter().map(|f| f.domain.len() as u128).product();
    let mut consistent = 0u128;
    for index in 0..total {
        let mut rest = index;
        let mut candidate = table.clone();
        for fill in fills {
            let pick = (rest % fill.domain.len() as u128) as usize;
            rest /= fill.domain.len() as u128;
            candidate.rows[fill.row][fill.col] = fill.domain[pick].clone();
        }
        if !evaluate(&candidate, rules).has_violation() {
            consistent += 1;
        }
    }
    (consistent, total)
}

fn check(table: &Table, rules: &[Rule], fills: &[Fill], expect_consistent: u128) {
    let got = completions(table, rules, fills).unwrap();
    let (brute_consistent, brute_total) = brute_completions(table, rules, fills);
    assert_eq!(got.total, brute_total, "total 与暴力枚举不一致");
    assert_eq!(
        got.consistent, brute_consistent,
        "DPLL 计数与暴力枚举不一致: {got:?}"
    );
    assert_eq!(got.consistent, expect_consistent, "固定参照值不一致");
}

#[test]
fn two_free_cells_three_values_match_brute() {
    // 两条待定、值域 {1,2,3}：UNIQUE 下一致完成 = 3×2 = 6（有序互异）。
    let t = int_table(&[vec![1], vec![1]]);
    let rules = vec![Rule::Unique { col: 0 }];
    let fills = vec![
        Fill::new(0, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        Fill::new(1, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    ];
    check(&t, &rules, &fills, 6);
}

#[test]
fn three_free_cells_match_brute() {
    // 三条待定、值域 {1,2,3}：一致完成 = 3! = 6。
    let t = int_table(&[vec![1], vec![1], vec![1]]);
    let rules = vec![Rule::Unique { col: 0 }];
    let fills = vec![
        Fill::new(0, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        Fill::new(1, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        Fill::new(2, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    ];
    check(&t, &rules, &fills, 6);
}

#[test]
fn free_and_fixed_mixed_keeps_fixed_prohibition() {
    // 一条固定 1 + 两条待定：固定值仍禁取 1；两条待定只能在 {2,3} 间互异 ⇒ 2 种。
    // 若“已定值禁取”被放松，计数会变成 6，本用例守住这条正确约束。
    let t = int_table(&[vec![1], vec![1], vec![1]]);
    let rules = vec![Rule::Unique { col: 0 }];
    let fills = vec![
        Fill::new(1, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        Fill::new(2, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    ];
    check(&t, &rules, &fills, 2);
}

#[test]
fn fuzz178_replay_check_and_unique_matches_brute() {
    // 回放 fuzz round=178：rows=[0,2,2]，CHECK a≥0 + UNIQUE a，
    // fills r0∈{2,1}, r1∈{1,0}；一致完成仅 (1,0,2) 一种 ⇒ 1。
    let t = int_table(&[vec![0], vec![2], vec![2]]);
    let rules = vec![
        Rule::Check {
            col: 0,
            op: trit_clean::CmpOp::Ge,
            lit: Value::Int(0),
        },
        Rule::Unique { col: 0 },
    ];
    let fills = vec![
        Fill::new(0, 0, vec![Value::Int(2), Value::Int(1)]),
        Fill::new(1, 0, vec![Value::Int(1), Value::Int(0)]),
    ];
    check(&t, &rules, &fills, 1);
}
