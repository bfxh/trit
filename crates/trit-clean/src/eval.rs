//! 三值求值（M0）：每行规则逐格判定，`UNIQUE` 是表级判定。
//!
//! 口径与 SQL 三值逻辑一致：`NULL` 参与比较得到 UNKNOWN；`CHECK` 只在结果为 False 时算违反
//! （UNKNOWN 放行）；`UNIQUE` 里 `NULL` 互不冲突。

use std::collections::HashMap;

use trit_core::value::Ternary;

use crate::dsl::Rule;
use crate::value::{Table, Value};

/// 单行规则的判定（`UNIQUE` 不适用，传进来会得到 UNKNOWN）。
#[must_use]
pub fn eval_row_rule(rule: &Rule, row: &[Value]) -> Ternary {
    match rule {
        Rule::NotNull { col } => match &row[*col] {
            Value::Null => Ternary::False,
            _ => Ternary::True,
        },
        Rule::Check { col, op, lit } => match row[*col].compare(lit) {
            None => Ternary::Unknown,
            Some(ord) => bool3(op.test(ord)),
        },
        Rule::Domain { col, values } => match &row[*col] {
            Value::Null => Ternary::Unknown,
            value => bool3(values.contains(value)),
        },
        Rule::Unique { .. } => Ternary::Unknown,
    }
}

/// 表级 `UNIQUE`：非空值重复 ⇒ False；否则 True（`NULL` 互不冲突）。
#[must_use]
pub fn eval_unique(table: &Table, col: usize) -> Ternary {
    let mut seen: HashMap<&Value, usize> = HashMap::new();
    for row in &table.rows {
        let value = &row[col];
        if value.is_null() {
            continue;
        }
        let count = seen.entry(value).or_insert(0);
        *count += 1;
        if *count > 1 {
            return Ternary::False;
        }
    }
    Ternary::True
}

fn bool3(value: bool) -> Ternary {
    if value { Ternary::True } else { Ternary::False }
}

/// 一条“规则 × 行”的判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeState {
    /// 规则下标。
    pub rule: usize,
    /// 行下标。
    pub row: usize,
    /// 三值结果。
    pub state: Ternary,
}

/// 一条表级规则的判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableState {
    /// 规则下标。
    pub rule: usize,
    /// 三值结果。
    pub state: Ternary,
}

/// 求值报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// 行级判定。
    pub rows: Vec<NodeState>,
    /// 表级判定（`UNIQUE`）。
    pub tables: Vec<TableState>,
}

/// 三值计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// 检查总数。
    pub checks: usize,
    /// True 的个数。
    pub yes: usize,
    /// False 的个数。
    pub no: usize,
    /// Unknown 的个数。
    pub unknown: usize,
}

impl Report {
    /// 计数。
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for state in self
            .rows
            .iter()
            .map(|n| n.state)
            .chain(self.tables.iter().map(|t| t.state))
        {
            counts.checks += 1;
            match state {
                Ternary::True => counts.yes += 1,
                Ternary::False => counts.no += 1,
                Ternary::Unknown => counts.unknown += 1,
            }
        }
        counts
    }

    /// 三值分数：True=1，Unknown=0.5（显式约定），False=0；没有检查时定义为 1.0。
    ///
    /// 这个 0.5 是**约定**而不是概率：它表示“证据不足”，与计数一起给出，
    /// 需要更细的口径时直接读 [`Report::counts`]。
    #[must_use]
    pub fn score(&self) -> f64 {
        let counts = self.counts();
        if counts.checks == 0 {
            return 1.0;
        }
        let weighted = counts.yes as f64 + 0.5 * counts.unknown as f64;
        weighted / counts.checks as f64
    }

    /// 是否存在 False（确定性违反）。
    #[must_use]
    pub fn has_violation(&self) -> bool {
        self.tables.iter().any(|t| t.state == Ternary::False)
            || self.rows.iter().any(|n| n.state == Ternary::False)
    }
}

/// 对整表求值。
#[must_use]
pub fn evaluate(table: &Table, rules: &[Rule]) -> Report {
    let mut report = Report::default();
    for (rule_idx, rule) in rules.iter().enumerate() {
        match rule {
            Rule::Unique { col } => report.tables.push(TableState {
                rule: rule_idx,
                state: eval_unique(table, *col),
            }),
            _ => {
                for (row_idx, row) in table.rows.iter().enumerate() {
                    report.rows.push(NodeState {
                        rule: rule_idx,
                        row: row_idx,
                        state: eval_row_rule(rule, row),
                    });
                }
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::CmpOp;
    use crate::value::{Column, Ty};

    fn row(values: &[Value]) -> Vec<Value> {
        values.to_vec()
    }

    fn table(rows: Vec<Vec<Value>>) -> Table {
        let mut t = Table::new(vec![
            Column::new("age", Ty::Int),
            Column::new("name", Ty::Text),
        ]);
        for r in rows {
            t.push_row(r).unwrap();
        }
        t
    }

    #[test]
    fn not_null_is_false_on_null() {
        let rule = Rule::NotNull { col: 0 };
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Null])), Ternary::False);
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Int(1)])), Ternary::True);
    }

    #[test]
    fn check_null_is_unknown() {
        let rule = Rule::Check {
            col: 0,
            op: CmpOp::Ge,
            lit: Value::Int(0),
        };
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Null])), Ternary::Unknown);
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Int(0)])), Ternary::True);
        assert_eq!(
            eval_row_rule(&rule, &row(&[Value::Int(-1)])),
            Ternary::False
        );
    }

    #[test]
    fn domain_null_is_unknown_and_membership_is_true() {
        let rule = Rule::Domain {
            col: 0,
            values: vec![Value::Int(1), Value::Int(2)],
        };
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Null])), Ternary::Unknown);
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Int(2)])), Ternary::True);
        assert_eq!(eval_row_rule(&rule, &row(&[Value::Int(3)])), Ternary::False);
    }

    #[test]
    fn unique_ignores_nulls_but_flags_duplicates() {
        let t = table(vec![
            row(&[Value::Null, Value::Text("a".into())]),
            row(&[Value::Null, Value::Text("b".into())]),
        ]);
        assert_eq!(eval_unique(&t, 0), Ternary::True);

        let t = table(vec![
            row(&[Value::Int(1), Value::Text("a".into())]),
            row(&[Value::Int(1), Value::Text("b".into())]),
        ]);
        assert_eq!(eval_unique(&t, 0), Ternary::False);
    }

    #[test]
    fn score_counts_unknown_as_half() {
        let t = table(vec![
            row(&[Value::Int(1), Value::Null]),
            row(&[Value::Int(2), Value::Null]),
        ]);
        let rules = vec![Rule::Check {
            col: 0,
            op: CmpOp::Ge,
            lit: Value::Int(0),
        }];
        let report = evaluate(&t, &rules);
        assert_eq!(
            report.counts(),
            Counts {
                checks: 2,
                yes: 2,
                no: 0,
                unknown: 0
            }
        );
        assert!((report.score() - 1.0).abs() < f64::EPSILON);

        let rules = vec![Rule::Check {
            col: 1,
            op: CmpOp::Eq,
            lit: Value::Text("x".into()),
        }];
        let report = evaluate(&t, &rules);
        assert_eq!(
            report.counts(),
            Counts {
                checks: 2,
                yes: 0,
                no: 0,
                unknown: 2
            }
        );
        assert!((report.score() - 0.5).abs() < f64::EPSILON);
    }
}
