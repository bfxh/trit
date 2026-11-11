//! 精确计数（B）：把“未知格填什么 / 哪几格改掉”编码成原始 CNF，用 `trit-count` 的 DPLL 计数。
//!
//! 两条能力：
//! - [`completions`]：给定若干“待定格”和各自的候选域，数**一致完成**与总数 —— 清洁度分数的分母/分子。
//! - [`minimal_repairs`]：按 Hamming 距离从小到大枚举改动集，数出**最小修复的条数**
//!   （每条修复是一种具体的赋值；距离相同才叫“最小”）。
//!
//! 口径：候选域只含非 `NULL` 值（修复不会把格子改成 `NULL`）；`CHECK`/`DOMAIN` 的 UNKNOWN 视为放行
//! （与 SQL 一致）；`UNIQUE` 里 `NULL` 互不冲突。
//! 精确计数最坏指数级：变量数、候选格数都有硬上限，超了返回结构化错误（**不许静默降级**）。

use std::collections::{HashMap, HashSet};

use trit_count::cnf::Cnf;

use crate::dsl::Rule;
use crate::eval::evaluate;
use crate::value::{Table, Value};

/// 精确计数的变量数上限（`trit-count` 在 128 个自由变量处会 panic，这里提前挡住）。
pub const MAX_VARS: usize = 120;

/// 最小修复枚举的候选格上限（2^n 子集）。
pub const MAX_CELLS: usize = 10;

/// 一个待定 / 可改的格子：`domain` 是允许取的值（非空、不含 `NULL`、不重复）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    /// 行下标。
    pub row: usize,
    /// 列下标。
    pub col: usize,
    /// 允许取的值。
    pub domain: Vec<Value>,
}

impl Fill {
    /// 新建一个填充格（不做校验，`completions` 会统一校验）。
    #[must_use]
    pub fn new(row: usize, col: usize, domain: Vec<Value>) -> Self {
        Self { row, col, domain }
    }
}

/// 一致完成计数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    /// 一致的完成数。
    pub consistent: u128,
    /// 全部完成数（各域大小的乘积）。
    pub total: u128,
}

impl Completion {
    /// 一致比例；`total == 0`（没有待定格）时定义为 1.0。
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.consistent as f64 / self.total as f64
    }
}

/// 最小修复计数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepairCount {
    /// 原表是否已经一致（True 时 `depth == 0, count == 1`，即“什么都不改”这一种修复）。
    pub consistent: bool,
    /// 最小改动格数；`0` 表示没有找到修复（域太小）。
    pub depth: usize,
    /// 最小修复的条数。
    pub count: u128,
    /// 实际尝试过的改动集个数（可复算的搜索量）。
    pub subsets_tried: u64,
}

