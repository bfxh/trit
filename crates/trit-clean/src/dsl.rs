//! 规则 DSL（M0）：`NOT NULL` / `CHECK` / `DOMAIN` / `UNIQUE`。
//!
//! 文法（一行一条，`#` 之后是注释，空行忽略）：
//!
//! ```text
//! NOT NULL age
//! CHECK age >= 0
//! CHECK name <> "x"
//! DOMAIN dept_id IN 1,2,3
//! UNIQUE email
//! ```
//!
//! 字面量只有两种：整数与双引号文本（M0 不做转义）。`DOMAIN` 是 M0 对外键字面量集合的简化，
//! 语义是“该列取值必须落在给定集合里”；`NULL` 一律得到 UNKNOWN（SQL 口径）。

use std::cmp::Ordering;

use crate::value::{Table, Ty, Value};

/// `CHECK` 的比较符。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    /// `=` / `==`
    Eq,
    /// `!=` / `<>`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CmpOp {
    /// 把 `Ordering` 判成真假（调用方保证两侧非 `NULL`）。
    #[must_use]
    pub fn test(&self, ord: Ordering) -> bool {
        match self {
            Self::Eq => ord == Ordering::Equal,
            Self::Ne => ord != Ordering::Equal,
            Self::Lt => ord == Ordering::Less,
            Self::Le => ord != Ordering::Greater,
            Self::Gt => ord == Ordering::Greater,
            Self::Ge => ord != Ordering::Less,
        }
    }
}

/// 一条规则（列已解析成下标）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// 该列不允许 `NULL`。
    NotNull {
        /// 列下标。
        col: usize,
    },
    /// 单列与字面量的比较。
    Check {
        /// 列下标。
        col: usize,
        /// 比较符。
        op: CmpOp,
        /// 右操作数字面量。
        lit: Value,
    },
    /// 取值必须落在给定集合里（M0 的外键简化）。
    Domain {
        /// 列下标。
        col: usize,
        /// 允许的取值（非空、不含 `NULL`）。
        values: Vec<Value>,
    },
    /// 非空值不得重复（`NULL` 互不冲突，SQL 口径）。
    Unique {
        /// 列下标。
        col: usize,
    },
}

impl Rule {
    /// 规则作用的列。
    #[must_use]
    pub fn column(&self) -> usize {
        match self {
            Self::NotNull { col }
            | Self::Check { col, .. }
            | Self::Domain { col, .. }
            | Self::Unique { col } => *col,
        }
    }
}

/// 解析规则文本；出错带行号。
pub fn parse_rules(table: &Table, src: &str) -> Result<Vec<Rule>, RuleError> {
    let mut rules = Vec::new();
    for (idx, raw) in src.lines().enumerate() {
        let line = idx + 1;
        let text = raw.split('#').next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        rules.push(parse_line(table, line, text)?);
    }
    Ok(rules)
}

fn parse_line(table: &Table, line: usize, text: &str) -> Result<Rule, RuleError> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let keyword = tokens[0].to_ascii_uppercase();
    match keyword.as_str() {
        "NOT" => {
            if tokens.len() != 3 || !tokens[1].eq_ignore_ascii_case("NULL") {
                return Err(RuleError::Arity {
                    line,
                    rule: "NOT NULL".into(),
                    expected: "NOT NULL <列>".into(),
                });
            }
            Ok(Rule::NotNull {
                col: column(table, line, tokens[2])?,
            })
        }
        "CHECK" => {
            if tokens.len() != 4 {
                return Err(RuleError::Arity {
                    line,
                    rule: "CHECK".into(),
                    expected: "CHECK <列> <比较符> <字面量>".into(),
                });
            }
            let col = column(table, line, tokens[1])?;
            let op = parse_op(line, tokens[2])?;
            let lit = parse_literal(line, tokens[3])?;
            check_type(table, line, col, &lit)?;
            Ok(Rule::Check { col, op, lit })
        }
        "UNIQUE" => {
            if tokens.len() != 2 {
                return Err(RuleError::Arity {
                    line,
                    rule: "UNIQUE".into(),
                    expected: "UNIQUE <列>".into(),
                });
            }
            Ok(Rule::Unique {
                col: column(table, line, tokens[1])?,
            })
        }
        "DOMAIN" => {
            if tokens.len() < 4 || !tokens[2].eq_ignore_ascii_case("IN") {
                return Err(RuleError::Arity {
                    line,
                    rule: "DOMAIN".into(),
                    expected: "DOMAIN <列> IN <值>[,<值>...]".into(),
                });
            }
            let col = column(table, line, tokens[1])?;
            let joined = tokens[3..].join(" ");
            let mut values = Vec::new();
            for part in joined.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    return Err(RuleError::EmptyDomain { line });
                }
                let value = parse_literal(line, part)?;
                check_type(table, line, col, &value)?;
                values.push(value);
            }
            if values.is_empty() {
                return Err(RuleError::EmptyDomain { line });
            }
            Ok(Rule::Domain { col, values })
        }
        other => Err(RuleError::UnknownRule {
            line,
            keyword: other.to_string(),
        }),
    }
}

fn column(table: &Table, line: usize, name: &str) -> Result<usize, RuleError> {
    table
        .column_index(name)
        .ok_or_else(|| RuleError::UnknownColumn {
            line,
            name: name.to_string(),
        })
}

