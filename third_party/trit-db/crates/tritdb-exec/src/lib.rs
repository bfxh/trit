#![forbid(unsafe_code)]
//! SQL value model and three-valued expression evaluation.
//!
//! `NULL` is `UNKNOWN` in boolean context. Predicates return
//! [`TruthValue`] internally and are materialized as `TRUE`/`FALSE`/`NULL`
//! when they appear as scalar values.

pub mod ann;
pub mod batch;
pub mod graph;

use std::cmp::Ordering;

use crate::ann::B4;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// A SQL scalar value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Value {
    /// SQL `NULL`, also boolean `UNKNOWN`.
    Null,
    /// Signed 64-bit integer.
    Int(i64),
    /// UTF-8 text.
    Text(String),
    /// Boolean.
    Bool(bool),
    /// Belnap four-valued logic (`LOGIC BELNAP`).
    Logic(B4),
    /// Łukasiewicz `[0, 1]` in thousandths (`LOGIC LUKASIEWICZ`).
    Lukasiewicz(u16),
    /// Natural-number counting annotation (`LOGIC NAT`).
    Nat(u64),
}

impl Value {
    /// SQL type name used in diagnostics.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "NULL",
            Value::Int(_) => "INT",
            Value::Text(_) => "TEXT",
            Value::Bool(_) => "BOOL",
            Value::Logic(_) => "BELNAP",
            Value::Lukasiewicz(_) => "LUKASIEWICZ",
            Value::Nat(_) => "NAT",
        }
    }

    /// Returns `true` for `NULL`.
    pub const fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Stable display form, used by the CLI and tests.
    pub fn display(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Int(value) => value.to_string(),
            Value::Text(value) => value.clone(),
            Value::Bool(true) => "TRUE".to_string(),
            Value::Bool(false) => "FALSE".to_string(),
            Value::Logic(value) => value.as_str().to_string(),
            Value::Lukasiewicz(value) => format!("{:.3}", f64::from(*value) / 1000.0),
            Value::Nat(value) => value.to_string(),
        }
    }
}

/// Column type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    /// 64-bit integer.
    Int,
    /// UTF-8 text.
    Text,
    /// Boolean.
    Bool,
    /// K3 annotation column (`LOGIC K3`). Stored as `TRUE`/`FALSE`/`NULL`.
    Logic,
    /// Belnap four-valued annotation column (`LOGIC BELNAP`).
    Belnap,
    /// Łukasiewicz annotation column (`LOGIC LUKASIEWICZ`).
    Lukasiewicz,
    /// Natural-number counting column (`LOGIC NAT`).
    Nat,
}

impl DataType {
    /// Parses a SQL type name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "INT" | "INTEGER" | "BIGINT" => Some(DataType::Int),
            "TEXT" | "VARCHAR" | "STRING" => Some(DataType::Text),
            "BOOL" | "BOOLEAN" => Some(DataType::Bool),
            "LOGIC" | "K3" => Some(DataType::Logic),
            "BELNAP" | "B4" => Some(DataType::Belnap),
            "LUKASIEWICZ" | "LUK" => Some(DataType::Lukasiewicz),
            "NAT" | "N" => Some(DataType::Nat),
            _ => None,
        }
    }

    /// Canonical type name.
    pub const fn name(self) -> &'static str {
        match self {
            DataType::Int => "INT",
            DataType::Text => "TEXT",
            DataType::Bool => "BOOL",
            DataType::Logic => "LOGIC",
            DataType::Belnap => "BELNAP",
            DataType::Lukasiewicz => "LUKASIEWICZ",
            DataType::Nat => "NAT",
        }
    }

    /// Coerces a value into this column type, rejecting `NULL` only when the
    /// caller separately enforces `NOT NULL`.
    pub fn coerce(self, value: Value) -> Result<Value, ExecError> {
        match (self, value) {
            (_, Value::Null) => Ok(Value::Null),
            (DataType::Int, Value::Int(value)) => Ok(Value::Int(value)),
            (DataType::Text, Value::Text(value)) => Ok(Value::Text(value)),
            (DataType::Bool, Value::Bool(value)) => Ok(Value::Bool(value)),
            (DataType::Logic, Value::Bool(value)) => Ok(Value::Bool(value)),
            (DataType::Logic, Value::Logic(value)) => match value {
                B4::True => Ok(Value::Bool(true)),
                B4::False => Ok(Value::Bool(false)),
                B4::Both | B4::Neither => Err(ExecError::TypeMismatch {
                    expected: "LOGIC K3",
                    actual: "LOGIC BELNAP",
                }),
            },
            (DataType::Belnap, Value::Logic(value)) => Ok(Value::Logic(value)),
            (DataType::Belnap, Value::Bool(value)) => Ok(Value::Logic(B4::from_bool(value))),
            (DataType::Lukasiewicz, Value::Lukasiewicz(value)) => Ok(Value::Lukasiewicz(value)),
            (DataType::Lukasiewicz, Value::Int(value)) if (0..=1000).contains(&value) => {
                Ok(Value::Lukasiewicz(value as u16))
            }
            (DataType::Nat, Value::Nat(value)) => Ok(Value::Nat(value)),
            (DataType::Nat, Value::Int(value)) if value >= 0 => Ok(Value::Nat(value as u64)),
            (expected, actual) => Err(ExecError::TypeMismatch {
                expected: expected.name(),
                actual: actual.type_name(),
            }),
        }
    }
}