/// 计数相关错误。
#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    /// 行列越界。
    #[error("格子越界：第 {row} 行第 {col} 列")]
    OutOfRange {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 域为空。
    #[error("域为空：第 {row} 行第 {col} 列")]
    EmptyDomain {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 域里含 `NULL`。
    #[error("域里不允许 NULL：第 {row} 行第 {col} 列（修复只能填非空值）")]
    NullInDomain {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 域值类型与列不符。
    #[error("域值类型与列不符：第 {row} 行第 {col} 列")]
    DomainType {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 同一个格子被重复声明。
    #[error("同一个格子被重复声明：第 {row} 行第 {col} 列")]
    DuplicateCell {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 域里有重复值（会让同一取值被数两次）。
    #[error("域里有重复值：第 {row} 行第 {col} 列")]
    DuplicateValue {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 候选格没有对应的域。
    #[error("候选格没有域：第 {row} 行第 {col} 列")]
    MissingDomain {
        /// 行下标。
        row: usize,
        /// 列下标。
        col: usize,
    },
    /// 变量数超上限。
    #[error("算不动：变量 {vars} 个超过上限 {limit}（精确计数是指数级，请缩小填充集）")]
    TooManyVariables {
        /// 变量数。
        vars: usize,
        /// 上限。
        limit: usize,
    },
    /// 候选格数超上限。
    #[error("算不动：候选格 {cells} 个超过上限 {limit}")]
    TooManyCells {
        /// 候选格数。
        cells: usize,
        /// 上限。
        limit: usize,
    },
    /// 计数溢出。
    #[error("算不动：计数超出 u128")]
    Overflow,
    /// 编码不自洽（计数大于总完成数）：这是 bug，必须响亮失败。
    #[error("内部不自洽：计数 {counted} > 总完成数 {total}（CNF 编码有 bug，请报告）")]
    EncodingBroken {
        /// DPLL 计数。
        counted: u128,
        /// 域大小乘积。
        total: u128,
    },
}

/// 数一致完成。
pub fn completions(
    table: &Table,
    rules: &[Rule],
    fills: &[Fill],
) -> Result<Completion, RepairError> {
    validate_fills(table, fills)?;
    let mut plan = Plan::build(table, fills)?;
    let total = plan.total()?;
    plan.encode(rules);
    let count = trit_count::dpll::count_cnf(&plan.cnf).count;
    if count > total {
        return Err(RepairError::EncodingBroken {
            counted: count,
            total,
        });
    }
    Ok(Completion {
        consistent: count,
        total,
    })
}

/// 数最小修复（Hamming 距离从小到大；距离相同的全部计数）。
pub fn minimal_repairs(
    table: &Table,
    rules: &[Rule],
    candidates: &[(usize, usize)],
    domains: &[(usize, Vec<Value>)],
) -> Result<RepairCount, RepairError> {
    if candidates.len() > MAX_CELLS {
        return Err(RepairError::TooManyCells {
            cells: candidates.len(),
            limit: MAX_CELLS,
        });
    }
    let report = evaluate(table, rules);
    if !report.has_violation() {
        return Ok(RepairCount {
            consistent: true,
            depth: 0,
            count: 1,
            subsets_tried: 0,
        });
    }
    let mut seen = HashSet::new();
    let mut changes: Vec<(usize, usize, Vec<Value>)> = Vec::new();
    for &(row, col) in candidates {
        if !seen.insert((row, col)) {
            return Err(RepairError::DuplicateCell { row, col });
        }
        let domain = domains
            .iter()
            .find(|(c, _)| *c == col)
            .map(|(_, d)| d.clone())
            .ok_or(RepairError::MissingDomain { row, col })?;
        let original = table
            .rows
            .get(row)
            .and_then(|r| r.get(col))
            .ok_or(RepairError::OutOfRange { row, col })?;
        let change_domain: Vec<Value> = domain.into_iter().filter(|v| v != original).collect();
        if !change_domain.is_empty() {
            changes.push((row, col, change_domain));
        }
    }
    let n = changes.len();
    if n == 0 {
        return Ok(RepairCount {
            consistent: false,
            depth: 0,
            count: 0,
            subsets_tried: 0,
        });
    }
    let mut subsets_tried = 0u64;
    for depth in 1..=n {
        let mut total = 0u128;
        for mask in 1u32..(1u32 << n) {
            if mask.count_ones() as usize != depth {
                continue;
            }
            subsets_tried += 1;
            let fills: Vec<Fill> = (0..n)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| {
                    let (row, col, ref domain) = changes[i];
                    Fill::new(row, col, domain.clone())
                })
                .collect();
            let completion = completions(table, rules, &fills)?;
            total = total
                .checked_add(completion.consistent)
                .ok_or(RepairError::Overflow)?;
        }
        if total > 0 {
            return Ok(RepairCount {
                consistent: false,
                depth,
                count: total,
                subsets_tried,
            });
        }
    }
    Ok(RepairCount {
        consistent: false,
        depth: 0,
        count: 0,
        subsets_tried,
    })
}

fn validate_fills(table: &Table, fills: &[Fill]) -> Result<(), RepairError> {
    let mut seen = HashSet::new();
    for fill in fills {
        table.rows.get(fill.row).ok_or(RepairError::OutOfRange {
            row: fill.row,
            col: fill.col,
        })?;
        let column = table.columns.get(fill.col).ok_or(RepairError::OutOfRange {
            row: fill.row,
            col: fill.col,
        })?;
        if !seen.insert((fill.row, fill.col)) {
            return Err(RepairError::DuplicateCell {
                row: fill.row,
                col: fill.col,
            });
        }
        if fill.domain.is_empty() {
            return Err(RepairError::EmptyDomain {
                row: fill.row,
                col: fill.col,
            });
        }
        let mut distinct = HashSet::new();
        for value in &fill.domain {
            if value.is_null() {
                return Err(RepairError::NullInDomain {
                    row: fill.row,
                    col: fill.col,
                });
            }
            if value.ty() != Some(column.ty) {
                return Err(RepairError::DomainType {
                    row: fill.row,
                    col: fill.col,
                });
            }
            if !distinct.insert(value) {
                return Err(RepairError::DuplicateValue {
                    row: fill.row,
                    col: fill.col,
                });
            }
        }
    }
    Ok(())
}

/// 一条 CNF 编码计划：待定格的一热变量 + 每条规则的子句。
struct Plan<'a> {
    table: &'a Table,
    cnf: Cnf,
    /// (行, 列) → (域, 与域一一对应的变量)
    cells: HashMap<(usize, usize), (Vec<Value>, Vec<i32>)>,
}

impl<'a> Plan<'a> {
    fn build(table: &'a Table, fills: &[Fill]) -> Result<Self, RepairError> {
        let mut plan = Self {
            table,
            cnf: Cnf::default(),
            cells: HashMap::new(),
        };
        let mut next = 1i32;
        for fill in fills {
            let vars: Vec<i32> = (0..fill.domain.len())
                .map(|_| {
                    let var = next;
                    next += 1;
                    var
                })
                .collect();
            plan.cells
                .insert((fill.row, fill.col), (fill.domain.clone(), vars.clone()));
        }
        let vars = (next - 1) as usize;
        if vars > MAX_VARS {
            return Err(RepairError::TooManyVariables {
                vars,
                limit: MAX_VARS,
            });
        }
        plan.cnf.nvars = vars;
        // 一热：至少一个 + 两两不同时成立。
        for vars in plan.cells.values().map(|(_, vars)| vars) {
            plan.cnf.clauses.push(vars.clone());
            for i in 0..vars.len() {
                for j in (i + 1)..vars.len() {
                    plan.cnf.clauses.push(vec![-vars[i], -vars[j]]);
                }
            }
        }
        Ok(plan)
    }

    fn total(&self) -> Result<u128, RepairError> {
        let mut total = 1u128;
        for (domain, _) in self.cells.values() {
            total = total
                .checked_mul(domain.len() as u128)
                .ok_or(RepairError::Overflow)?;
        }
        Ok(total)
    }

    fn unsat(&mut self) {
        self.cnf.clauses.push(Vec::new());
    }

    fn encode(&mut self, rules: &[Rule]) {
        for rule in rules {
            match rule {
                Rule::NotNull { col } => self.encode_not_null(*col),
                Rule::Check { col, op, lit } => self.encode_check(*col, *op, lit),
                Rule::Domain { col, values } => self.encode_domain(*col, values),
                Rule::Unique { col } => self.encode_unique(*col),
            }
        }
    }

    fn encode_not_null(&mut self, col: usize) {
        for row in 0..self.table.rows.len() {
            if self.cells.contains_key(&(row, col)) {
                continue; // 域保证非 NULL
            }
            if self.table.rows[row][col].is_null() {
                self.unsat();
            }
        }
    }

    fn encode_check(&mut self, col: usize, op: crate::dsl::CmpOp, lit: &Value) {
        for row in 0..self.table.rows.len() {
            if let Some((domain, vars)) = self.cells.get(&(row, col)) {
                for (value, var) in domain.iter().zip(vars) {
                    if value.compare(lit).is_some_and(|ord| !op.test(ord)) {
                        let var = *var;
                        self.cnf.clauses.push(vec![-var]);
                    }
                }
            } else {
                match self.table.rows[row][col].compare(lit) {
                    None => {} // UNKNOWN 放行
                    Some(ord) if !op.test(ord) => self.unsat(),
                    Some(_) => {}
                }
            }
        }
    }

    fn encode_domain(&mut self, col: usize, allowed: &[Value]) {
        for row in 0..self.table.rows.len() {
            if let Some((domain, vars)) = self.cells.get(&(row, col)) {
                for (value, var) in domain.iter().zip(vars) {
                    if !allowed.contains(value) {
                        let var = *var;
                        self.cnf.clauses.push(vec![-var]);
                    }
                }
            } else {
                let value = &self.table.rows[row][col];
                if !value.is_null() && !allowed.contains(value) {
                    self.unsat();
                }
            }
        }
    }

    fn encode_unique(&mut self, col: usize) {
        let rows = self.table.rows.len();
        for r1 in 0..rows {
            for r2 in (r1 + 1)..rows {
                // 一侧固定、另一侧待定：禁掉相等的取值。
                for (free_row, fixed_row) in [(r1, r2), (r2, r1)] {
                    if let Some((domain, vars)) = self.cells.get(&(free_row, col)) {
                        let fixed = &self.table.rows[fixed_row][col];
                        if fixed.is_null() {
                            continue;
                        }
                        for (value, var) in domain.iter().zip(vars) {
                            if value == fixed {
                                let var = *var;
                                self.cnf.clauses.push(vec![-var]);
                            }
                        }
                    }
                }
                // 两侧固定：相等即不可满足。
                let left = &self.table.rows[r1][col];
                let right = &self.table.rows[r2][col];
                let left_free = self.cells.contains_key(&(r1, col));
                let right_free = self.cells.contains_key(&(r2, col));
                if !left_free && !right_free && !left.is_null() && left == right {
                    self.unsat();
                }
                // 两侧都待定：共同取值不能同时取。
                if let (Some((domain1, vars1)), Some((domain2, vars2))) =
                    (self.cells.get(&(r1, col)), self.cells.get(&(r2, col)))
                {
                    for (value, var1) in domain1.iter().zip(vars1) {
                        if let Some(pos) = domain2.iter().position(|v| v == value) {
                            let var2 = vars2[pos];
                            let var1 = *var1;
                            self.cnf.clauses.push(vec![-var1, -var2]);
                        }
                    }
                }
            }
        }
    }
}

/// 内部辅助：编码 + 计数（测试用；生产路径走 [`completions`]）。
#[doc(hidden)]
pub fn encode_and_count(
    table: &Table,
    rules: &[Rule],
    fills: &[Fill],
) -> Result<Completion, RepairError> {
    completions(table, rules, fills)
}

#[cfg(test)]
mod tests {
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
    fn report_has_violation_detects_false_only() {
        let t = table(vec![vec![Value::Null, Value::Text("a".into())]]);
        let rules = vec![Rule::Check {
            col: 0,
            op: CmpOp::Ge,
            lit: Value::Int(0),
        }];
        assert!(!evaluate(&t, &rules).has_violation());
    }
}