fn parse_op(line: usize, token: &str) -> Result<CmpOp, RuleError> {
    match token {
        "=" | "==" => Ok(CmpOp::Eq),
        "!=" | "<>" => Ok(CmpOp::Ne),
        "<" => Ok(CmpOp::Lt),
        "<=" => Ok(CmpOp::Le),
        ">" => Ok(CmpOp::Gt),
        ">=" => Ok(CmpOp::Ge),
        other => Err(RuleError::BadOp {
            line,
            op: other.to_string(),
        }),
    }
}

fn parse_literal(line: usize, token: &str) -> Result<Value, RuleError> {
    if let Some(inner) = token.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        return Ok(Value::Text(inner.to_string()));
    }
    token
        .parse::<i64>()
        .map(Value::Int)
        .map_err(|_| RuleError::BadLiteral {
            line,
            text: token.to_string(),
        })
}

fn check_type(table: &Table, line: usize, col: usize, lit: &Value) -> Result<(), RuleError> {
    let expected = table.columns[col].ty;
    let actual = lit.ty().unwrap_or(Ty::Text);
    if expected != actual {
        return Err(RuleError::TypeMismatch {
            line,
            name: table.columns[col].name.clone(),
            expected,
            actual,
        });
    }
    Ok(())
}

/// DSL 解析错误（带行号）。
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// 未知规则关键字。
    #[error("第 {line} 行：未知规则关键字 `{keyword}`（支持 NOT NULL / CHECK / DOMAIN / UNIQUE）")]
    UnknownRule {
        /// 行号（1 起）。
        line: usize,
        /// 关键字原文。
        keyword: String,
    },
    /// 未知列名。
    #[error("第 {line} 行：未知名 `{name}`（表里没有这一列）")]
    UnknownColumn {
        /// 行号（1 起）。
        line: usize,
        /// 列名原文。
        name: String,
    },
    /// 字面量无法解析。
    #[error("第 {line} 行：字面量 `{text}` 无法解析（整数或双引号文本）")]
    BadLiteral {
        /// 行号（1 起）。
        line: usize,
        /// 原文。
        text: String,
    },
    /// 字面量类型与列不符。
    #[error("第 {line} 行：列 `{name}` 是 {expected:?}，字面量是 {actual:?}")]
    TypeMismatch {
        /// 行号（1 起）。
        line: usize,
        /// 列名。
        name: String,
        /// 列类型。
        expected: Ty,
        /// 字面量类型。
        actual: Ty,
    },
    /// 参数个数不对。
    #[error("第 {line} 行：`{rule}` 参数不对（期望 {expected}）")]
    Arity {
        /// 行号（1 起）。
        line: usize,
        /// 规则名。
        rule: String,
        /// 期望写法。
        expected: String,
    },
    /// `DOMAIN` 列表为空。
    #[error("第 {line} 行：DOMAIN 列表为空")]
    EmptyDomain {
        /// 行号（1 起）。
        line: usize,
    },
    /// 未知比较符。
    #[error("第 {line} 行：未知比较符 `{op}`")]
    BadOp {
        /// 行号（1 起）。
        line: usize,
        /// 原文。
        op: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Column;

    fn table() -> Table {
        Table::new(vec![
            Column::new("age", Ty::Int),
            Column::new("name", Ty::Text),
            Column::new("dept", Ty::Int),
        ])
    }

    #[test]
    fn parses_all_rule_kinds() {
        let src = r#"
            # 注释与空行
            NOT NULL age
            CHECK age >= 0   # 行尾注释
            CHECK name <> "x"
            DOMAIN dept IN 1, 2,3
            UNIQUE name
        "#;
        let rules = parse_rules(&table(), src).unwrap();
        assert_eq!(rules.len(), 5, "{rules:?}");
        assert_eq!(
            rules[1],
            Rule::Check {
                col: 0,
                op: CmpOp::Ge,
                lit: Value::Int(0)
            }
        );
        assert_eq!(
            rules[3],
            Rule::Domain {
                col: 2,
                values: vec![Value::Int(1), Value::Int(2), Value::Int(3)]
            }
        );
        assert_eq!(rules[4], Rule::Unique { col: 1 });
    }

    #[test]
    fn rejects_unknown_column_with_line_number() {
        let err = parse_rules(&table(), "NOT NULL nope").unwrap_err();
        match err {
            RuleError::UnknownColumn { line, name } => {
                assert_eq!(line, 1);
                assert_eq!(name, "nope");
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn rejects_type_mismatch() {
        let err = parse_rules(&table(), r#"CHECK age = "x""#).unwrap_err();
        assert!(
            matches!(err, RuleError::TypeMismatch { line: 1, .. }),
            "{err}"
        );
    }

    #[test]
    fn rejects_bad_literals_and_ops() {
        assert!(matches!(
            parse_rules(&table(), "CHECK age < 1.5").unwrap_err(),
            RuleError::BadLiteral { .. }
        ));
        assert!(matches!(
            parse_rules(&table(), "CHECK age ~= 1").unwrap_err(),
            RuleError::BadOp { .. }
        ));
        assert!(matches!(
            parse_rules(&table(), "DOMAIN dept IN ").unwrap_err(),
            RuleError::Arity { .. } | RuleError::EmptyDomain { .. }
        ));
    }

    #[test]
    fn tolerates_double_equals_and_ne_aliases() {
        let rules = parse_rules(&table(), "CHECK age == 1\nCHECK age <> 2").unwrap();
        assert!(matches!(rules[0], Rule::Check { op: CmpOp::Eq, .. }));
        assert!(matches!(rules[1], Rule::Check { op: CmpOp::Ne, .. }));
    }
}