/// A table column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    /// Column name. Unquoted SQL identifiers are normalized to lowercase.
    pub name: String,
    /// Column type.
    pub data_type: DataType,
    /// Reject `NULL` on insert.
    pub not_null: bool,
    /// Reject duplicate non-`NULL` values.
    pub unique: bool,
    /// Column-level `CHECK (expr)` text.
    pub check: Option<String>,
}

impl Column {
    /// Creates a nullable column.
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            not_null: false,
            unique: false,
            check: None,
        }
    }
}

/// Table schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schema {
    /// Ordered columns.
    pub columns: Vec<Column>,
    /// Additional qualified names that resolve to an existing column. `USING`
    /// and `NATURAL JOIN` expose one merged output column through both input
    /// qualifiers, while `SELECT *` must still see it only once.
    aliases: BTreeMap<String, usize>,
    /// Table-level `UNIQUE (a, b, ...)` groups, as column indices.
    pub unique_sets: Vec<Vec<usize>>,
    /// Table-level `CHECK (expr)` texts.
    pub checks: Vec<String>,
}

impl Schema {
    /// Creates a schema from columns.
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            aliases: BTreeMap::new(),
            unique_sets: Vec::new(),
            checks: Vec::new(),
        }
    }

    /// Adds qualified aliases that resolve to existing column indexes.
    pub fn with_aliases(mut self, aliases: BTreeMap<String, usize>) -> Self {
        self.aliases = aliases;
        self
    }

    /// Returns the alias map.
    pub fn aliases(&self) -> &BTreeMap<String, usize> {
        &self.aliases
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Returns `true` when there are no columns.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Resolves a column name exactly, then through a join alias.
    /// The parser lowercases unquoted identifiers and preserves quoted
    /// identifiers.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|column| column.name == name)
            .or_else(|| self.aliases.get(name).copied())
    }
}

/// One row, ordered by schema.
pub type Row = Vec<Value>;

/// A table held by the SQL engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// Table name.
    pub name: String,
    /// Schema.
    pub schema: Schema,
    /// Rows.
    pub rows: Vec<Row>,
}

impl Table {
    /// Materializes a `LOGIC K3` / `LOGIC BELNAP` column as a packed K3 vector.
    ///
    /// This is the optional vectorized read path: storage stays row-based, but
    /// scans can materialize annotation columns for [`batch::eval_predicate`].
    /// Returns `None` for other column types or a missing column.
    pub fn k3_column(&self, index: usize) -> Option<batch::K3Column> {
        let column = self.schema.columns.get(index)?;
        if !matches!(column.data_type, DataType::Logic | DataType::Belnap) {
            return None;
        }
        batch::K3Column::from_rows(&self.rows, index)
    }
}

/// SQL three-valued logic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TruthValue {
    /// True.
    True,
    /// False.
    False,
    /// Unknown (`NULL`).
    Unknown,
}

impl TruthValue {
    /// SQL `NOT`.
    pub const fn not(self) -> Self {
        match self {
            TruthValue::True => TruthValue::False,
            TruthValue::False => TruthValue::True,
            TruthValue::Unknown => TruthValue::Unknown,
        }
    }

    /// SQL `AND`.
    pub const fn and(self, other: Self) -> Self {
        match (self, other) {
            (TruthValue::False, _) | (_, TruthValue::False) => TruthValue::False,
            (TruthValue::True, TruthValue::True) => TruthValue::True,
            _ => TruthValue::Unknown,
        }
    }

    /// SQL `OR`.
    pub const fn or(self, other: Self) -> Self {
        match (self, other) {
            (TruthValue::True, _) | (_, TruthValue::True) => TruthValue::True,
            (TruthValue::False, TruthValue::False) => TruthValue::False,
            _ => TruthValue::Unknown,
        }
    }

    /// Materializes the boolean as a scalar: `UNKNOWN` becomes `NULL`.
    pub const fn to_value(self) -> Value {
        match self {
            TruthValue::True => Value::Bool(true),
            TruthValue::False => Value::Bool(false),
            TruthValue::Unknown => Value::Null,
        }
    }
}

/// Comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    /// `=`.
    Eq,
    /// `<>` or `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
}

/// Arithmetic operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
}

