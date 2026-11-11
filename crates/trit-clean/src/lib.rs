//! 三值约束检查 + 精确修复计数（`trit-clean` M0）。
//!
//! 数据清洗里“未知 / 矛盾 / 待定”本来就是三值判定：本 crate 把规则编译成三值求值与 CNF，
//! 用 [`trit_count`] 的 DPLL 做**精确计数**，给出可解释的清洁度分数与最小修复条数。
//!
//! ```
//! use trit_clean::{parse_rules, evaluate, Column, Table, Ty, Value};
//!
//! let mut table = Table::new(vec![Column::new("age", Ty::Int)]);
//! table.push_row(vec![Value::Null]).unwrap();
//! table.push_row(vec![Value::Int(-1)]).unwrap();
//! let rules = parse_rules(&table, "NOT NULL age\nCHECK age >= 0").unwrap();
//! let report = evaluate(&table, &rules);
//! assert_eq!(report.counts().no, 2); // 一行 NULL 违反 NOT NULL，一行 -1 违反 CHECK
//! assert!(report.score() < 0.5);
//! ```
//!
//! 精确计数（[`completions`] / [`minimal_repairs`]）的边界：变量数、候选格数有硬上限；
//! 变量超限时可显式改用 [`completions_or_sample`]（抽样结果带标记与置信区间），
//! 其余超限仍是结构化错误。两条独立计数链（DPLL / BDD）可用 [`crosscheck`] 对拍。
//!
//! 口径与 SQL 三值逻辑对齐：`NULL` 参与比较得到 UNKNOWN；`CHECK` 只在 False 时违反；
//! `UNIQUE` 里 `NULL` 互不冲突。对拍见 `tests/sqlite_parity.rs`（真 SQLite）。

#![forbid(unsafe_code)]

pub mod bdd;
pub mod dsl;
pub mod eval;
pub mod repair;
pub mod sample;
pub mod value;

pub use bdd::{BDD_MAX_NODES, Crosscheck, crosscheck};
pub use dsl::{CmpOp, Rule, RuleError, parse_rules};
pub use eval::{Counts, NodeState, Report, TableState, eval_row_rule, eval_unique, evaluate};
pub use repair::{
    Completion, Fill, MAX_CELLS, MAX_VARS, RepairCount, RepairError, completions, minimal_repairs,
    minimal_repairs_auto,
};
pub use sample::{CompletionOutcome, SampleBudget, Sampled, completions_or_sample};
pub use value::{Column, Table, TableError, Ty, Value};