/// A bound expression. Column references are indexes into the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// Literal value.
    Literal(Value),
    /// Column index.
    Column(usize),
    /// `expr IS [NOT] NULL`.
    IsNull {
        /// Input expression.
        expr: Box<Expr>,
        /// `true` for `IS NOT NULL`.
        negated: bool,
    },
    /// `truth(expr [, threshold])`: projection to `TRUE`/`FALSE`/`UNKNOWN`
    /// text. Łukasiewicz values compare their thousandths against the
    /// threshold (default `500`), ℕ counts against the default `1`.
    Truth {
        /// Input annotation.
        expr: Box<Expr>,
        /// Optional `Int` threshold.
        threshold: Option<Box<Expr>>,
    },
    /// `certainty(expr [, threshold])`: `CERTAIN` / `CONTINGENT` /
    /// `IMPOSSIBLE` verdict, the same vocabulary as `UNDER REPAIR`.
    Certainty {
        /// Input annotation.
        expr: Box<Expr>,
        /// Optional `Int` threshold.
        threshold: Option<Box<Expr>>,
    },
    /// `expr IS [NOT] TRUE|FALSE|UNKNOWN`.
    IsTruth {
        /// Input expression.
        expr: Box<Expr>,
        /// Expected truth value.
        expected: TruthValue,
        /// `true` for `IS NOT`.
        negated: bool,
    },
    /// `NOT expr`.
    Not(Box<Expr>),
    /// `expr [NOT] IN (value, ...)`.
    InList {
        /// Probe expression.
        expr: Box<Expr>,
        /// Candidate values.
        values: Vec<Expr>,
        /// `true` for `NOT IN`.
        negated: bool,
    },
    /// `expr [NOT] IN (<constant set>)`, pre-indexed so each row is O(1).
    InSet {
        /// Probe expression.
        expr: Box<Expr>,
        /// Constant candidates; includes [`Value::Null`] when the list had one.
        values: std::collections::HashSet<Value>,
        /// Type shared by every non-NULL candidate.
        type_name: &'static str,
        /// `true` for `NOT IN`.
        negated: bool,
    },
    /// `expr [NOT] BETWEEN low AND high`.
    Between {
        /// Probe expression.
        expr: Box<Expr>,
        /// Lower bound.
        low: Box<Expr>,
        /// Upper bound.
        high: Box<Expr>,
        /// `true` for `NOT BETWEEN`.
        negated: bool,
    },
    /// `expr [NOT] LIKE pattern`.
    Like {
        /// Probe expression.
        expr: Box<Expr>,
        /// Pattern expression.
        pattern: Box<Expr>,
        /// `true` for `NOT LIKE`.
        negated: bool,
    },
    /// `CASE [operand] WHEN ... THEN ... [ELSE ...] END`.
    Case {
        /// Simple-case operand; `None` for a searched case.
        operand: Option<Box<Expr>>,
        /// `WHEN`/`THEN` pairs in source order.
        branches: Vec<(Expr, Expr)>,
        /// `ELSE` result; `None` yields `NULL` when nothing matches.
        else_expr: Option<Box<Expr>>,
    },
    /// Binary boolean, comparison, or arithmetic operator.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
    },
}

/// Binary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// `AND`.
    And,
    /// `OR`.
    Or,
    /// `=`.
    Eq,
    /// `<>`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
}

impl BinaryOp {
    /// Comparison operator, if this is one.
    pub const fn cmp(self) -> Option<CmpOp> {
        match self {
            BinaryOp::Eq => Some(CmpOp::Eq),
            BinaryOp::Ne => Some(CmpOp::Ne),
            BinaryOp::Lt => Some(CmpOp::Lt),
            BinaryOp::Le => Some(CmpOp::Le),
            BinaryOp::Gt => Some(CmpOp::Gt),
            BinaryOp::Ge => Some(CmpOp::Ge),
            _ => None,
        }
    }

    /// Arithmetic operator, if this is one.
    pub const fn arith(self) -> Option<ArithOp> {
        match self {
            BinaryOp::Add => Some(ArithOp::Add),
            BinaryOp::Sub => Some(ArithOp::Sub),
            BinaryOp::Mul => Some(ArithOp::Mul),
            BinaryOp::Div => Some(ArithOp::Div),
            _ => None,
        }
    }
}

/// Structured expression/execution error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// A bound column index is outside the row.
    RowShape {
        /// Schema column count.
        expected: usize,
        /// Actual row length.
        actual: usize,
    },
    /// A column index is outside the schema.
    ColumnIndex {
        /// Index.
        index: usize,
        /// Schema column count.
        columns: usize,
    },
    /// Type mismatch.
    TypeMismatch {
        /// Expected type.
        expected: &'static str,
        /// Actual type.
        actual: &'static str,
    },
    /// Division by zero.
    DivisionByZero,
    /// `i64` overflow.
    IntegerOverflow,
    /// A predicate expression produced a non-boolean value.
    NonBooleanPredicate {
        /// Actual type.
        actual: &'static str,
    },
    /// `LIKE` pattern exceeds the safety limit.
    LikePatternTooLong {
        /// Maximum accepted pattern length in characters.
        max: usize,
    },
    /// `LIKE` matching exceeded its comparison budget.
    LikeTooComplex,
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::RowShape { expected, actual } => {
                write!(
                    f,
                    "row shape mismatch: expected {expected} values, got {actual}"
                )
            }
            ExecError::ColumnIndex { index, columns } => {
                write!(f, "column index {index} out of range for {columns} columns")
            }
            ExecError::TypeMismatch { expected, actual } => {
                write!(f, "type mismatch: expected {expected}, got {actual}")
            }
            ExecError::DivisionByZero => write!(f, "division by zero"),
            ExecError::IntegerOverflow => write!(f, "integer overflow"),
            ExecError::NonBooleanPredicate { actual } => {
                write!(f, "predicate must be BOOLEAN, got {actual}")
            }
            ExecError::LikePatternTooLong { max } => {
                write!(f, "LIKE pattern is longer than {max} characters")
            }
            ExecError::LikeTooComplex => {
                write!(f, "LIKE comparison exceeded its budget")
            }
        }
    }
}

impl Error for ExecError {}

/// Converts a scalar to a truth value. `NULL` is `UNKNOWN`; non-boolean
/// scalars are rejected in predicate context.
pub fn truth_of(value: &Value) -> Result<TruthValue, ExecError> {
    match value {
        Value::Bool(true) => Ok(TruthValue::True),
        Value::Bool(false) => Ok(TruthValue::False),
        Value::Null => Ok(TruthValue::Unknown),
        Value::Logic(B4::True) => Ok(TruthValue::True),
        Value::Logic(B4::False) => Ok(TruthValue::False),
        Value::Logic(B4::Both | B4::Neither) => Ok(TruthValue::Unknown),
        other => Err(ExecError::NonBooleanPredicate {
            actual: other.type_name(),
        }),
    }
}

/// Projects any annotation to three-valued truth for `truth(...)`.
///
/// `threshold` is only meaningful for Łukasiewicz (thousandths) and ℕ
/// (count); K3/Belnap ignore it.
pub fn truth_projection(value: &Value, threshold: Option<&Value>) -> Result<TruthValue, ExecError> {
    match value {
        Value::Lukasiewicz(milli) => {
            let limit = match threshold {
                Some(Value::Int(raw)) => (*raw).clamp(0, 1000) as u16,
                Some(Value::Lukasiewicz(items)) => *items,
                Some(other) => {
                    return Err(ExecError::TypeMismatch {
                        expected: "INT",
                        actual: other.type_name(),
                    })
                }
                None => 500,
            };
            Ok(if *milli >= limit {
                TruthValue::True
            } else {
                TruthValue::False
            })
        }
        Value::Nat(count) => {
            let limit = match threshold {
                Some(Value::Int(raw)) => (*raw).max(0) as u64,
                Some(Value::Nat(raw)) => *raw,
                Some(other) => {
                    return Err(ExecError::TypeMismatch {
                        expected: "INT",
                        actual: other.type_name(),
                    })
                }
                None => 1,
            };
            Ok(if *count >= limit {
                TruthValue::True
            } else {
                TruthValue::False
            })
        }
        _ => truth_of(value),
    }
}

pub fn truth_text(truth: TruthValue) -> &'static str {
    match truth {
        TruthValue::True => "TRUE",
        TruthValue::False => "FALSE",
        TruthValue::Unknown => "UNKNOWN",
    }
}

/// Verdict for `certainty(...)`: the same `CERTAIN` / `CONTINGENT` /
/// `IMPOSSIBLE` vocabulary as `UNDER REPAIR`, but over a single annotation.
pub fn certainty_of_value(
    value: &Value,
    threshold: Option<&Value>,
) -> Result<crate::ann::Certainty, ExecError> {
    let annotation = match value {
        Value::Bool(true) => crate::ann::Annotation::K3(TruthValue::True),
        Value::Bool(false) => crate::ann::Annotation::K3(TruthValue::False),
        Value::Null => crate::ann::Annotation::K3(TruthValue::Unknown),
        Value::Logic(four) => crate::ann::Annotation::Belnap(*four),
        Value::Lukasiewicz(milli) => {
            crate::ann::Annotation::Lukasiewicz(f64::from(*milli) / 1000.0)
        }
        Value::Nat(count) => crate::ann::Annotation::Nat(*count),
        other => {
            return Err(ExecError::NonBooleanPredicate {
                actual: other.type_name(),
            })
        }
    };
    let threshold = match threshold {
        None => None,
        Some(Value::Int(raw)) => match value {
            // Łukasiewicz thresholds are given in thousandths.
            Value::Lukasiewicz(_) => Some(*raw as f64 / 1000.0),
            _ => Some(*raw as f64),
        },
        Some(Value::Lukasiewicz(items)) => Some(f64::from(*items) / 1000.0),
        Some(Value::Nat(items)) => Some(*items as f64),
        Some(other) => {
            return Err(ExecError::TypeMismatch {
                expected: "INT",
                actual: other.type_name(),
            })
        }
    };
    if let Some(threshold) = threshold {
        let bounded = matches!(value, Value::Lukasiewicz(_));
        if !threshold.is_finite() || threshold < 0.0 || (bounded && threshold > 1.0) {
            return Err(ExecError::TypeMismatch {
                expected: "threshold in [0, 1]",
                actual: "out of range",
            });
        }
    }
    annotation
        .certainty(threshold)
        .map_err(|_| ExecError::NonBooleanPredicate {
            actual: value.type_name(),
        })
}

/// Compares two values. `NULL` yields `None`; mixed non-null types are an
/// error rather than an implicit cast.
pub fn compare_values(left: &Value, right: &Value) -> Result<Option<Ordering>, ExecError> {
    match (left, right) {
        (Value::Null, _) | (_, Value::Null) => Ok(None),
        (Value::Logic(left), Value::Logic(right)) => Ok(Some(left.cmp(right))),
        (Value::Lukasiewicz(left), Value::Lukasiewicz(right)) => Ok(Some(left.cmp(right))),
        (Value::Nat(left), Value::Nat(right)) => Ok(Some(left.cmp(right))),
        (Value::Bool(left), Value::Logic(right)) => Ok(Some(B4::from_bool(*left).cmp(right))),
        (Value::Logic(left), Value::Bool(right)) => Ok(Some(left.cmp(&B4::from_bool(*right)))),
        (Value::Int(left), Value::Int(right)) => Ok(Some(left.cmp(right))),
        (Value::Text(left), Value::Text(right)) => Ok(Some(left.cmp(right))),
        (Value::Bool(left), Value::Bool(right)) => Ok(Some(left.cmp(right))),
        (left, right) => Err(ExecError::TypeMismatch {
            expected: left.type_name(),
            actual: right.type_name(),
        }),
    }
}

fn compare_op(op: CmpOp, ordering: Ordering) -> bool {
    match op {
        CmpOp::Eq => ordering == Ordering::Equal,
        CmpOp::Ne => ordering != Ordering::Equal,
        CmpOp::Lt => ordering == Ordering::Less,
        CmpOp::Le => ordering != Ordering::Greater,
        CmpOp::Gt => ordering == Ordering::Greater,
        CmpOp::Ge => ordering != Ordering::Less,
    }
}

fn comparison_truth(op: CmpOp, ordering: Option<Ordering>) -> TruthValue {
    match ordering {
        None => TruthValue::Unknown,
        Some(ordering) => {
            if compare_op(op, ordering) {
                TruthValue::True
            } else {
                TruthValue::False
            }
        }
    }
}

/// Maximum `LIKE` pattern length in characters.
const MAX_LIKE_PATTERN: usize = 64 * 1024;
/// Maximum character comparisons one `LIKE` evaluation may perform, so a
/// hostile pattern cannot burn CPU indefinitely.
const MAX_LIKE_STEPS: usize = 10_000_000;

/// SQLite-style `LIKE`: `%` matches any sequence, `_` matches one character,
/// and ASCII letters compare case-insensitively.
///
/// Greedy wildcard matching that backtracks on the last `%`: constant extra
/// space instead of the `text x pattern` boolean matrix this used to allocate
/// (which let one query reserve terabytes).
fn like_matches(text: &str, pattern: &str) -> Result<bool, ExecError> {
    let pattern: Vec<char> = pattern.chars().collect();
    if pattern.len() > MAX_LIKE_PATTERN {
        return Err(ExecError::LikePatternTooLong {
            max: MAX_LIKE_PATTERN,
        });
    }
    let text: Vec<char> = text.chars().collect();
    let mut text_at = 0usize;
    let mut pattern_at = 0usize;
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    let mut steps = 0usize;
    while text_at < text.len() {
        steps += 1;
        if steps > MAX_LIKE_STEPS {
            return Err(ExecError::LikeTooComplex);
        }
        if pattern_at < pattern.len()
            && (pattern[pattern_at] == '_'
                || pattern[pattern_at].eq_ignore_ascii_case(&text[text_at]))
        {
            text_at += 1;
            pattern_at += 1;
        } else if pattern_at < pattern.len() && pattern[pattern_at] == '%' {
            star = Some(pattern_at + 1);
            mark = text_at;
            pattern_at += 1;
        } else if let Some(after_star) = star {
            mark += 1;
            text_at = mark;
            pattern_at = after_star;
        } else {
            return Ok(false);
        }
    }
    while pattern_at < pattern.len() && pattern[pattern_at] == '%' {
        pattern_at += 1;
    }
    Ok(pattern_at == pattern.len())
}

fn eval_arith(op: ArithOp, left: i64, right: i64) -> Result<Value, ExecError> {
    let value = match op {
        ArithOp::Add => left.checked_add(right),
        ArithOp::Sub => left.checked_sub(right),
        ArithOp::Mul => left.checked_mul(right),
        ArithOp::Div => {
            if right == 0 {
                return Err(ExecError::DivisionByZero);
            }
            left.checked_div(right)
        }
    };
    value.map(Value::Int).ok_or(ExecError::IntegerOverflow)
}

/// Evaluates a bound expression against a schema and row.
pub fn eval(expr: &Expr, schema: &Schema, row: &[Value]) -> Result<Value, ExecError> {
    if row.len() != schema.len() {
        return Err(ExecError::RowShape {
            expected: schema.len(),
            actual: row.len(),
        });
    }
    match expr {
        Expr::Literal(value) => Ok(value.clone()),
        Expr::Column(index) => {
            if *index >= schema.len() {
                return Err(ExecError::ColumnIndex {
                    index: *index,
                    columns: schema.len(),
                });
            }
            Ok(row[*index].clone())
        }
        Expr::IsNull { expr, negated } => {
            let value = eval(expr, schema, row)?;
            let is_null = value.is_null();
            Ok(Value::Bool(if *negated { !is_null } else { is_null }))
        }
        Expr::Truth { expr, threshold } => {
            let value = eval(expr, schema, row)?;
            let threshold = match threshold {
                Some(threshold) => Some(eval(threshold, schema, row)?),
                None => None,
            };
            let truth = truth_projection(&value, threshold.as_ref())?;
            Ok(Value::Text(truth_text(truth).to_string()))
        }
        Expr::Certainty { expr, threshold } => {
            let value = eval(expr, schema, row)?;
            let threshold = match threshold {
                Some(threshold) => Some(eval(threshold, schema, row)?),
                None => None,
            };
            let verdict = certainty_of_value(&value, threshold.as_ref())?;
            Ok(Value::Text(verdict.as_str().to_string()))
        }
        Expr::IsTruth {
            expr,
            expected,
            negated,
        } => {
            let value = eval(expr, schema, row)?;
            let truth = truth_of(&value)?;
            let matched = truth == *expected;
            Ok(Value::Bool(if *negated { !matched } else { matched }))
        }
        Expr::Not(expr) => {
            let value = eval(expr, schema, row)?;
            Ok(truth_of(&value)?.not().to_value())
        }
        Expr::InList {
            expr,
            values,
            negated,
        } => {
            let probe = eval(expr, schema, row)?;
            let mut unknown = probe.is_null();
            let mut matched = false;
            for value in values {
                match compare_values(&probe, &eval(value, schema, row)?)? {
                    Some(Ordering::Equal) => {
                        matched = true;
                        break;
                    }
                    Some(_) => {}
                    None => unknown = true,
                }
            }
            let truth = if matched {
                TruthValue::True
            } else if unknown {
                TruthValue::Unknown
            } else {
                TruthValue::False
            };
            Ok(if *negated {
                truth.not().to_value()
            } else {
                truth.to_value()
            })
        }
        Expr::InSet {
            expr,
            values,
            type_name,
            negated,
        } => {
            let probe = eval(expr, schema, row)?;
            let same_type = !probe.is_null() && probe.type_name() == *type_name;
            let matched = same_type && values.contains(&probe);
            let unknown = probe.is_null() || !same_type || values.contains(&Value::Null);
            let truth = if matched {
                TruthValue::True
            } else if unknown {
                TruthValue::Unknown
            } else {
                TruthValue::False
            };
            Ok(if *negated {
                truth.not().to_value()
            } else {
                truth.to_value()
            })
        }
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => {
            let value = eval(expr, schema, row)?;
            let lower =
                comparison_truth(CmpOp::Ge, compare_values(&value, &eval(low, schema, row)?)?);
            let upper = comparison_truth(
                CmpOp::Le,
                compare_values(&value, &eval(high, schema, row)?)?,
            );
            let truth = lower.and(upper);
            Ok(if *negated {
                truth.not().to_value()
            } else {
                truth.to_value()
            })
        }
        Expr::Like {
            expr,
            pattern,
            negated,
        } => {
            let value = eval(expr, schema, row)?;
            let pattern = eval(pattern, schema, row)?;
            match (&value, &pattern) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Text(text), Value::Text(pattern)) => {
                    let matched = like_matches(text, pattern)?;
                    Ok(Value::Bool(if *negated { !matched } else { matched }))
                }
                (value, _) => Err(ExecError::TypeMismatch {
                    expected: "TEXT",
                    actual: value.type_name(),
                }),
            }
        }
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            if let Some(operand) = operand {
                let probe = eval(operand, schema, row)?;
                for (when, then) in branches {
                    let candidate = eval(when, schema, row)?;
                    if compare_values(&probe, &candidate)? == Some(Ordering::Equal) {
                        return eval(then, schema, row);
                    }
                }
            } else {
                for (when, then) in branches {
                    let value = eval(when, schema, row)?;
                    if truth_of(&value)? == TruthValue::True {
                        return eval(then, schema, row);
                    }
                }
            }
            match else_expr {
                Some(expr) => eval(expr, schema, row),
                None => Ok(Value::Null),
            }
        }
        Expr::Binary { op, left, right } => {
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                let left_value = eval(left, schema, row)?;
                let right_value = eval(right, schema, row)?;
                let left_truth = truth_of(&left_value)?;
                let right_truth = truth_of(&right_value)?;
                let result = if matches!(op, BinaryOp::And) {
                    left_truth.and(right_truth)
                } else {
                    left_truth.or(right_truth)
                };
                return Ok(result.to_value());
            }

            let left_value = eval(left, schema, row)?;
            let right_value = eval(right, schema, row)?;

            if let Some(cmp) = op.cmp() {
                return match compare_values(&left_value, &right_value)? {
                    None => Ok(Value::Null),
                    Some(ordering) => Ok(Value::Bool(compare_op(cmp, ordering))),
                };
            }

            if let Some(arith) = op.arith() {
                if left_value.is_null() || right_value.is_null() {
                    return Ok(Value::Null);
                }
                return match (left_value, right_value) {
                    (Value::Int(left), Value::Int(right)) => eval_arith(arith, left, right),
                    (left, right) => Err(ExecError::TypeMismatch {
                        expected: left.type_name(),
                        actual: right.type_name(),
                    }),
                };
            }

            unreachable!("all binary operators handled")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_k3_column_only_for_annotation_columns() {
        let schema = Schema::new(vec![
            Column {
                name: "id".to_string(),
                data_type: DataType::Int,
                not_null: true,
                unique: false,
                check: None,
            },
            Column {
                name: "flag".to_string(),
                data_type: DataType::Logic,
                not_null: false,
                unique: false,
                check: None,
            },
        ]);
        let table = Table {
            name: "t".to_string(),
            schema,
            rows: vec![vec![Value::Int(1), Value::Bool(true)]],
        };
        assert!(table.k3_column(0).is_none());
        let column = table.k3_column(1).expect("logic column");
        assert_eq!(column.get(0), Some(crate::batch::K3::True));
        assert!(table.k3_column(2).is_none());
    }

    fn truth(op: BinaryOp, left: TruthValue, right: TruthValue) -> TruthValue {
        let schema = Schema::new(vec![
            Column::new("a", DataType::Bool),
            Column::new("b", DataType::Bool),
        ]);
        let row = vec![left.to_value(), right.to_value()];
        let expr = Expr::Binary {
            op,
            left: Box::new(Expr::Column(0)),
            right: Box::new(Expr::Column(1)),
        };
        let value = eval(&expr, &schema, &row).expect("eval");
        truth_of(&value).expect("truth")
    }

    fn all() -> [TruthValue; 3] {
        [TruthValue::True, TruthValue::False, TruthValue::Unknown]
    }

    #[test]
    fn not_matches_sql_truth_table() {
        assert_eq!(TruthValue::True.not(), TruthValue::False);
        assert_eq!(TruthValue::False.not(), TruthValue::True);
        assert_eq!(TruthValue::Unknown.not(), TruthValue::Unknown);
    }

    #[test]
    fn and_matches_sql_truth_table() {
        for left in all() {
            for right in all() {
                let expected = match (left, right) {
                    (TruthValue::False, _) | (_, TruthValue::False) => TruthValue::False,
                    (TruthValue::True, TruthValue::True) => TruthValue::True,
                    _ => TruthValue::Unknown,
                };
                assert_eq!(
                    truth(BinaryOp::And, left, right),
                    expected,
                    "{left:?} AND {right:?}"
                );
            }
        }
    }

    #[test]
    fn or_matches_sql_truth_table() {
        for left in all() {
            for right in all() {
                let expected = match (left, right) {
                    (TruthValue::True, _) | (_, TruthValue::True) => TruthValue::True,
                    (TruthValue::False, TruthValue::False) => TruthValue::False,
                    _ => TruthValue::Unknown,
                };
                assert_eq!(
                    truth(BinaryOp::Or, left, right),
                    expected,
                    "{left:?} OR {right:?}"
                );
            }
        }
    }

    #[test]
    fn null_comparison_is_unknown() {
        assert_eq!(
            compare_values(&Value::Null, &Value::Int(1)).expect("cmp"),
            None
        );
        assert_eq!(
            compare_values(&Value::Int(1), &Value::Null).expect("cmp"),
            None
        );
    }

    #[test]
    fn comparisons_produce_bool_or_null() {
        let schema = Schema::new(vec![Column::new("a", DataType::Int)]);
        let row = vec![Value::Int(2)];
        let expr = Expr::Binary {
            op: BinaryOp::Gt,
            left: Box::new(Expr::Column(0)),
            right: Box::new(Expr::Literal(Value::Int(1))),
        };
        assert_eq!(eval(&expr, &schema, &row).expect("eval"), Value::Bool(true));

        let null_expr = Expr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Column(0)),
            right: Box::new(Expr::Literal(Value::Null)),
        };
        assert_eq!(eval(&null_expr, &schema, &row).expect("eval"), Value::Null);
    }

    #[test]
    fn is_null_and_is_unknown_are_boolean() {
        let schema = Schema::new(vec![Column::new("a", DataType::Int)]);
        let row = vec![Value::Null];
        let is_null = Expr::IsNull {
            expr: Box::new(Expr::Column(0)),
            negated: false,
        };
        assert_eq!(
            eval(&is_null, &schema, &row).expect("eval"),
            Value::Bool(true)
        );

        let is_unknown = Expr::IsTruth {
            expr: Box::new(Expr::Column(0)),
            expected: TruthValue::Unknown,
            negated: false,
        };
        assert_eq!(
            eval(&is_unknown, &schema, &row).expect("eval"),
            Value::Bool(true)
        );
    }

    #[test]
    fn arithmetic_propagates_null_and_checks_division() {
        let schema = Schema::new(vec![Column::new("a", DataType::Int)]);
        let row = vec![Value::Int(10)];
        let null_add = Expr::Binary {
            op: BinaryOp::Add,
            left: Box::new(Expr::Literal(Value::Null)),
            right: Box::new(Expr::Column(0)),
        };
        assert_eq!(eval(&null_add, &schema, &row).expect("eval"), Value::Null);

        let div_zero = Expr::Binary {
            op: BinaryOp::Div,
            left: Box::new(Expr::Column(0)),
            right: Box::new(Expr::Literal(Value::Int(0))),
        };
        assert!(matches!(
            eval(&div_zero, &schema, &row),
            Err(ExecError::DivisionByZero)
        ));
    }

    #[test]
    fn in_list_uses_three_valued_logic() {
        let schema = Schema::new(vec![Column::new("a", DataType::Int)]);
        let row = vec![Value::Int(1)];
        let probe = || Box::new(Expr::Column(0));
        let list = |values: Vec<Value>, negated: bool| Expr::InList {
            expr: probe(),
            values: values.into_iter().map(Expr::Literal).collect(),
            negated,
        };
        assert_eq!(
            eval(
                &list(vec![Value::Int(1), Value::Int(3)], false),
                &schema,
                &row
            )
            .expect("in"),
            Value::Bool(true)
        );
        assert_eq!(
            eval(
                &list(vec![Value::Int(2), Value::Int(3)], false),
                &schema,
                &row
            )
            .expect("in"),
            Value::Bool(false)
        );
        assert_eq!(
            eval(
                &list(vec![Value::Int(2), Value::Null], false),
                &schema,
                &row
            )
            .expect("in"),
            Value::Null
        );
        assert_eq!(
            eval(&list(vec![Value::Int(2), Value::Null], true), &schema, &row).expect("not in"),
            Value::Null
        );
        assert_eq!(
            eval(
                &list(vec![Value::Int(1), Value::Int(2)], true),
                &schema,
                &row
            )
            .expect("not in"),
            Value::Bool(false)
        );
    }

    #[test]
    fn between_is_three_valued_conjunction() {
        let schema = Schema::new(vec![Column::new("a", DataType::Int)]);
        let row = vec![Value::Int(20)];
        let between = |low: Value, high: Value, negated: bool| Expr::Between {
            expr: Box::new(Expr::Column(0)),
            low: Box::new(Expr::Literal(low)),
            high: Box::new(Expr::Literal(high)),
            negated,
        };
        assert_eq!(
            eval(
                &between(Value::Int(10), Value::Int(30), false),
                &schema,
                &row
            )
            .expect("between"),
            Value::Bool(true)
        );
        assert_eq!(
            eval(
                &between(Value::Int(21), Value::Int(30), false),
                &schema,
                &row
            )
            .expect("between"),
            Value::Bool(false)
        );
        assert_eq!(
            eval(&between(Value::Null, Value::Int(30), false), &schema, &row).expect("between"),
            Value::Null
        );
        assert_eq!(
            eval(&between(Value::Int(21), Value::Null, false), &schema, &row).expect("between"),
            Value::Bool(false)
        );
        assert_eq!(
            eval(
                &between(Value::Int(21), Value::Int(30), true),
                &schema,
                &row
            )
            .expect("not between"),
            Value::Bool(true)
        );
    }

    #[test]
    fn like_matches_wildcards_and_ascii_case() {
        assert!(like_matches("abc", "a%").expect("like"));
        assert!(like_matches("abc", "A%").expect("like"));
        assert!(like_matches("abc", "a_c").expect("like"));
        assert!(!like_matches("abc", "a_").expect("like"));
        assert!(like_matches("", "%").expect("like"));
        assert!(like_matches("abc", "%b%").expect("like"));
        assert!(!like_matches("abc", "abd").expect("like"));
        assert!(like_matches("中", "_").expect("like"));
        assert!(!like_matches("中文", "_").expect("like"));
    }

    #[test]
    fn like_rejects_oversized_patterns_and_runaway_backtracking() {
        let oversized = "%".repeat(MAX_LIKE_PATTERN + 1);
        assert!(matches!(
            like_matches("abc", &oversized),
            Err(ExecError::LikePatternTooLong { .. })
        ));

        // `%a...ab` against a long run of `a` forces quadratic backtracking;
        // the step budget turns that into an error instead of a hang.
        let text = "a".repeat(8000);
        let mut pattern = String::from("%");
        pattern.push_str(&"a".repeat(3000));
        pattern.push('b');
        assert!(matches!(
            like_matches(&text, &pattern),
            Err(ExecError::LikeTooComplex)
        ));

        // The greedy matcher still agrees with the old table on real patterns.
        assert!(like_matches("aab", "%a%a%b").expect("like"));
        assert!(!like_matches("aab", "%a%a%c").expect("like"));
        assert!(like_matches("", "%").expect("like"));
        assert!(!like_matches("abc", "").expect("like"));

        let schema = Schema::new(vec![Column::new("a", DataType::Text)]);
        let row = vec![Value::Null];
        let like = Expr::Like {
            expr: Box::new(Expr::Column(0)),
            pattern: Box::new(Expr::Literal(Value::Text("a%".to_string()))),
            negated: false,
        };
        assert_eq!(eval(&like, &schema, &row).expect("like"), Value::Null);

        let row = vec![Value::Int(1)];
        assert!(matches!(
            eval(&like, &schema, &row),
            Err(ExecError::TypeMismatch {
                expected: "TEXT",
                ..
            })
        ));
    }

    #[test]
    fn case_is_three_valued_and_lazily_evaluated() {
        let schema = Schema::new(vec![
            Column::new("flag", DataType::Bool),
            Column::new("score", DataType::Int),
        ]);
        let true_row = vec![Value::Bool(true), Value::Int(10)];
        let null_row = vec![Value::Null, Value::Int(20)];

        let searched = Expr::Case {
            operand: None,
            branches: vec![
                (
                    Expr::Column(0),
                    Expr::Literal(Value::Text("yes".to_string())),
                ),
                (
                    Expr::IsNull {
                        expr: Box::new(Expr::Column(0)),
                        negated: false,
                    },
                    Expr::Literal(Value::Text("unknown".to_string())),
                ),
            ],
            else_expr: Some(Box::new(Expr::Literal(Value::Text("no".to_string())))),
        };
        assert_eq!(
            eval(&searched, &schema, &true_row).expect("true"),
            Value::Text("yes".to_string())
        );
        assert_eq!(
            eval(&searched, &schema, &null_row).expect("null"),
            Value::Text("unknown".to_string())
        );

        let no_else = Expr::Case {
            operand: None,
            branches: vec![(Expr::Column(0), Expr::Literal(Value::Int(1)))],
            else_expr: None,
        };
        assert_eq!(
            eval(&no_else, &schema, &null_row).expect("no else"),
            Value::Null
        );

        let simple = Expr::Case {
            operand: Some(Box::new(Expr::Column(1))),
            branches: vec![
                (Expr::Literal(Value::Int(10)), Expr::Literal(Value::Int(1))),
                (Expr::Literal(Value::Int(20)), Expr::Literal(Value::Int(2))),
            ],
            else_expr: Some(Box::new(Expr::Literal(Value::Int(0)))),
        };
        assert_eq!(
            eval(&simple, &schema, &true_row).expect("simple ten"),
            Value::Int(1)
        );
        assert_eq!(
            eval(&simple, &schema, &null_row).expect("simple twenty"),
            Value::Int(2)
        );

        let lazy = Expr::Case {
            operand: None,
            branches: vec![(
                Expr::Literal(Value::Bool(true)),
                Expr::Literal(Value::Int(1)),
            )],
            else_expr: Some(Box::new(Expr::Binary {
                op: BinaryOp::Div,
                left: Box::new(Expr::Literal(Value::Int(1))),
                right: Box::new(Expr::Literal(Value::Int(0))),
            })),
        };
        assert_eq!(
            eval(&lazy, &schema, &true_row).expect("lazy"),
            Value::Int(1)
        );
    }

    #[test]
    fn non_boolean_predicate_is_rejected() {
        let schema = Schema::new(vec![Column::new("a", DataType::Int)]);
        let row = vec![Value::Int(1)];
        let expr = Expr::Column(0);
        assert!(matches!(
            truth_of(&eval(&expr, &schema, &row).expect("eval")),
            Err(ExecError::NonBooleanPredicate { .. })
        ));
    }
}
