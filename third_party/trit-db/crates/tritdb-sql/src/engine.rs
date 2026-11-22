use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::str;
use std::time::Instant;

use tritdb_exec::{
    ann::B4,
    batch::{eval_predicate, filter_indices, K3Column, TriPredicate, TritVec},
    certainty_of_value, compare_values, eval, truth_of, truth_projection, truth_text, BinaryOp,
    Column, DataType, ExecError, Expr as ExecExpr, Row, Schema, Table, TruthValue, Value,
};
use tritdb_txn::TxnStore;

use crate::ast::{
    AggregateKind, Assignment, Cte, Expr, Join, JoinCondition, JoinKind, OrderBy, Quantifier,
    Query, Select, SelectItem, Statement, WindowFunction, With,
};
use crate::bind::{
    bind_expr, bind_select, eval_constant, select_has_aggregate, BoundSelect, GroupExpr,
};
use crate::error::{closest_names, NameKind, SqlError};
use crate::parser::parse_script;
use crate::plan::{
    AggregatePlan, JoinKey, JoinKeys, JoinPlan, Metrics, PlanExpr, PlanKind, PlanNode, ProjectItem,
    ProjectPlan, SortKey, SortPlan,
};
use crate::stats::{analyze_table, TableStats};
use crate::storage::{
    decode_stats, decode_table, encode_stats, encode_table, stats_key, table_key, STATS_KEY_PREFIX,
    TABLE_KEY_PREFIX,
};

/// Result of one SQL statement.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryResult {
    /// Rows with column names.
    Rows {
        /// Output column names.
        columns: Vec<String>,
        /// Output rows.
        rows: Vec<Vec<Value>>,
    },
    /// DDL/DML acknowledgement.
    Message(String),
}

/// JOIN execution strategy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JoinStrategy {
    /// Use hash join when equi-keys are available; otherwise nested loop.
    /// When both sides have `ANALYZE` statistics, compare the two join costs
    /// instead of always choosing hash.
    #[default]
    Auto,
    /// Prefer hash join when equi-keys are available.
    Hash,
    /// Always use nested loop, for benchmarks and differential tests.
    NestedLoop,
}

/// Hard cap for recursive CTE fixpoint evaluation.
const MAX_RECURSIVE_ITERATIONS: usize = 1000;
/// Hard cap on the materialized size of one recursive CTE.
const MAX_RECURSIVE_ROWS: usize = 100_000;
/// Hard cap on rows materialized by one join, so a cartesian product cannot
/// exhaust memory.
const MAX_JOIN_ROWS: usize = 1 << 20;
/// Hard cap on outer rows a correlated subquery may run for; each row executes
/// the subquery again, so this is a CPU budget rather than a size limit.
const MAX_CORRELATED_ROWS: usize = 10_000;

fn tuple_in_expr(exprs: Vec<Expr>, rows: Vec<Vec<Value>>, negated: bool) -> Expr {
    if rows.is_empty() {
        return Expr::Literal(Value::Bool(negated));
    }
    let mut combined: Option<Expr> = None;
    for row in rows {
        let mut comparison: Option<Expr> = None;
        for (expr, value) in exprs.iter().cloned().zip(row) {
            let pair = Expr::Binary {
                op: BinaryOp::Eq,
                left: Box::new(expr),
                right: Box::new(Expr::Literal(value)),
            };
            comparison = Some(match comparison {
                None => pair,
                Some(previous) => Expr::Binary {
                    op: BinaryOp::And,
                    left: Box::new(previous),
                    right: Box::new(pair),
                },
            });
        }
        let row_expr = comparison.expect("tuple has at least one expression");
        combined = Some(match combined {
            None => row_expr,
            Some(previous) => Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(previous),
                right: Box::new(row_expr),
            },
        });
    }
    let in_expr = combined.expect("non-empty rows");
    if negated {
        Expr::Not(Box::new(in_expr))
    } else {
        in_expr
    }
}

fn convert_column_value(value: Value, target: DataType) -> Result<Value, SqlError> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match (target, value) {
        (DataType::Text, Value::Int(value)) => Ok(Value::Text(value.to_string())),
        (DataType::Text, Value::Bool(value)) => Ok(Value::Text(value.to_string())),
        (DataType::Int, Value::Text(value)) => value
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| SqlError::Unsupported(format!("cannot cast {value:?} to INT"))),
        (DataType::Int, Value::Bool(value)) => Ok(Value::Int(if value { 1 } else { 0 })),
        (DataType::Bool, Value::Int(value)) => match value {
            0 => Ok(Value::Bool(false)),
            1 => Ok(Value::Bool(true)),
            _ => Err(SqlError::Unsupported(format!(
                "cannot cast {value} to BOOL"
            ))),
        },
        (DataType::Bool, Value::Text(value)) => match value.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Value::Bool(true)),
            "false" | "0" => Ok(Value::Bool(false)),
            _ => Err(SqlError::Unsupported(format!(
                "cannot cast {value:?} to BOOL"
            ))),
        },
        (target, value) => target.coerce(value).map_err(SqlError::Exec),
    }
}

fn compare_window_keys(left: &[Value], right: &[Value], order_by: &[OrderBy]) -> Ordering {
    for (index, order) in order_by.iter().enumerate() {
        let ordering = match (&left[index], &right[index]) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            (left, right) => compare_values(left, right)
                .ok()
                .flatten()
                .unwrap_or(Ordering::Equal),
        };
        if ordering != Ordering::Equal {
            return if order.ascending {
                ordering
            } else {
                ordering.reverse()
            };
        }
    }
    Ordering::Equal
}

fn expr_contains_window(expr: &Expr) -> bool {
    match expr {
        Expr::Window { .. } => true,
        Expr::Literal(_) | Expr::Column { .. } => false,
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
            expr_contains_window(expr)
        }
        Expr::Truth { expr, threshold } => {
            expr_contains_window(expr) || threshold.as_deref().is_some_and(expr_contains_window)
        }
        Expr::Certainty { expr, threshold } => {
            expr_contains_window(expr) || threshold.as_deref().is_some_and(expr_contains_window)
        }
        Expr::InList { expr, values, .. } => {
            expr_contains_window(expr) || values.iter().any(expr_contains_window)
        }
        Expr::InSubquery { expr, .. } => expr_contains_window(expr),
        Expr::InSubqueryTuple { exprs, .. } => exprs.iter().any(expr_contains_window),
        Expr::ScalarSubquery { .. } | Expr::Exists { .. } => false,
        Expr::Quantified { expr, .. } => expr_contains_window(expr),
        Expr::Tuple(exprs) => exprs.iter().any(expr_contains_window),
        Expr::Between {
            expr, low, high, ..
        } => expr_contains_window(expr) || expr_contains_window(low) || expr_contains_window(high),
        Expr::Like { expr, pattern, .. } => {
            expr_contains_window(expr) || expr_contains_window(pattern)
        }
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand.as_deref().is_some_and(expr_contains_window)
                || branches
                    .iter()
                    .any(|(when, then)| expr_contains_window(when) || expr_contains_window(then))
                || else_expr.as_deref().is_some_and(expr_contains_window)
        }
        Expr::Binary { left, right, .. } => {
            expr_contains_window(left) || expr_contains_window(right)
        }
        Expr::Aggregate { input, .. } => input.as_deref().is_some_and(expr_contains_window),
    }
}

fn quantified_expr(probe: Expr, op: BinaryOp, kind: Quantifier, values: Vec<Value>) -> Expr {
    if values.is_empty() {
        return Expr::Literal(Value::Bool(matches!(kind, Quantifier::All)));
    }
    let combine = match kind {
        Quantifier::Any => BinaryOp::Or,
        Quantifier::All => BinaryOp::And,
    };
    let mut combined: Option<Expr> = None;
    for value in values {
        let comparison = Expr::Binary {
            op,
            left: Box::new(probe.clone()),
            right: Box::new(Expr::Literal(value)),
        };
        combined = Some(match combined {
            None => comparison,
            Some(previous) => Expr::Binary {
                op: combine,
                left: Box::new(previous),
                right: Box::new(comparison),
            },
        });
    }
    combined.expect("values is non-empty")
}

fn expr_has_outer_subquery(expr: &Expr, outer: &str) -> bool {
    match expr {
        Expr::Window { .. } => false,
        Expr::Literal(_) | Expr::Column { .. } => false,
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
            expr_has_outer_subquery(expr, outer)
        }
        Expr::Truth { expr, threshold } => {
            expr_has_outer_subquery(expr, outer)
                || threshold
                    .as_deref()
                    .is_some_and(|threshold| expr_has_outer_subquery(threshold, outer))
        }
        Expr::Certainty { expr, threshold } => {
            expr_has_outer_subquery(expr, outer)
                || threshold
                    .as_deref()
                    .is_some_and(|threshold| expr_has_outer_subquery(threshold, outer))
        }
        Expr::InList { expr, values, .. } => {
            expr_has_outer_subquery(expr, outer)
                || values
                    .iter()
                    .any(|value| expr_has_outer_subquery(value, outer))
        }
        Expr::InSubquery { expr, query, .. } => {
            expr_has_outer_subquery(expr, outer) || query_has_outer_ref(query, outer)
        }
        Expr::InSubqueryTuple { exprs, query, .. } => {
            exprs
                .iter()
                .any(|expr| expr_has_outer_subquery(expr, outer))
                || query_has_outer_ref(query, outer)
        }
        Expr::Tuple(exprs) => exprs
            .iter()
            .any(|expr| expr_has_outer_subquery(expr, outer)),
        Expr::ScalarSubquery { query } => query_has_outer_ref(query, outer),
        Expr::Exists { query } => query_has_outer_ref(query, outer),
        Expr::Quantified { expr, query, .. } => {
            expr_has_outer_subquery(expr, outer) || query_has_outer_ref(query, outer)
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            expr_has_outer_subquery(expr, outer)
                || expr_has_outer_subquery(low, outer)
                || expr_has_outer_subquery(high, outer)
        }
        Expr::Like { expr, pattern, .. } => {
            expr_has_outer_subquery(expr, outer) || expr_has_outer_subquery(pattern, outer)
        }
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand
                .as_deref()
                .is_some_and(|expr| expr_has_outer_subquery(expr, outer))
                || branches.iter().any(|(when, then)| {
                    expr_has_outer_subquery(when, outer) || expr_has_outer_subquery(then, outer)
                })
                || else_expr
                    .as_deref()
                    .is_some_and(|expr| expr_has_outer_subquery(expr, outer))
        }
        Expr::Binary { left, right, .. } => {
            expr_has_outer_subquery(left, outer) || expr_has_outer_subquery(right, outer)
        }
        Expr::Aggregate { input, .. } => input
            .as_deref()
            .is_some_and(|expr| expr_has_outer_subquery(expr, outer)),
    }
}

fn query_has_outer_ref(query: &Query, outer: &str) -> bool {
    match query {
        Query::Select(select) => select_has_outer_ref(select, outer),
        Query::With(_) => false,
    }
}

fn select_has_outer_ref(select: &Select, outer: &str) -> bool {
    select.items.iter().any(|item| match item {
        SelectItem::Wildcard => false,
        SelectItem::Expr { expr, .. } => expr_mentions_outer(expr, outer),
    }) || select
        .filter
        .as_ref()
        .is_some_and(|expr| expr_mentions_outer(expr, outer))
        || select.joins.iter().any(|join| match &join.condition {
            JoinCondition::On(expr) => expr_mentions_outer(expr, outer),
            _ => false,
        })
        || select
            .group_by
            .iter()
            .any(|expr| expr_mentions_outer(expr, outer))
        || select
            .having
            .as_ref()
            .is_some_and(|expr| expr_mentions_outer(expr, outer))
        || select
            .order_by
            .iter()
            .any(|order| expr_mentions_outer(&order.expr, outer))
}

fn expr_mentions_outer(expr: &Expr, outer: &str) -> bool {
    match expr {
        Expr::Window { .. } => false,
        Expr::Column {
            table: Some(qualifier),
            ..
        } => qualifier == outer,
        Expr::Column { table: None, .. } | Expr::Literal(_) => false,
        Expr::InSubquery { expr, query, .. } => {
            expr_mentions_outer(expr, outer) || query_has_outer_ref(query, outer)
        }
        Expr::InSubqueryTuple { exprs, query, .. } => {
            exprs.iter().any(|expr| expr_mentions_outer(expr, outer))
                || query_has_outer_ref(query, outer)
        }
        Expr::Tuple(exprs) => exprs.iter().any(|expr| expr_mentions_outer(expr, outer)),
        Expr::ScalarSubquery { query } => query_has_outer_ref(query, outer),
        Expr::Exists { query } => query_has_outer_ref(query, outer),
        Expr::Quantified { expr, query, .. } => {
            expr_mentions_outer(expr, outer) || query_has_outer_ref(query, outer)
        }
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
            expr_mentions_outer(expr, outer)
        }
        Expr::Truth { expr, threshold } => {
            expr_mentions_outer(expr, outer)
                || threshold
                    .as_deref()
                    .is_some_and(|threshold| expr_mentions_outer(threshold, outer))
        }
        Expr::Certainty { expr, threshold } => {
            expr_mentions_outer(expr, outer)
                || threshold
                    .as_deref()
                    .is_some_and(|threshold| expr_mentions_outer(threshold, outer))
        }
        Expr::InList { expr, values, .. } => {
            expr_mentions_outer(expr, outer)
                || values.iter().any(|value| expr_mentions_outer(value, outer))
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            expr_mentions_outer(expr, outer)
                || expr_mentions_outer(low, outer)
                || expr_mentions_outer(high, outer)
        }
        Expr::Like { expr, pattern, .. } => {
            expr_mentions_outer(expr, outer) || expr_mentions_outer(pattern, outer)
        }
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand
                .as_deref()
                .is_some_and(|expr| expr_mentions_outer(expr, outer))
                || branches.iter().any(|(when, then)| {
                    expr_mentions_outer(when, outer) || expr_mentions_outer(then, outer)
                })
                || else_expr
                    .as_deref()
                    .is_some_and(|expr| expr_mentions_outer(expr, outer))
        }
        Expr::Binary { left, right, .. } => {
            expr_mentions_outer(left, outer) || expr_mentions_outer(right, outer)
        }
        Expr::Aggregate { input, .. } => input
            .as_deref()
            .is_some_and(|expr| expr_mentions_outer(expr, outer)),
    }
}

fn rows_of(result: QueryResult) -> Result<(Vec<String>, Vec<Row>), SqlError> {
    match result {
        QueryResult::Rows { columns, rows } => Ok((columns, rows)),
        QueryResult::Message(message) => Err(SqlError::Unsupported(format!(
            "WITH query returned a message: {message}"
        ))),
    }
}

fn infer_schema(names: &[String], rows: &[Row]) -> Schema {
    let columns = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let data_type = rows
                .iter()
                .find_map(|row| match row.get(index) {
                    Some(Value::Null) | None => None,
                    Some(value) => Some(value_data_type(value)),
                })
                .unwrap_or(DataType::Text);
            Column::new(name.clone(), data_type)
        })
        .collect();
    Schema::new(columns)
}

/// Exact result type for the shapes [`SqlEngine::describe_select`]
/// fast-paths; `None` means "probe by execution instead".
fn group_expr_type(expr: &GroupExpr, schema: &Schema) -> Option<DataType> {
    match expr {
        GroupExpr::Scalar(ExecExpr::Column(index)) => {
            schema.columns.get(*index).map(|column| column.data_type)
        }
        GroupExpr::Scalar(ExecExpr::Literal(value)) => Some(value_data_type(value)),
        GroupExpr::Aggregate { kind, input } => match kind {
            AggregateKind::Count | AggregateKind::Sum | AggregateKind::Avg => Some(DataType::Int),
            AggregateKind::ProbSum => Some(DataType::Lukasiewicz),
            AggregateKind::Min | AggregateKind::Max => match input.as_ref() {
                Some(ExecExpr::Column(index)) => {
                    schema.columns.get(*index).map(|column| column.data_type)
                }
                Some(ExecExpr::Literal(value)) => Some(value_data_type(value)),
                _ => None,
            },
        },
        _ => None,
    }
}

fn value_data_type(value: &Value) -> DataType {
    match value {
        Value::Null => DataType::Text,
        Value::Int(_) => DataType::Int,
        Value::Text(_) => DataType::Text,
        Value::Bool(_) => DataType::Bool,
        Value::Logic(B4::Both) | Value::Logic(B4::Neither) => DataType::Belnap,
        Value::Logic(_) => DataType::Logic,
        Value::Lukasiewicz(_) => DataType::Lukasiewicz,
        Value::Nat(_) => DataType::Nat,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplainFormat {
    Text,
    Sexpr,
}

/// Runtime counters attached to an `EXPLAIN ANALYZE` plan.
#[derive(Debug, Default)]
struct ExecutionStats {
    scan_rows: Vec<usize>,
    scan_filters: Vec<bool>,
    join_rows: Vec<usize>,
    join_ms: Vec<f64>,
    join_uses_hash: Vec<bool>,
    filter_rows: usize,
    filter_ms: f64,
    aggregate_groups: usize,
    aggregate_ms: f64,
    having_groups: usize,
    having_ms: f64,
    project_rows: usize,
    project_ms: f64,
    sort_rows: usize,
    sort_ms: f64,
    output_rows: usize,
    total_ms: f64,
}

/// Cardinality estimates attached to a static `EXPLAIN` plan.
#[derive(Debug, Default)]
struct PlanEstimate {
    scan_rows: Vec<u64>,
    join_rows: Vec<u64>,
    join_uses_hash: Vec<bool>,
    filter_rows: u64,
    aggregate_groups: u64,
    having_groups: u64,
    project_rows: u64,
    sort_rows: u64,
    output_rows: u64,
}

/// Source-local `WHERE` conjuncts moved to source scans.
#[derive(Clone, Debug)]
struct PushdownPlan {
    /// Combined predicate for each source in base-table + join order.
    ///
    /// Multiple conjuncts assigned to the same source are joined with `AND`.
    filters: Vec<Option<Expr>>,
    /// Part of the original filter that remains above the join.
    residual: Option<Expr>,
}

/// Columns retained for each source before joining.
///
/// `None` means the source cannot be pruned safely; `Some(columns)` is a subset
/// in schema order, and an empty vector means the source only contributes row
/// multiplicity. This is a conservative physical optimization: every column
/// reference must be static, an unqualified name must match exactly one source,
/// and no wildcard, `USING`, or `NATURAL` join may depend on the original
/// schema.
#[derive(Clone, Debug, Default)]
struct ScanProjection {
    columns: Vec<Option<Vec<String>>>,
}

impl ScanProjection {
    fn columns(&self, index: usize) -> Option<&[String]> {
        self.columns
            .get(index)
            .and_then(|columns| columns.as_deref())
    }

    fn all(count: usize) -> Self {
        Self {
            columns: vec![None; count],
        }
    }
}

/// NDV inputs for one join cost decision.
#[derive(Clone, Copy)]
struct JoinStats<'a> {
    left_ndvs: &'a [u64],
    right_ndvs: &'a [u64],
    available: bool,
}

/// A SQL engine backed by one [`TxnStore`].
///
/// M1 keeps the whole table in memory and rewrites the table value on every
/// mutation. This is a semantic spike, not a buffer pool or a heap file.
pub struct SqlEngine {
    store: TxnStore,
    tables: BTreeMap<String, Table>,
    stats: BTreeMap<String, TableStats>,
    join_strategy: JoinStrategy,
    /// Deadline checked while scanning rows; `None` means "no limit".
    deadline: std::sync::RwLock<Option<Instant>>,
}

impl SqlEngine {
    /// Builds a table-not-found error with the closest existing table names.
    fn unknown_table(&self, name: &str) -> SqlError {
        SqlError::UnknownName {
            kind: NameKind::Table,
            name: name.to_string(),
            candidates: closest_names(name, self.tables.keys().map(String::as_str)),
        }
    }

    /// Borrows one table so `COPY ... TO STDOUT` can stream rows instead of
    /// materializing a full query result.
    pub fn copy_source(&self, name: &str) -> Option<&Table> {
        self.tables.get(name)
    }

    /// Returns the column names and types of `name`, if the table exists.
    pub fn table_schema(&self, name: &str) -> Option<Vec<(String, DataType)>> {
        self.tables.get(name).map(|table| {
            table
                .schema
                .columns
                .iter()
                .map(|column| (column.name.clone(), column.data_type))
                .collect()
        })
    }

    /// Result column names and types for a single-table `SELECT`, computed by
    /// binding only. Returns `None` for shapes this fast path does not cover
    /// (joins, non-SELECT, or expressions whose type is not exact), so callers
    /// can fall back to probing with `LIMIT 1`.
    pub fn describe_select(&self, sql: &str) -> Option<Vec<(String, DataType)>> {
        let statements = parse_script(sql).ok()?;
        let Some(Statement::Select(select)) = statements.first() else {
            return None;
        };
        if !select.joins.is_empty() {
            return None;
        }
        let table = self.tables.get(&select.table)?;
        let bound = bind_select(select, table).ok()?;
        bound
            .items
            .iter()
            .map(|item| {
                group_expr_type(&item.expr, &table.schema)
                    .map(|data_type| (item.name.clone(), data_type))
            })
            .collect()
    }

    /// Opens an engine and loads all persisted tables.
    pub fn open(path: &Path) -> Result<Self, SqlError> {
        let store = TxnStore::open(path)?;
        let mut tables = BTreeMap::new();
        let mut stats_bytes = BTreeMap::new();
        for (key, value) in store.iter()? {
            if let Some(name_bytes) = key.strip_prefix(TABLE_KEY_PREFIX.as_bytes()) {
                let name = str::from_utf8(name_bytes).map_err(|error| SqlError::CorruptTable {
                    table: "<non-utf8>".to_string(),
                    reason: error.to_string(),
                })?;
                if name.is_empty() {
                    return Err(SqlError::CorruptTable {
                        table: "<empty>".to_string(),
                        reason: "empty table name".to_string(),
                    });
                }
                let table = decode_table(name, &value)?;
                tables.insert(name.to_string(), table);
                continue;
            }
            if let Some(name_bytes) = key.strip_prefix(STATS_KEY_PREFIX.as_bytes()) {
                let name = str::from_utf8(name_bytes).map_err(|error| SqlError::CorruptStats {
                    table: "<non-utf8>".to_string(),
                    reason: error.to_string(),
                })?;
                if name.is_empty() {
                    return Err(SqlError::CorruptStats {
                        table: "<empty>".to_string(),
                        reason: "empty table name".to_string(),
                    });
                }
                stats_bytes.insert(name.to_string(), value);
            }
        }
        let mut stats = BTreeMap::new();
        for (name, value) in stats_bytes {
            // Statistics are derived metadata. An orphaned stats key is ignored
            // rather than making a database with otherwise valid tables unopenable.
            if let Some(table) = tables.get(&name) {
                stats.insert(name, decode_stats(table, &value)?);
            }
        }
        Ok(Self {
            store,
            tables,
            stats,
            join_strategy: JoinStrategy::Auto,
            deadline: std::sync::RwLock::new(None),
        })
    }

    /// WAL path.
    pub fn path(&self) -> &Path {
        self.store.path()
    }

    /// Current JOIN strategy.
    pub const fn join_strategy(&self) -> JoinStrategy {
        self.join_strategy
    }

    /// Overrides the JOIN strategy for benchmarks or differential tests.
    pub fn set_join_strategy(&mut self, strategy: JoinStrategy) {
        self.join_strategy = strategy;
    }

    /// Sets a deadline checked while scanning rows (`None` clears it).
    ///
    /// A query that runs past the deadline returns
    /// `Unsupported("query cancelled: ...")` instead of holding the engine
    /// lock until it finishes.
    pub fn set_deadline(&self, deadline: Option<Instant>) {
        *self
            .deadline
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = deadline;
    }

    /// Current scan deadline, if any.
    pub fn deadline(&self) -> Option<Instant> {
        *self
            .deadline
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Executes a SQL script and returns one result per statement.
    pub fn execute_script(&mut self, sql: &str) -> Result<Vec<QueryResult>, SqlError> {
        let statements = parse_script(sql)?;
        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            results.push(self.execute_statement(&statement)?);
        }
        Ok(results)
    }

    /// Executes one parsed statement.
    pub fn execute_statement(&mut self, statement: &Statement) -> Result<QueryResult, SqlError> {
        match statement {
            Statement::CreateTable {
                name,
                if_not_exists,
                columns,
                unique,
                checks,
            } => self.create_table(name, *if_not_exists, columns, unique, checks),
            Statement::AlterTableAddColumn {
                name,
                if_not_exists,
                column,
            } => self.alter_table_add_column(name, *if_not_exists, column),
            Statement::AlterTableDropColumn {
                name,
                if_exists,
                column,
            } => self.alter_table_drop_column(name, *if_exists, column),
            Statement::AlterTableRenameColumn { name, from, to } => {
                self.alter_table_rename_column(name, from, to)
            }
            Statement::RenameTable { from, to } => self.rename_table(from, to),
            Statement::AlterColumnType {
                table,
                column,
                data_type,
            } => self.alter_column_type(table, column, *data_type),
            Statement::DropTable { name, if_exists } => self.drop_table(name, *if_exists),
            Statement::Insert {
                table,
                columns,
                rows,
                select,
            } => {
                let mut rows = rows.clone();
                for row in &mut rows {
                    for value in row {
                        self.materialize_expr(value, "")?;
                    }
                }
                self.insert(table, columns.as_deref(), &rows, select.as_deref())
            }
            Statement::CountModels { expr, vars } => self.count_models(expr, *vars),
            Statement::Select(select) => {
                let mut select = select.clone();
                self.materialize_subqueries(&mut select)?;
                self.select(&select)
            }
            Statement::With(with) => self.with_query(with),
            Statement::Explain(select) => {
                let mut select = select.clone();
                self.materialize_subqueries(&mut select)?;
                self.explain(&select, false, ExplainFormat::Text)
            }
            Statement::ExplainSexpr(select) => {
                let mut select = select.clone();
                self.materialize_subqueries(&mut select)?;
                self.explain(&select, false, ExplainFormat::Sexpr)
            }
            Statement::ExplainAnalyze(select) => {
                let mut select = select.clone();
                self.materialize_subqueries(&mut select)?;
                self.explain(&select, true, ExplainFormat::Text)
            }
            Statement::ExplainAnalyzeSexpr(select) => {
                let mut select = select.clone();
                self.materialize_subqueries(&mut select)?;
                self.explain(&select, true, ExplainFormat::Sexpr)
            }
            Statement::Analyze { table } => self.analyze(table.as_deref()),
            Statement::Delete { table, filter } => {
                let mut filter = filter.clone();
                if let Some(filter) = &mut filter {
                    self.materialize_expr(filter, table)?;
                }
                self.delete(table, filter.as_ref())
            }
            Statement::Update {
                table,
                assignments,
                filter,
            } => {
                let mut assignments = assignments.clone();
                for assignment in &mut assignments {
                    self.materialize_expr(&mut assignment.value, table)?;
                }
                let mut filter = filter.clone();
                if let Some(filter) = &mut filter {
                    self.materialize_expr(filter, table)?;
                }
                self.update(table, &assignments, filter.as_ref())
            }
        }
    }

    fn create_table(
        &mut self,
        name: &str,
        if_not_exists: bool,
        definitions: &[crate::ast::ColumnDef],
        unique_sets: &[Vec<String>],
        checks: &[String],
    ) -> Result<QueryResult, SqlError> {
        if self.tables.contains_key(name) {
            if if_not_exists {
                return Ok(QueryResult::Message(format!("table {name} already exists")));
            }
            return Err(SqlError::TableExists(name.to_string()));
        }
        if definitions.is_empty() {
            return Err(SqlError::Unsupported(
                "table must have at least one column".to_string(),
            ));
        }
        let mut columns = Vec::with_capacity(definitions.len());
        let mut seen = BTreeMap::new();
        for definition in definitions {
            if seen.insert(definition.name.clone(), ()).is_some() {
                return Err(SqlError::DuplicateColumn(definition.name.clone()));
            }
            columns.push(Column {
                name: definition.name.clone(),
                data_type: definition.data_type,
                not_null: definition.not_null,
                unique: definition.unique,
                check: definition.check.clone(),
            });
        }
        let mut unique_indices = Vec::with_capacity(unique_sets.len());
        for set in unique_sets {
            let mut indices = Vec::with_capacity(set.len());
            for column in set {
                let index = columns
                    .iter()
                    .position(|candidate| candidate.name == *column)
                    .ok_or_else(|| SqlError::UnknownColumn(column.clone()))?;
                indices.push(index);
            }
            unique_indices.push(indices);
        }
        let mut schema = Schema::new(columns);
        schema.unique_sets = unique_indices;
        schema.checks = checks.to_vec();
        let table = Table {
            name: name.to_string(),
            schema,
            rows: Vec::new(),
        };
        self.put_table(table)?;
        Ok(QueryResult::Message(format!("created table {name}")))
    }

    fn alter_table_add_column(
        &mut self,
        name: &str,
        if_not_exists: bool,
        definition: &crate::ast::ColumnDef,
    ) -> Result<QueryResult, SqlError> {
        let Some(existing) = self.tables.get(name) else {
            return Err(self.unknown_table(name));
        };
        if existing
            .schema
            .columns
            .iter()
            .any(|column| column.name == definition.name)
        {
            if if_not_exists {
                return Ok(QueryResult::Message(format!(
                    "column {} already exists in table {name}",
                    definition.name
                )));
            }
            return Err(SqlError::DuplicateColumn(definition.name.clone()));
        }
        if definition.not_null {
            return Err(SqlError::Unsupported(
                "ALTER TABLE ADD COLUMN does not support NOT NULL without a default".to_string(),
            ));
        }
        let mut table = existing.clone();
        let mut columns = table.schema.columns.clone();
        columns.push(Column {
            name: definition.name.clone(),
            data_type: definition.data_type,
            not_null: false,
            unique: definition.unique,
            check: definition.check.clone(),
        });
        table.schema = Schema::new(columns);
        for row in &mut table.rows {
            row.push(Value::Null);
        }
        self.put_table(table)?;
        Ok(QueryResult::Message(format!(
            "added column {} to table {name}",
            definition.name
        )))
    }

    fn alter_table_drop_column(
        &mut self,
        name: &str,
        if_exists: bool,
        column_name: &str,
    ) -> Result<QueryResult, SqlError> {
        let Some(existing) = self.tables.get(name) else {
            return Err(self.unknown_table(name));
        };
        let Some(index) = existing.schema.index_of(column_name) else {
            if if_exists {
                return Ok(QueryResult::Message(format!(
                    "column {column_name} does not exist in table {name}"
                )));
            }
            return Err(SqlError::UnknownColumn(column_name.to_string()));
        };
        if existing.schema.len() <= 1 {
            return Err(SqlError::Unsupported(
                "cannot drop the last column of a table".to_string(),
            ));
        }
        let mut table = existing.clone();
        let mut schema = table.schema.clone();
        schema.columns.remove(index);
        for set in &mut schema.unique_sets {
            set.retain(|&column| column != index);
            for column in set.iter_mut() {
                if *column > index {
                    *column -= 1;
                }
            }
        }
        schema.unique_sets.retain(|set| !set.is_empty());
        table.schema = schema;
        for row in &mut table.rows {
            row.remove(index);
        }
        self.put_table(table)?;
        self.stats.remove(name);
        Ok(QueryResult::Message(format!(
            "dropped column {column_name} from table {name}"
        )))
    }

    fn alter_table_rename_column(
        &mut self,
        name: &str,
        from: &str,
        to: &str,
    ) -> Result<QueryResult, SqlError> {
        let Some(existing) = self.tables.get(name) else {
            return Err(self.unknown_table(name));
        };
        let Some(index) = existing.schema.index_of(from) else {
            return Err(SqlError::UnknownColumn(from.to_string()));
        };
        if existing.schema.index_of(to).is_some() {
            return Err(SqlError::DuplicateColumn(to.to_string()));
        }
        let mut table = existing.clone();
        let mut schema = table.schema.clone();
        schema.columns[index].name = to.to_string();
        table.schema = schema;
        self.put_table(table)?;
        self.stats.remove(name);
        Ok(QueryResult::Message(format!(
            "renamed column {from} to {to} in table {name}"
        )))
    }

    fn rename_table(&mut self, from: &str, to: &str) -> Result<QueryResult, SqlError> {
        let Some(existing) = self.tables.get(from) else {
            return Err(self.unknown_table(from));
        };
        if self.tables.contains_key(to) {
            return Err(SqlError::TableExists(to.to_string()));
        }
        let mut table = existing.clone();
        table.name = to.to_string();
        let value = encode_table(&table)?;
        let mut txn = self.store.begin()?;
        txn.delete(&table_key(from))?;
        txn.delete(&stats_key(from))?;
        txn.put(&table_key(to), &value)?;
        txn.commit()?;
        self.tables.remove(from);
        self.stats.remove(from);
        self.tables.insert(to.to_string(), table);
        Ok(QueryResult::Message(format!(
            "renamed table {from} to {to}"
        )))
    }

    fn alter_column_type(
        &mut self,
        table_name: &str,
        column_name: &str,
        data_type: DataType,
    ) -> Result<QueryResult, SqlError> {
        let Some(existing) = self.tables.get(table_name) else {
            return Err(self.unknown_table(table_name));
        };
        let Some(index) = existing.schema.index_of(column_name) else {
            return Err(SqlError::UnknownColumn(column_name.to_string()));
        };
        let mut table = existing.clone();
        let mut schema = table.schema.clone();
        if schema.columns[index].data_type == data_type {
            return Ok(QueryResult::Message(format!(
                "column {column_name} already has type {}",
                data_type.name()
            )));
        }
        let mut rows = table.rows.clone();
        for row in &mut rows {
            let value = std::mem::replace(&mut row[index], Value::Null);
            row[index] = convert_column_value(value, data_type)?;
        }
        schema.columns[index].data_type = data_type;
        table.schema = schema;
        table.rows = rows;
        self.put_table(table)?;
        Ok(QueryResult::Message(format!(
            "altered column {column_name} to type {}",
            data_type.name()
        )))
    }

    fn drop_table(&mut self, name: &str, if_exists: bool) -> Result<QueryResult, SqlError> {
        if !self.tables.contains_key(name) {
            if if_exists {
                return Ok(QueryResult::Message(format!("table {name} does not exist")));
            }
            return Err(self.unknown_table(name));
        }
        let key = table_key(name);
        let stats = stats_key(name);
        let mut txn = self.store.begin()?;
        txn.delete(&key)?;
        txn.delete(&stats)?;
        txn.commit()?;
        self.tables.remove(name);
        self.stats.remove(name);
        Ok(QueryResult::Message(format!("dropped table {name}")))
    }

    fn insert(
        &mut self,
        name: &str,
        target_columns: Option<&[String]>,
        rows: &[Vec<crate::ast::Expr>],
        select: Option<&Query>,
    ) -> Result<QueryResult, SqlError> {
        let mut table = self
            .tables
            .get(name)
            .cloned()
            .ok_or_else(|| self.unknown_table(name))?;
        let schema = &table.schema;
        let indices = match target_columns {
            Some(names) => {
                let mut indices = Vec::with_capacity(names.len());
                let mut seen = BTreeMap::new();
                for column_name in names {
                    if seen.insert(column_name.clone(), ()).is_some() {
                        return Err(SqlError::DuplicateColumn(column_name.clone()));
                    }
                    let index = schema
                        .index_of(column_name)
                        .ok_or_else(|| SqlError::UnknownColumn(column_name.clone()))?;
                    indices.push(index);
                }
                indices
            }
            None => (0..schema.len()).collect(),
        };

        let mut new_rows = Vec::new();
        if let Some(query) = select {
            let result = self.execute_query(query)?;
            let QueryResult::Rows {
                columns,
                rows: value_rows,
            } = result
            else {
                return Err(SqlError::Unsupported(
                    "INSERT source must return rows".to_string(),
                ));
            };
            if columns.len() != indices.len() {
                return Err(SqlError::ColumnCount {
                    table: name.to_string(),
                    expected: indices.len(),
                    actual: columns.len(),
                });
            }
            for values in value_rows {
                if values.len() != indices.len() {
                    return Err(SqlError::ColumnCount {
                        table: name.to_string(),
                        expected: indices.len(),
                        actual: values.len(),
                    });
                }
                let mut row = vec![Value::Null; table.schema.len()];
                for (value, &index) in values.into_iter().zip(&indices) {
                    let value = table.schema.columns[index].data_type.coerce(value)?;
                    if table.schema.columns[index].not_null && value.is_null() {
                        return Err(SqlError::NotNull {
                            table: name.to_string(),
                            column: table.schema.columns[index].name.clone(),
                        });
                    }
                    row[index] = value;
                }
                new_rows.push(row);
            }
        } else {
            new_rows.reserve(rows.len());
            for expressions in rows {
                if expressions.len() != indices.len() {
                    return Err(SqlError::ColumnCount {
                        table: name.to_string(),
                        expected: indices.len(),
                        actual: expressions.len(),
                    });
                }
                let mut row = vec![Value::Null; schema.len()];
                for (expression, &index) in expressions.iter().zip(&indices) {
                    let value = eval_constant(expression)?;
                    let value = schema.columns[index].data_type.coerce(value)?;
                    if schema.columns[index].not_null && value.is_null() {
                        return Err(SqlError::NotNull {
                            table: name.to_string(),
                            column: schema.columns[index].name.clone(),
                        });
                    }
                    row[index] = value;
                }
                for (index, value) in row.iter().enumerate() {
                    if value.is_null() && schema.columns[index].not_null {
                        return Err(SqlError::NotNull {
                            table: name.to_string(),
                            column: schema.columns[index].name.clone(),
                        });
                    }
                }
                new_rows.push(row);
            }
        }

        for row in &new_rows {
            check_row(&table.schema, &table.name, row)?;
        }
        let inserted = new_rows.len();
        table.rows.extend(new_rows);
        check_unique(&table)?;
        self.put_table(table)?;
        Ok(QueryResult::Message(format!(
            "inserted {inserted} row(s) into {name}"
        )))
    }
    fn delete(
        &mut self,
        name: &str,
        filter: Option<&crate::ast::Expr>,
    ) -> Result<QueryResult, SqlError> {
        let mut table = self
            .tables
            .get(name)
            .cloned()
            .ok_or_else(|| self.unknown_table(name))?;
        let filter = filter
            .map(|expr| bind_expr(expr, &table.schema))
            .transpose()?;
        let batch_passing = filter
            .as_ref()
            .and_then(|expr| batch_filter_indices(expr, &table.schema, &table.rows));
        #[cfg(debug_assertions)]
        if let Some(indices) = &batch_passing {
            let scalar = scalar_filter_indices(&filter, &table.schema, &table.rows)?;
            debug_assert_eq!(indices, &scalar, "batch WHERE diverged from the row path");
        }
        let mut kept = Vec::with_capacity(table.rows.len());
        let mut deleted = 0usize;
        match &batch_passing {
            Some(passing) => {
                let passing: std::collections::BTreeSet<usize> = passing.iter().copied().collect();
                let mut rows = std::mem::take(&mut table.rows);
                for (index, row) in rows.drain(..).enumerate() {
                    if passing.contains(&index) {
                        deleted += 1;
                    } else {
                        kept.push(row);
                    }
                }
            }
            None => {
                for row in table.rows.drain(..) {
                    let keep = match &filter {
                        Some(expr) => {
                            truth_of(&eval(expr, &table.schema, &row)?)? != TruthValue::True
                        }
                        None => false,
                    };
                    if keep {
                        kept.push(row);
                    } else {
                        deleted += 1;
                    }
                }
            }
        }
        table.rows = kept;
        if deleted > 0 {
            self.put_table(table)?;
        }
        Ok(QueryResult::Message(format!(
            "deleted {deleted} row(s) from {name}"
        )))
    }

    fn update(
        &mut self,
        name: &str,
        assignments: &[Assignment],
        filter: Option<&crate::ast::Expr>,
    ) -> Result<QueryResult, SqlError> {
        let mut table = self
            .tables
            .get(name)
            .cloned()
            .ok_or_else(|| self.unknown_table(name))?;
        let schema = &table.schema;
        let mut targets = Vec::with_capacity(assignments.len());
        let mut seen = BTreeMap::new();
        for assignment in assignments {
            if seen.insert(assignment.column.clone(), ()).is_some() {
                return Err(SqlError::DuplicateColumn(assignment.column.clone()));
            }
            let index = schema
                .index_of(&assignment.column)
                .ok_or_else(|| SqlError::UnknownColumn(assignment.column.clone()))?;
            targets.push((index, bind_expr(&assignment.value, schema)?));
        }
        let filter = filter.map(|expr| bind_expr(expr, schema)).transpose()?;
        let batch_passing = filter
            .as_ref()
            .and_then(|expr| batch_filter_indices(expr, schema, &table.rows));
        #[cfg(debug_assertions)]
        if let Some(indices) = &batch_passing {
            let scalar = scalar_filter_indices(&filter, schema, &table.rows)?;
            debug_assert_eq!(indices, &scalar, "batch WHERE diverged from the row path");
        }

        let mut updated = 0usize;
        for (row_index, row) in table.rows.iter_mut().enumerate() {
            let passes = match &batch_passing {
                Some(indices) => indices.binary_search(&row_index).is_ok(),
                None => passes_filter(filter.as_ref(), schema, row)?,
            };
            if !passes {
                continue;
            }
            let old = row.clone();
            let mut values = Vec::with_capacity(targets.len());
            for (index, expr) in &targets {
                let value = eval(expr, schema, &old)?;
                let value = schema.columns[*index].data_type.coerce(value)?;
                if schema.columns[*index].not_null && value.is_null() {
                    return Err(SqlError::NotNull {
                        table: name.to_string(),
                        column: schema.columns[*index].name.clone(),
                    });
                }
                values.push((*index, value));
            }
            for (index, value) in values {
                row[index] = value;
            }
            check_row(schema, name, row)?;
            updated += 1;
        }
        if updated > 0 {
            check_unique(&table)?;
            self.put_table(table)?;
        }
        Ok(QueryResult::Message(format!(
            "updated {updated} row(s) in {name}"
        )))
    }

    fn count_models(&self, expr: &str, vars: usize) -> Result<QueryResult, SqlError> {
        let (parsed, inferred) = tritdb_exec::ann::parse_bool_expr(expr)
            .map_err(|error| SqlError::Unsupported(error.to_string()))?;
        let vars = if vars == 0 { inferred } else { vars };
        if inferred > vars {
            return Err(SqlError::Unsupported(format!(
                "expression uses {inferred} variables, only {vars} declared"
            )));
        }
        let models = tritdb_exec::ann::count_models(&parsed, vars)
            .map_err(|error| SqlError::Unsupported(error.to_string()))?;
        Ok(QueryResult::Rows {
            columns: vec!["models".to_string()],
            rows: vec![vec![Value::Int(models as i64)]],
        })
    }

    fn execute_query(&mut self, query: &Query) -> Result<QueryResult, SqlError> {
        match query {
            Query::Select(select) => {
                let mut select = select.as_ref().clone();
                self.materialize_subqueries(&mut select)?;
                self.select(&select)
            }
            Query::With(with) => self.with_query(with),
        }
    }

    /// Replaces `IN (SELECT ...)` and scalar subqueries with literal
    /// expressions, so the binder and executor never see a subquery.
    ///
    /// Only uncorrelated subqueries are supported: a subquery that references
    /// an outer column fails to resolve and reports `UnknownColumn`.
    fn materialize_subqueries(&mut self, select: &mut Select) -> Result<(), SqlError> {
        let outer = select
            .table_alias
            .as_deref()
            .unwrap_or(&select.table)
            .to_string();
        for item in &mut select.items {
            if let SelectItem::Expr { expr, .. } = item {
                self.materialize_expr(expr, &outer)?;
            }
        }
        for join in &mut select.joins {
            if let JoinCondition::On(expr) = &mut join.condition {
                self.materialize_expr(expr, &outer)?;
            }
        }
        if let Some(filter) = &mut select.filter {
            self.materialize_expr(filter, &outer)?;
        }
        for expr in &mut select.group_by {
            self.materialize_expr(expr, &outer)?;
        }
        if let Some(having) = &mut select.having {
            self.materialize_expr(having, &outer)?;
        }
        for order in &mut select.order_by {
            self.materialize_expr(&mut order.expr, &outer)?;
        }
        Ok(())
    }

    fn materialize_expr(&mut self, expr: &mut Expr, outer: &str) -> Result<(), SqlError> {
        match expr {
            Expr::Window { .. } => Ok(()),
            Expr::Literal(_) | Expr::Column { .. } => Ok(()),
            Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
                self.materialize_expr(expr, outer)
            }
            Expr::Truth { expr, threshold } => {
                self.materialize_expr(expr, outer)?;
                if let Some(threshold) = threshold {
                    self.materialize_expr(threshold, outer)?;
                }
                Ok(())
            }
            Expr::Certainty { expr, threshold } => {
                self.materialize_expr(expr, outer)?;
                if let Some(threshold) = threshold {
                    self.materialize_expr(threshold, outer)?;
                }
                Ok(())
            }
            Expr::InList { expr, values, .. } => {
                self.materialize_expr(expr, outer)?;
                for value in values {
                    self.materialize_expr(value, outer)?;
                }
                Ok(())
            }
            Expr::Tuple(exprs) => {
                for expr in exprs {
                    self.materialize_expr(expr, outer)?;
                }
                Ok(())
            }
            Expr::InSubqueryTuple { .. } => {
                let (mut exprs, query, negated) =
                    match std::mem::replace(expr, Expr::Literal(Value::Null)) {
                        Expr::InSubqueryTuple {
                            exprs,
                            query,
                            negated,
                        } => (exprs, query, negated),
                        _ => unreachable!("replaced expression was not tuple IN subquery"),
                    };
                if query_has_outer_ref(&query, outer) {
                    *expr = Expr::InSubqueryTuple {
                        exprs,
                        query,
                        negated,
                    };
                    return Ok(());
                }
                for expr in &mut exprs {
                    self.materialize_expr(expr, outer)?;
                }
                let rows = self.evaluate_subquery_rows(&query, exprs.len())?;
                *expr = tuple_in_expr(exprs, rows, negated);
                Ok(())
            }
            Expr::InSubquery { query, negated, .. } => {
                if query_has_outer_ref(query, outer) {
                    return Ok(());
                }
                let values = self.evaluate_subquery_column(query)?;
                let negated = *negated;
                let probe = match std::mem::replace(expr, Expr::Literal(Value::Null)) {
                    Expr::InSubquery { expr: probe, .. } => probe,
                    _ => unreachable!("replaced expression was not IN subquery"),
                };
                let mut probe = *probe;
                self.materialize_expr(&mut probe, outer)?;
                *expr = Expr::InList {
                    expr: Box::new(probe),
                    values,
                    negated,
                };
                Ok(())
            }
            Expr::ScalarSubquery { query } => {
                if query_has_outer_ref(query, outer) {
                    return Ok(());
                }
                let value = self.evaluate_scalar_subquery(query)?;
                *expr = Expr::Literal(value);
                Ok(())
            }
            Expr::Exists { query } => {
                if query_has_outer_ref(query, outer) {
                    return Ok(());
                }
                let result = self.execute_query(query)?;
                let exists = matches!(&result, QueryResult::Rows { rows, .. } if !rows.is_empty());
                *expr = Expr::Literal(Value::Bool(exists));
                Ok(())
            }
            Expr::Quantified { .. } => {
                let (mut probe, op, kind, query) =
                    match std::mem::replace(expr, Expr::Literal(Value::Null)) {
                        Expr::Quantified {
                            expr,
                            op,
                            kind,
                            query,
                        } => (expr, op, kind, query),
                        _ => unreachable!("replaced expression was not quantified"),
                    };
                if query_has_outer_ref(&query, outer) {
                    *expr = Expr::Quantified {
                        expr: probe,
                        op,
                        kind,
                        query,
                    };
                    return Ok(());
                }
                self.materialize_expr(&mut probe, outer)?;
                let values = self.evaluate_quantified_column(&query)?;
                *expr = quantified_expr(*probe, op, kind, values);
                Ok(())
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.materialize_expr(expr, outer)?;
                self.materialize_expr(low, outer)?;
                self.materialize_expr(high, outer)
            }
            Expr::Like { expr, pattern, .. } => {
                self.materialize_expr(expr, outer)?;
                self.materialize_expr(pattern, outer)
            }
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => {
                if let Some(operand) = operand {
                    self.materialize_expr(operand, outer)?;
                }
                for (when, then) in branches {
                    self.materialize_expr(when, outer)?;
                    self.materialize_expr(then, outer)?;
                }
                if let Some(else_expr) = else_expr {
                    self.materialize_expr(else_expr, outer)?;
                }
                Ok(())
            }
            Expr::Binary { left, right, .. } => {
                self.materialize_expr(left, outer)?;
                self.materialize_expr(right, outer)
            }
            Expr::Aggregate { input, .. } => {
                if let Some(input) = input {
                    self.materialize_expr(input, outer)?;
                }
                Ok(())
            }
        }
    }

    fn evaluate_subquery_column(&mut self, query: &Query) -> Result<Vec<Expr>, SqlError> {
        let result = self.execute_query(query)?;
        let QueryResult::Rows { columns, rows } = result else {
            return Err(SqlError::Unsupported(
                "subquery in IN must return rows".to_string(),
            ));
        };
        if columns.len() != 1 {
            return Err(SqlError::Unsupported(
                "subquery in IN must return exactly one column".to_string(),
            ));
        }
        Ok(rows
            .into_iter()
            .map(|mut row| Expr::Literal(row.pop().unwrap_or(Value::Null)))
            .collect())
    }

    fn evaluate_subquery_rows(
        &mut self,
        query: &Query,
        width: usize,
    ) -> Result<Vec<Vec<Value>>, SqlError> {
        let result = self.execute_query(query)?;
        let QueryResult::Rows { columns, rows } = result else {
            return Err(SqlError::Unsupported(
                "tuple IN subquery must return rows".to_string(),
            ));
        };
        if columns.len() != width {
            return Err(SqlError::Unsupported(format!(
                "tuple IN subquery returns {} column(s), expected {width}",
                columns.len()
            )));
        }
        Ok(rows)
    }

    fn evaluate_quantified_column(&mut self, query: &Query) -> Result<Vec<Value>, SqlError> {
        let result = self.execute_query(query)?;
        let QueryResult::Rows { columns, rows } = result else {
            return Err(SqlError::Unsupported(
                "quantified subquery must return rows".to_string(),
            ));
        };
        if columns.len() != 1 {
            return Err(SqlError::Unsupported(
                "quantified subquery must return exactly one column".to_string(),
            ));
        }
        Ok(rows
            .into_iter()
            .map(|mut row| row.pop().unwrap_or(Value::Null))
            .collect())
    }

    fn evaluate_scalar_subquery(&mut self, query: &Query) -> Result<Value, SqlError> {
        let result = self.execute_query(query)?;
        let QueryResult::Rows { columns, rows } = result else {
            return Err(SqlError::Unsupported(
                "scalar subquery must return rows".to_string(),
            ));
        };
        if columns.len() != 1 {
            return Err(SqlError::Unsupported(
                "scalar subquery must return exactly one column".to_string(),
            ));
        }
        match rows.len() {
            0 => Ok(Value::Null),
            1 => Ok(rows
                .into_iter()
                .next()
                .and_then(|mut row| row.pop())
                .unwrap_or(Value::Null)),
            _ => Err(SqlError::Unsupported(
                "scalar subquery returned more than one row".to_string(),
            )),
        }
    }

    fn with_query(&mut self, with: &With) -> Result<QueryResult, SqlError> {
        let mut saved: Vec<(String, Option<Table>)> = Vec::new();
        for cte in &with.ctes {
            let table = match self.materialize_cte(cte, with.recursive) {
                Ok(table) => table,
                Err(error) => {
                    self.restore_tables(saved);
                    return Err(error);
                }
            };
            saved.push((
                cte.name.clone(),
                self.tables.insert(cte.name.clone(), table),
            ));
        }
        let result = self.execute_statement(&with.body);
        self.restore_tables(saved);
        result
    }

    fn restore_tables(&mut self, saved: Vec<(String, Option<Table>)>) {
        for (name, previous) in saved.into_iter().rev() {
            match previous {
                Some(table) => {
                    self.tables.insert(name, table);
                }
                None => {
                    self.tables.remove(&name);
                }
            }
        }
    }

    /// Evaluates one CTE to a fixed point.
    ///
    /// The recursive step re-reads the CTE table from the catalog, so the
    /// current accumulated rows are visible under `cte.name`.
    fn materialize_cte(&mut self, cte: &Cte, recursive: bool) -> Result<Table, SqlError> {
        let seed = self.execute_query(&cte.seed)?;
        let (seed_columns, seed_rows) = rows_of(seed)?;
        if !cte.columns.is_empty() && cte.columns.len() != seed_columns.len() {
            return Err(SqlError::Unsupported(format!(
                "CTE {} declares {} column(s) but the seed produces {}",
                cte.name,
                cte.columns.len(),
                seed_columns.len()
            )));
        }
        let names = if cte.columns.is_empty() {
            seed_columns
        } else {
            cte.columns.clone()
        };
        let schema = infer_schema(&names, &seed_rows);
        let mut table = Table {
            name: cte.name.clone(),
            schema,
            rows: seed_rows,
        };

        if !recursive {
            if let Some(step_query) = &cte.step {
                let step = self.execute_query(step_query)?;
                let (step_columns, step_rows) = rows_of(step)?;
                if step_columns.len() != names.len() {
                    return Err(SqlError::Unsupported(format!(
                        "CTE {} step returns {} column(s), expected {}",
                        cte.name,
                        step_columns.len(),
                        names.len()
                    )));
                }
                for row in step_rows {
                    if cte.union_all || !table.rows.contains(&row) {
                        table.rows.push(row);
                    }
                }
            }
            return Ok(table);
        }
        let Some(step_query) = &cte.step else {
            return Err(SqlError::Unsupported(format!(
                "WITH RECURSIVE {} requires `UNION [ALL]`",
                cte.name
            )));
        };

        let step_schema = table.schema.clone();
        let mut working = table.rows.clone();
        let mut iterations = 0;
        loop {
            if iterations >= MAX_RECURSIVE_ITERATIONS {
                return Err(SqlError::Unsupported(format!(
                    "WITH RECURSIVE {} did not reach a fixed point within {} iterations",
                    cte.name, MAX_RECURSIVE_ITERATIONS
                )));
            }
            iterations += 1;

            // The recursive step sees only the previous round's rows, as in
            // the SQL standard's working table, not the accumulated result.
            let step_table = Table {
                name: cte.name.clone(),
                schema: step_schema.clone(),
                rows: working,
            };
            let previous = self.tables.insert(cte.name.clone(), step_table);
            let step = self.execute_query(step_query);
            match previous {
                Some(previous) => {
                    self.tables.insert(cte.name.clone(), previous);
                }
                None => {
                    self.tables.remove(&cte.name);
                }
            }
            let (step_columns, step_rows) = rows_of(step?)?;
            if step_columns.len() != names.len() {
                return Err(SqlError::Unsupported(format!(
                    "WITH RECURSIVE {} step returns {} column(s), expected {}",
                    cte.name,
                    step_columns.len(),
                    names.len()
                )));
            }
            let mut next = Vec::new();
            for row in step_rows {
                if cte.union_all || !table.rows.contains(&row) {
                    next.push(row.clone());
                    table.rows.push(row);
                }
            }
            if next.is_empty() {
                break;
            }
            working = next;
            if table.rows.len() > MAX_RECURSIVE_ROWS {
                return Err(SqlError::Unsupported(format!(
                    "WITH RECURSIVE {} produced more than {} rows",
                    cte.name, MAX_RECURSIVE_ROWS
                )));
            }
        }
        Ok(table)
    }

    fn select(&mut self, select: &Select) -> Result<QueryResult, SqlError> {
        let folded = fold_select(select);
        let outer = folded
            .table_alias
            .as_deref()
            .unwrap_or(&folded.table)
            .to_string();
        if folded.items.iter().any(|item| match item {
            SelectItem::Wildcard => false,
            SelectItem::Expr { expr, .. } => expr_contains_window(expr),
        }) {
            return self.select_with_window(select, &folded);
        }
        let has_correlated = folded
            .filter
            .as_ref()
            .is_some_and(|filter| expr_has_outer_subquery(filter, &outer))
            || folded.items.iter().any(|item| match item {
                SelectItem::Wildcard => false,
                SelectItem::Expr { expr, .. } => expr_has_outer_subquery(expr, &outer),
            })
            || folded
                .having
                .as_ref()
                .is_some_and(|having| expr_has_outer_subquery(having, &outer))
            || folded
                .order_by
                .iter()
                .any(|order| expr_has_outer_subquery(&order.expr, &outer));
        if has_correlated {
            return self.select_with_correlated(select, &folded, &outer);
        }
        let pushdown = plan_filter_pushdown(&folded);
        let projection = self.plan_scan_projection(&folded)?;
        let table = self.build_from(&folded, &pushdown, &projection)?;
        let filtered_select = select_with_filter(&folded, pushdown.residual.clone());
        let plan = bind_select(&filtered_select, &table)?;
        let result = execute_select(&table, &plan, self.deadline())?;
        match &select.under_repair {
            Some(keys) => filter_under_repair(result, keys),
            None => Ok(result),
        }
    }

    /// Evaluates window functions in the select list.
    fn select_with_window(
        &mut self,
        original: &Select,
        folded: &Select,
    ) -> Result<QueryResult, SqlError> {
        if !folded.group_by.is_empty() || select_has_aggregate(folded) {
            return Err(SqlError::Unsupported(
                "window functions with GROUP BY or aggregates are not supported".to_string(),
            ));
        }
        let outer = folded
            .table_alias
            .as_deref()
            .unwrap_or(&folded.table)
            .to_string();
        if folded.items.iter().any(|item| match item {
            SelectItem::Wildcard => false,
            SelectItem::Expr { expr, .. } => expr_has_outer_subquery(expr, &outer),
        }) {
            return Err(SqlError::Unsupported(
                "window functions combined with correlated subqueries are not supported"
                    .to_string(),
            ));
        }

        let pushdown = plan_filter_pushdown(folded);
        let projection = self.plan_scan_projection(folded)?;
        let mut table = self.build_from(folded, &pushdown, &projection)?;
        let mut planned = folded.clone();
        for (index, item) in folded.items.iter().enumerate() {
            let SelectItem::Expr { expr, alias } = item else {
                continue;
            };
            let Expr::Window {
                function,
                input,
                partition_by,
                order_by,
            } = expr
            else {
                if expr_contains_window(expr) {
                    return Err(SqlError::Unsupported(
                        "window function must be a top-level select item".to_string(),
                    ));
                }
                continue;
            };
            let values =
                self.window_values(&table, *function, input.as_deref(), partition_by, order_by)?;
            let name = format!("__win{index}");
            let data_type = values
                .iter()
                .find_map(|value| match value {
                    Value::Null => None,
                    other => Some(value_data_type(other)),
                })
                .unwrap_or(DataType::Text);
            table
                .schema
                .columns
                .push(Column::new(name.clone(), data_type));
            for (row, value) in table.rows.iter_mut().zip(values) {
                row.push(value);
            }
            planned.items[index] = SelectItem::Expr {
                expr: Expr::Column { table: None, name },
                alias: alias.clone(),
            };
        }

        let plan = bind_select(&planned, &table)?;
        let result = execute_select(&table, &plan, self.deadline())?;
        match &original.under_repair {
            Some(keys) => filter_under_repair(result, keys),
            None => Ok(result),
        }
    }

    #[allow(clippy::type_complexity)]
    fn window_values(
        &self,
        table: &Table,
        function: WindowFunction,
        input: Option<&Expr>,
        partition_by: &[Expr],
        order_by: &[OrderBy],
    ) -> Result<Vec<Value>, SqlError> {
        let mut groups: Vec<(Vec<Value>, Vec<(Vec<Value>, usize)>)> = Vec::new();
        for (index, row) in table.rows.iter().enumerate() {
            let mut partition = Vec::with_capacity(partition_by.len());
            for expr in partition_by {
                let bound = bind_expr(expr, &table.schema)?;
                partition.push(eval(&bound, &table.schema, row)?);
            }
            let mut keys = Vec::with_capacity(order_by.len());
            for order in order_by {
                let bound = bind_expr(&order.expr, &table.schema)?;
                keys.push(eval(&bound, &table.schema, row)?);
            }
            if let Some((_, group)) = groups.iter_mut().find(|(key, _)| *key == partition) {
                group.push((keys, index));
            } else {
                groups.push((partition, vec![(keys, index)]));
            }
        }

        let bound_input = input
            .map(|expr| bind_expr(expr, &table.schema))
            .transpose()?;
        let mut values = vec![Value::Null; table.rows.len()];
        for (_, mut group) in groups {
            group.sort_by(|(left, _), (right, _)| compare_window_keys(left, right, order_by));
            let mut rank = 1usize;
            let mut dense = 1usize;
            for (position, (keys, index)) in group.iter().enumerate() {
                if position > 0 {
                    let previous = &group[position - 1].0;
                    if compare_window_keys(previous, keys, order_by) != Ordering::Equal {
                        rank = position + 1;
                        dense += 1;
                    }
                }
                values[*index] = match function {
                    WindowFunction::RowNumber => Value::Int((position + 1) as i64),
                    WindowFunction::Rank => Value::Int(rank as i64),
                    WindowFunction::DenseRank => Value::Int(dense as i64),
                    WindowFunction::Lag | WindowFunction::Lead => {
                        let Some(bound) = bound_input.as_ref() else {
                            return Err(SqlError::Unsupported(
                                "LAG/LEAD requires an argument".to_string(),
                            ));
                        };
                        let target = if function == WindowFunction::Lag {
                            position.checked_sub(1)
                        } else {
                            Some(position + 1).filter(|next| *next < group.len())
                        };
                        match target {
                            Some(target) => {
                                let row_index = group[target].1;
                                eval(bound, &table.schema, &table.rows[row_index])?
                            }
                            None => Value::Null,
                        }
                    }
                    WindowFunction::Aggregate(kind) => {
                        // Without ORDER BY the frame is the whole partition; with
                        // ORDER BY it is a running frame up to the current row.
                        let frame_end = if order_by.is_empty() {
                            group.len()
                        } else {
                            position + 1
                        };
                        let rows: Vec<&Row> = group[..frame_end]
                            .iter()
                            .map(|(_, index)| &table.rows[*index])
                            .collect();
                        eval_aggregate(kind, bound_input.as_ref(), &table.schema, &rows)?
                    }
                };
            }
        }
        Ok(values)
    }

    /// Evaluates correlated subqueries in `WHERE` and in the select list.
    ///
    /// Every outer row is substituted into the subquery's outer-column
    /// references. A correlated `WHERE` item keeps or drops the row; a
    /// correlated select-list expression is evaluated per row into a hidden
    /// `__corrN` column, and the item becomes a reference to that column.
    /// The rewritten query then goes through the ordinary projection/sort path.
    fn select_with_correlated(
        &mut self,
        original: &Select,
        folded: &Select,
        outer: &str,
    ) -> Result<QueryResult, SqlError> {
        let items_have_correlated = folded.items.iter().any(|item| match item {
            SelectItem::Wildcard => false,
            SelectItem::Expr { expr, .. } => expr_has_outer_subquery(expr, outer),
        });
        if items_have_correlated && (!folded.group_by.is_empty() || select_has_aggregate(folded)) {
            return Err(SqlError::Unsupported(
                "correlated subqueries with GROUP BY or aggregates are not supported".to_string(),
            ));
        }
        let pushdown = plan_filter_pushdown(folded);
        let projection = self.plan_scan_projection(folded)?;
        let mut table = self.build_from(folded, &pushdown, &projection)?;
        if table.rows.len() > MAX_CORRELATED_ROWS {
            return Err(SqlError::Unsupported(format!(
                "correlated subquery would run for {} rows (limit {MAX_CORRELATED_ROWS}); \
                 rewrite it as a JOIN",
                table.rows.len()
            )));
        }

        let mut planned = folded.clone();
        if let Some(filter) = &folded.filter {
            if expr_has_outer_subquery(filter, outer) {
                let filter = filter.clone();
                let mut kept = Vec::with_capacity(table.rows.len());
                for row in table.rows.drain(..) {
                    let mut row_filter = filter.clone();
                    self.bind_correlated_expr(&mut row_filter, &table.schema, &row, outer)?;
                    let bound = bind_expr(&row_filter, &table.schema)?;
                    if truth_of(&eval(&bound, &table.schema, &row)?)? == TruthValue::True {
                        kept.push(row);
                    }
                }
                table.rows = kept;
                planned.filter = None;
            }
        }
        for (index, item) in folded.items.iter().enumerate() {
            let SelectItem::Expr { expr, alias } = item else {
                continue;
            };
            if !expr_has_outer_subquery(expr, outer) {
                continue;
            }
            let name = format!("__corr{index}");
            let mut values = Vec::with_capacity(table.rows.len());
            for row in &table.rows {
                let mut row_expr = expr.clone();
                self.bind_correlated_expr(&mut row_expr, &table.schema, row, outer)?;
                let bound = bind_expr(&row_expr, &table.schema)?;
                values.push(eval(&bound, &table.schema, row)?);
            }
            let data_type = values
                .iter()
                .find_map(|value| match value {
                    Value::Null => None,
                    other => Some(value_data_type(other)),
                })
                .unwrap_or(DataType::Text);
            table
                .schema
                .columns
                .push(Column::new(name.clone(), data_type));
            for (row, value) in table.rows.iter_mut().zip(values) {
                row.push(value);
            }
            planned.items[index] = SelectItem::Expr {
                expr: Expr::Column { table: None, name },
                alias: alias.clone(),
            };
        }

        for (index, order) in folded.order_by.iter().enumerate() {
            if !expr_has_outer_subquery(&order.expr, outer) {
                continue;
            }
            let name = format!("__ord{index}");
            let mut values = Vec::with_capacity(table.rows.len());
            for row in &table.rows {
                let mut row_expr = order.expr.clone();
                self.bind_correlated_expr(&mut row_expr, &table.schema, row, outer)?;
                let bound = bind_expr(&row_expr, &table.schema)?;
                values.push(eval(&bound, &table.schema, row)?);
            }
            let data_type = values
                .iter()
                .find_map(|value| match value {
                    Value::Null => None,
                    other => Some(value_data_type(other)),
                })
                .unwrap_or(DataType::Text);
            table
                .schema
                .columns
                .push(Column::new(name.clone(), data_type));
            for (row, value) in table.rows.iter_mut().zip(values) {
                row.push(value);
            }
            planned.order_by[index].expr = Expr::Column { table: None, name };
        }

        if let Some(having) = &folded.having {
            if expr_has_outer_subquery(having, outer) {
                if folded.group_by.is_empty() {
                    return Err(SqlError::Unsupported(
                        "correlated subqueries in HAVING require GROUP BY".to_string(),
                    ));
                }
                let groups = self.group_keys(folded, &table)?;
                let mut planned_having = having.clone();
                self.replace_having_subqueries(
                    &mut planned_having,
                    &table,
                    &folded.group_by,
                    &groups,
                    outer,
                )?;
                planned.having = Some(planned_having);
            }
        }

        let plan = bind_select(&planned, &table)?;
        let result = execute_select(&table, &plan, self.deadline())?;
        match &original.under_repair {
            Some(keys) => filter_under_repair(result, keys),
            None => Ok(result),
        }
    }

    /// One representative row per `GROUP BY` key.
    fn group_keys(
        &self,
        select: &Select,
        table: &Table,
    ) -> Result<Vec<(Vec<Value>, Row)>, SqlError> {
        // A hash set keeps this O(rows); the previous linear `seen` vector
        // made a high-cardinality GROUP BY quadratic.
        let mut seen: std::collections::HashSet<Vec<Value>> = std::collections::HashSet::new();
        let mut groups: Vec<(Vec<Value>, Row)> = Vec::new();
        for row in &table.rows {
            let mut key = Vec::with_capacity(select.group_by.len());
            for expr in &select.group_by {
                let bound = bind_expr(expr, &table.schema)?;
                key.push(eval(&bound, &table.schema, row)?);
            }
            if seen.insert(key.clone()) {
                groups.push((key, row.clone()));
            }
        }
        Ok(groups)
    }

    /// Rewrites scalar correlated subqueries in `HAVING` into a `CASE` over
    /// the group keys, so the ordinary grouping executor can evaluate it.
    fn replace_having_subqueries(
        &mut self,
        expr: &mut Expr,
        table: &Table,
        group_by: &[Expr],
        groups: &[(Vec<Value>, Row)],
        outer: &str,
    ) -> Result<(), SqlError> {
        match expr {
            Expr::Window { .. } => Ok(()),
            Expr::ScalarSubquery { query } => {
                if !query_has_outer_ref(query, outer) {
                    return Ok(());
                }
                *expr = self.having_case(query, table, group_by, groups, outer)?;
                Ok(())
            }
            Expr::InSubquery { .. }
            | Expr::InSubqueryTuple { .. }
            | Expr::Exists { .. }
            | Expr::Quantified { .. } => {
                if expr_has_outer_subquery(expr, outer) {
                    return Err(SqlError::Unsupported(
                        "only scalar correlated subqueries are supported in HAVING".to_string(),
                    ));
                }
                Ok(())
            }
            Expr::Literal(_) | Expr::Column { .. } | Expr::Tuple(_) => Ok(()),
            Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
                self.replace_having_subqueries(expr, table, group_by, groups, outer)
            }
            Expr::Truth { expr, threshold } => {
                self.replace_having_subqueries(expr, table, group_by, groups, outer)?;
                if let Some(threshold) = threshold {
                    self.replace_having_subqueries(threshold, table, group_by, groups, outer)?;
                }
                Ok(())
            }
            Expr::Certainty { expr, threshold } => {
                self.replace_having_subqueries(expr, table, group_by, groups, outer)?;
                if let Some(threshold) = threshold {
                    self.replace_having_subqueries(threshold, table, group_by, groups, outer)?;
                }
                Ok(())
            }
            Expr::InList { expr, values, .. } => {
                self.replace_having_subqueries(expr, table, group_by, groups, outer)?;
                for value in values {
                    self.replace_having_subqueries(value, table, group_by, groups, outer)?;
                }
                Ok(())
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.replace_having_subqueries(expr, table, group_by, groups, outer)?;
                self.replace_having_subqueries(low, table, group_by, groups, outer)?;
                self.replace_having_subqueries(high, table, group_by, groups, outer)
            }
            Expr::Like { expr, pattern, .. } => {
                self.replace_having_subqueries(expr, table, group_by, groups, outer)?;
                self.replace_having_subqueries(pattern, table, group_by, groups, outer)
            }
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => {
                if let Some(operand) = operand {
                    self.replace_having_subqueries(operand, table, group_by, groups, outer)?;
                }
                for (when, then) in branches {
                    self.replace_having_subqueries(when, table, group_by, groups, outer)?;
                    self.replace_having_subqueries(then, table, group_by, groups, outer)?;
                }
                if let Some(else_expr) = else_expr {
                    self.replace_having_subqueries(else_expr, table, group_by, groups, outer)?;
                }
                Ok(())
            }
            Expr::Binary { left, right, .. } => {
                self.replace_having_subqueries(left, table, group_by, groups, outer)?;
                self.replace_having_subqueries(right, table, group_by, groups, outer)
            }
            Expr::Aggregate { input, .. } => {
                if let Some(input) = input {
                    self.replace_having_subqueries(input, table, group_by, groups, outer)?;
                }
                Ok(())
            }
        }
    }

    fn having_case(
        &mut self,
        query: &Query,
        table: &Table,
        group_by: &[Expr],
        groups: &[(Vec<Value>, Row)],
        outer: &str,
    ) -> Result<Expr, SqlError> {
        if groups.len() > MAX_CORRELATED_ROWS {
            return Err(SqlError::Unsupported(format!(
                "correlated subquery in HAVING would run for {} groups (limit {MAX_CORRELATED_ROWS})",
                groups.len()
            )));
        }
        let mut branches = Vec::with_capacity(groups.len());
        for (key, row) in groups {
            let mut query = query.clone();
            self.substitute_outer(&mut query, &table.schema, row, outer)?;
            let value = self.evaluate_scalar_subquery(&query)?;
            let mut condition: Option<Expr> = None;
            for (expr, value) in group_by.iter().cloned().zip(key) {
                let pair = Expr::Binary {
                    op: BinaryOp::Eq,
                    left: Box::new(expr),
                    right: Box::new(Expr::Literal(value.clone())),
                };
                condition = Some(match condition {
                    None => pair,
                    Some(previous) => Expr::Binary {
                        op: BinaryOp::And,
                        left: Box::new(previous),
                        right: Box::new(pair),
                    },
                });
            }
            let condition = condition.ok_or_else(|| {
                SqlError::Unsupported(
                    "correlated subqueries in HAVING require GROUP BY".to_string(),
                )
            })?;
            branches.push((condition, Expr::Literal(value)));
        }
        Ok(Expr::Case {
            operand: None,
            branches,
            else_expr: Some(Box::new(Expr::Literal(Value::Null))),
        })
    }

    /// Replaces correlated subqueries in one row's filter with their values.
    fn bind_correlated_expr(
        &mut self,
        expr: &mut Expr,
        outer_schema: &Schema,
        outer_row: &Row,
        outer: &str,
    ) -> Result<(), SqlError> {
        match expr {
            Expr::Window { .. } => Ok(()),
            Expr::Literal(_) | Expr::Column { .. } => Ok(()),
            Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
                self.bind_correlated_expr(expr, outer_schema, outer_row, outer)
            }
            Expr::Truth { expr, threshold } => {
                self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                if let Some(threshold) = threshold {
                    self.bind_correlated_expr(threshold, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::Certainty { expr, threshold } => {
                self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                if let Some(threshold) = threshold {
                    self.bind_correlated_expr(threshold, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::InList { expr, values, .. } => {
                self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                for value in values {
                    self.bind_correlated_expr(value, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::Tuple(exprs) => {
                for expr in exprs {
                    self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::InSubqueryTuple { .. } => {
                let (mut exprs, query, negated) =
                    match std::mem::replace(expr, Expr::Literal(Value::Null)) {
                        Expr::InSubqueryTuple {
                            exprs,
                            query,
                            negated,
                        } => (exprs, query, negated),
                        _ => unreachable!("replaced expression was not tuple IN subquery"),
                    };
                for expr in &mut exprs {
                    self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                }
                let mut query = query.as_ref().clone();
                self.substitute_outer(&mut query, outer_schema, outer_row, outer)?;
                let rows = self.evaluate_subquery_rows(&query, exprs.len())?;
                *expr = tuple_in_expr(exprs, rows, negated);
                Ok(())
            }
            Expr::InSubquery { query, negated, .. } => {
                let mut query = query.as_ref().clone();
                self.substitute_outer(&mut query, outer_schema, outer_row, outer)?;
                let values = self.evaluate_subquery_column(&query)?;
                let negated = *negated;
                let probe = match std::mem::replace(expr, Expr::Literal(Value::Null)) {
                    Expr::InSubquery { expr: probe, .. } => probe,
                    _ => unreachable!("replaced expression was not IN subquery"),
                };
                let mut probe = *probe;
                self.bind_correlated_expr(&mut probe, outer_schema, outer_row, outer)?;
                *expr = Expr::InList {
                    expr: Box::new(probe),
                    values,
                    negated,
                };
                Ok(())
            }
            Expr::ScalarSubquery { query } => {
                let mut query = query.as_ref().clone();
                self.substitute_outer(&mut query, outer_schema, outer_row, outer)?;
                let value = self.evaluate_scalar_subquery(&query)?;
                *expr = Expr::Literal(value);
                Ok(())
            }
            Expr::Exists { query } => {
                let mut query = query.as_ref().clone();
                self.substitute_outer(&mut query, outer_schema, outer_row, outer)?;
                let result = self.execute_query(&query)?;
                let exists = matches!(&result, QueryResult::Rows { rows, .. } if !rows.is_empty());
                *expr = Expr::Literal(Value::Bool(exists));
                Ok(())
            }
            Expr::Quantified { .. } => {
                let (mut probe, op, kind, mut query) =
                    match std::mem::replace(expr, Expr::Literal(Value::Null)) {
                        Expr::Quantified {
                            expr,
                            op,
                            kind,
                            query,
                        } => (expr, op, kind, query),
                        _ => unreachable!("replaced expression was not quantified"),
                    };
                self.bind_correlated_expr(&mut probe, outer_schema, outer_row, outer)?;
                self.substitute_outer(&mut query, outer_schema, outer_row, outer)?;
                let values = self.evaluate_quantified_column(&query)?;
                *expr = quantified_expr(*probe, op, kind, values);
                Ok(())
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                self.bind_correlated_expr(low, outer_schema, outer_row, outer)?;
                self.bind_correlated_expr(high, outer_schema, outer_row, outer)
            }
            Expr::Like { expr, pattern, .. } => {
                self.bind_correlated_expr(expr, outer_schema, outer_row, outer)?;
                self.bind_correlated_expr(pattern, outer_schema, outer_row, outer)
            }
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => {
                if let Some(operand) = operand {
                    self.bind_correlated_expr(operand, outer_schema, outer_row, outer)?;
                }
                for (when, then) in branches {
                    self.bind_correlated_expr(when, outer_schema, outer_row, outer)?;
                    self.bind_correlated_expr(then, outer_schema, outer_row, outer)?;
                }
                if let Some(else_expr) = else_expr {
                    self.bind_correlated_expr(else_expr, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::Binary { left, right, .. } => {
                self.bind_correlated_expr(left, outer_schema, outer_row, outer)?;
                self.bind_correlated_expr(right, outer_schema, outer_row, outer)
            }
            Expr::Aggregate { input, .. } => {
                if let Some(input) = input {
                    self.bind_correlated_expr(input, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
        }
    }

    fn substitute_outer(
        &mut self,
        query: &mut Query,
        outer_schema: &Schema,
        outer_row: &Row,
        outer: &str,
    ) -> Result<(), SqlError> {
        match query {
            Query::Select(select) => {
                self.substitute_outer_select(select, outer_schema, outer_row, outer)
            }
            Query::With(_) => Err(SqlError::Unsupported(
                "correlated subquery with a nested WITH is not supported".to_string(),
            )),
        }
    }

    fn substitute_outer_select(
        &mut self,
        select: &mut Select,
        outer_schema: &Schema,
        outer_row: &Row,
        outer: &str,
    ) -> Result<(), SqlError> {
        for item in &mut select.items {
            if let SelectItem::Expr { expr, .. } = item {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
            }
        }
        for join in &mut select.joins {
            if let JoinCondition::On(expr) = &mut join.condition {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
            }
        }
        if let Some(filter) = &mut select.filter {
            self.substitute_outer_expr(filter, outer_schema, outer_row, outer)?;
        }
        for expr in &mut select.group_by {
            self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
        }
        if let Some(having) = &mut select.having {
            self.substitute_outer_expr(having, outer_schema, outer_row, outer)?;
        }
        for order in &mut select.order_by {
            self.substitute_outer_expr(&mut order.expr, outer_schema, outer_row, outer)?;
        }
        Ok(())
    }

    fn substitute_outer_expr(
        &mut self,
        expr: &mut Expr,
        outer_schema: &Schema,
        outer_row: &Row,
        outer: &str,
    ) -> Result<(), SqlError> {
        match expr {
            Expr::Window { .. } => Ok(()),
            Expr::Column {
                table: Some(qualifier),
                name,
            } => {
                if qualifier.as_str() == outer {
                    let qualified = format!("{outer}.{name}");
                    let index = outer_schema
                        .index_of(name)
                        .or_else(|| outer_schema.index_of(&qualified))
                        .ok_or_else(|| SqlError::UnknownColumn(name.clone()))?;
                    let value = outer_row[index].clone();
                    *expr = Expr::Literal(value);
                }
                Ok(())
            }
            Expr::Literal(_) | Expr::Column { table: None, .. } => Ok(()),
            Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)
            }
            Expr::Truth { expr, threshold } => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                if let Some(threshold) = threshold {
                    self.substitute_outer_expr(threshold, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::Certainty { expr, threshold } => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                if let Some(threshold) = threshold {
                    self.substitute_outer_expr(threshold, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::InList { expr, values, .. } => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                for value in values {
                    self.substitute_outer_expr(value, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::Tuple(exprs) => {
                for expr in exprs {
                    self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::InSubqueryTuple { exprs, query, .. } => {
                for expr in exprs {
                    self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                }
                self.substitute_outer(query, outer_schema, outer_row, outer)
            }
            Expr::InSubquery { query, .. } | Expr::ScalarSubquery { query } => {
                self.substitute_outer(query, outer_schema, outer_row, outer)
            }
            Expr::Exists { query } => self.substitute_outer(query, outer_schema, outer_row, outer),
            Expr::Quantified { expr, query, .. } => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                self.substitute_outer(query, outer_schema, outer_row, outer)
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                self.substitute_outer_expr(low, outer_schema, outer_row, outer)?;
                self.substitute_outer_expr(high, outer_schema, outer_row, outer)
            }
            Expr::Like { expr, pattern, .. } => {
                self.substitute_outer_expr(expr, outer_schema, outer_row, outer)?;
                self.substitute_outer_expr(pattern, outer_schema, outer_row, outer)
            }
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => {
                if let Some(operand) = operand {
                    self.substitute_outer_expr(operand, outer_schema, outer_row, outer)?;
                }
                for (when, then) in branches {
                    self.substitute_outer_expr(when, outer_schema, outer_row, outer)?;
                    self.substitute_outer_expr(then, outer_schema, outer_row, outer)?;
                }
                if let Some(else_expr) = else_expr {
                    self.substitute_outer_expr(else_expr, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
            Expr::Binary { left, right, .. } => {
                self.substitute_outer_expr(left, outer_schema, outer_row, outer)?;
                self.substitute_outer_expr(right, outer_schema, outer_row, outer)
            }
            Expr::Aggregate { input, .. } => {
                if let Some(input) = input {
                    self.substitute_outer_expr(input, outer_schema, outer_row, outer)?;
                }
                Ok(())
            }
        }
    }

    fn analyze(&mut self, table_name: Option<&str>) -> Result<QueryResult, SqlError> {
        let names = match table_name {
            Some(name) => {
                if !self.tables.contains_key(name) {
                    return Err(self.unknown_table(name));
                }
                vec![name.to_string()]
            }
            None => self.tables.keys().cloned().collect(),
        };
        if names.is_empty() {
            return Ok(QueryResult::Message("analyzed 0 table(s)".to_string()));
        }

        let mut analyzed = Vec::with_capacity(names.len());
        for name in &names {
            let table = self
                .tables
                .get(name)
                .expect("table list came from the catalog");
            let stats = analyze_table(table);
            let value = encode_stats(&stats)?;
            analyzed.push((name.clone(), stats, value));
        }

        let mut txn = self.store.begin()?;
        for (name, _, value) in &analyzed {
            txn.put(&stats_key(name), value)?;
        }
        txn.commit()?;
        for (name, stats, _) in analyzed {
            self.stats.insert(name, stats);
        }
        Ok(QueryResult::Message(format!(
            "analyzed {} table(s)",
            names.len()
        )))
    }

    fn explain(
        &self,
        select: &Select,
        analyze: bool,
        format: ExplainFormat,
    ) -> Result<QueryResult, SqlError> {
        let folded = fold_select(select);
        let pushdown = plan_filter_pushdown(&folded);
        let projection = self.plan_scan_projection(&folded)?;
        let filtered_select = select_with_filter(&folded, pushdown.residual.clone());
        let estimates = if analyze {
            None
        } else {
            self.estimate_plan(&folded, &pushdown)?
        };
        let (plan, stats) = if analyze {
            let start = Instant::now();
            let mut stats = ExecutionStats::default();
            let table = self.build_from_inner(&folded, &pushdown, &projection, Some(&mut stats))?;
            let plan = bind_select(&filtered_select, &table)?;
            let result = execute_select_inner(&table, &plan, Some(&mut stats), self.deadline())?;
            if let QueryResult::Rows { rows, .. } = &result {
                stats.output_rows = rows.len();
            }
            stats.total_ms = start.elapsed().as_secs_f64() * 1000.0;
            let plan = self.build_plan(&folded, &pushdown, &projection, Some(&stats), None)?;
            (plan, Some(stats))
        } else {
            (
                self.build_plan(&folded, &pushdown, &projection, None, estimates.as_ref())?,
                None,
            )
        };
        let mut plan = match (&format, &stats) {
            (ExplainFormat::Text, _) => plan.to_text(),
            (ExplainFormat::Sexpr, None) => plan.to_sexpr(),
            (ExplainFormat::Sexpr, Some(stats)) => {
                plan.to_sexpr_with_execution(stats.output_rows, stats.total_ms)
            }
        };
        if matches!(format, ExplainFormat::Text) {
            if let Some(stats) = stats {
                plan.push_str(&format!(
                    "\nExecution: rows={} time={:.3}ms",
                    stats.output_rows, stats.total_ms
                ));
            }
        }
        Ok(QueryResult::Rows {
            columns: vec!["plan".to_string()],
            rows: match format {
                ExplainFormat::Text => plan
                    .lines()
                    .map(|line| vec![Value::Text(line.to_string())])
                    .collect(),
                ExplainFormat::Sexpr => vec![vec![Value::Text(plan)]],
            },
        })
    }

    fn estimate_plan(
        &self,
        select: &Select,
        pushdown: &PushdownPlan,
    ) -> Result<Option<PlanEstimate>, SqlError> {
        let mut sources = self.source_tables(select)?.into_iter();
        let (left, left_qualifier) = sources.next().expect("base table is always present");
        let Some(left_stats) = self.stats.get(&left.name) else {
            return Ok(None);
        };
        let mut left_table = qualify_table_schema(&left, &left_qualifier);
        let left_filter = pushdown_filter(pushdown, 0);
        let mut left_rows = estimate_scan_rows(&left, left_stats, &left_qualifier, left_filter)?;
        let mut left_ndvs = left_stats
            .columns
            .iter()
            .map(|column| column.distinct)
            .collect::<Vec<_>>();
        let mut estimate = PlanEstimate {
            scan_rows: vec![ceil_estimate(left_rows)],
            ..PlanEstimate::default()
        };

        for (join_index, (join, (right, qualifier))) in select.joins.iter().zip(sources).enumerate()
        {
            let Some(right_stats) = self.stats.get(&right.name) else {
                return Ok(None);
            };
            let right_table = qualify_table_schema(&right, &qualifier);
            let right_filter = pushdown_filter(pushdown, join_index + 1);
            let right_rows = estimate_scan_rows(&right, right_stats, &qualifier, right_filter)?;
            let pairs = match &join.condition {
                JoinCondition::Using(names) => using_pairs(&left_table, &right_table, names)?,
                JoinCondition::Natural => natural_pairs(&left_table, &right_table)?,
                JoinCondition::On(_) | JoinCondition::None => Vec::new(),
            };
            let pair_schema = Schema::new(
                left_table
                    .schema
                    .columns
                    .iter()
                    .chain(right_table.schema.columns.iter())
                    .cloned()
                    .collect(),
            );
            let predicate = match &join.condition {
                JoinCondition::On(expr) => Some(bind_expr(expr, &pair_schema)?),
                JoinCondition::Using(_) | JoinCondition::Natural => {
                    using_predicate(&pairs, left_table.schema.len())
                }
                JoinCondition::None => None,
            };
            let hash_pairs = match &join.condition {
                JoinCondition::Using(_) | JoinCondition::Natural => pairs.clone(),
                JoinCondition::On(_) => predicate
                    .as_ref()
                    .map(|expr| equi_join_pairs(expr, left_table.schema.len()))
                    .unwrap_or_default(),
                JoinCondition::None => Vec::new(),
            };
            let right_ndvs = right_stats
                .columns
                .iter()
                .map(|column| column.distinct)
                .collect::<Vec<_>>();
            let uses_hash = choose_join_algorithm(
                self.join_strategy,
                join.kind,
                left_rows,
                right_rows,
                &hash_pairs,
                predicate.as_ref(),
                JoinStats {
                    left_ndvs: &left_ndvs,
                    right_ndvs: &right_ndvs,
                    available: true,
                },
            );
            let join_rows = estimate_join_rows(
                join.kind,
                left_rows,
                right_rows,
                &hash_pairs,
                &left_ndvs,
                &right_ndvs,
                predicate.as_ref(),
            );
            let (schema, output_sources) =
                build_join_layout(&left_table, &right_table, &pairs, join.kind);
            left_ndvs = join_output_ndvs(&output_sources, &left_ndvs, &right_ndvs);
            left_table = Table {
                name: left_table.name,
                schema,
                rows: Vec::new(),
            };
            left_rows = join_rows;
            estimate.scan_rows.push(ceil_estimate(right_rows));
            estimate.join_rows.push(ceil_estimate(join_rows));
            estimate.join_uses_hash.push(uses_hash);
        }

        let filtered_select = select_with_filter(select, pushdown.residual.clone());
        let bound = bind_select(&filtered_select, &left_table)?;
        let filtered = match bound.filter.as_ref() {
            Some(filter) => left_rows * estimate_selectivity(filter, &left_ndvs),
            None => left_rows,
        };
        estimate.filter_rows = ceil_estimate(filtered);

        if bound.is_grouped() {
            let groups = estimate_group_count(&bound, filtered, &left_ndvs);
            let having = if bound.having.is_some() {
                groups * 0.5
            } else {
                groups
            };
            estimate.aggregate_groups = ceil_estimate(groups);
            estimate.having_groups = ceil_estimate(having);
            estimate.project_rows = ceil_estimate(having);
        } else {
            estimate.project_rows = ceil_estimate(filtered);
        }
        estimate.sort_rows = estimate.project_rows;
        estimate.output_rows = match bound.limit {
            Some(limit) => estimate.project_rows.min(limit as u64),
            None => estimate.project_rows,
        };
        Ok(Some(estimate))
    }

    fn source_tables(&self, select: &Select) -> Result<Vec<(Table, String)>, SqlError> {
        if select.table.is_empty() {
            if !select.joins.is_empty() {
                return Err(SqlError::Unsupported(
                    "JOIN requires a FROM clause".to_string(),
                ));
            }
            // `SELECT <exprs>` without a table: one empty row, zero columns.
            return Ok(vec![(
                Table {
                    name: String::new(),
                    schema: Schema::new(Vec::new()),
                    rows: vec![Vec::new()],
                },
                String::new(),
            )]);
        }
        let table = self
            .tables
            .get(&select.table)
            .cloned()
            .ok_or_else(|| self.unknown_table(&select.table))?;
        let mut qualifiers = BTreeSet::new();
        let mut sources = Vec::with_capacity(select.joins.len() + 1);
        let left_qualifier = select.table_alias.as_deref().unwrap_or(&select.table);
        if !qualifiers.insert(left_qualifier.to_string()) {
            return Err(SqlError::DuplicateTableAlias(left_qualifier.to_string()));
        }
        sources.push((table, left_qualifier.to_string()));
        for join in &select.joins {
            let right = self
                .tables
                .get(&join.table)
                .cloned()
                .ok_or_else(|| self.unknown_table(&join.table))?;
            let qualifier = join.alias.as_deref().unwrap_or(&join.table);
            if !qualifiers.insert(qualifier.to_string()) {
                return Err(SqlError::DuplicateTableAlias(qualifier.to_string()));
            }
            sources.push((right, qualifier.to_string()));
        }
        Ok(sources)
    }

    /// Plans a conservative source-column projection. A query is prunable only
    /// when every column reference can be assigned to one source, there is no
    /// wildcard, and no `USING`/`NATURAL` join depends on merged schema names.
    fn plan_scan_projection(&self, select: &Select) -> Result<ScanProjection, SqlError> {
        let sources = self.source_tables(select)?;
        let qualifiers = sources
            .iter()
            .map(|(_, qualifier)| qualifier.clone())
            .collect::<Vec<_>>();
        let source_columns = sources
            .iter()
            .map(|(table, _)| {
                table
                    .schema
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect::<BTreeSet<_>>()
            })
            .collect::<Vec<_>>();
        let mut keep = vec![BTreeSet::new(); qualifiers.len()];
        let mut disabled = false;
        let mut collect =
            |expr: &Expr| collect_expr_columns(expr, &qualifiers, &source_columns, &mut keep);

        for item in &select.items {
            match item {
                SelectItem::Wildcard => disabled = true,
                SelectItem::Expr { expr, .. } => {
                    if !collect(expr) {
                        disabled = true;
                    }
                }
            }
        }
        if let Some(filter) = &select.filter {
            if !collect(filter) {
                disabled = true;
            }
        }
        for expr in &select.group_by {
            if !collect(expr) {
                disabled = true;
            }
        }
        if let Some(having) = &select.having {
            if !collect(having) {
                disabled = true;
            }
        }
        for order in &select.order_by {
            if !collect(&order.expr) {
                disabled = true;
            }
        }
        for join in &select.joins {
            match &join.condition {
                JoinCondition::On(expr) => {
                    if !collect(expr) {
                        disabled = true;
                    }
                }
                JoinCondition::Using(_) | JoinCondition::Natural => disabled = true,
                JoinCondition::None => {}
            }
        }

        if disabled {
            return Ok(ScanProjection::all(sources.len()));
        }

        let mut columns = Vec::with_capacity(sources.len());
        for (index, (table, _)) in sources.iter().enumerate() {
            let available = table
                .schema
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>();
            let kept = available
                .iter()
                .filter(|name| keep[index].contains(*name))
                .cloned()
                .collect::<Vec<_>>();
            if kept.len() == available.len() {
                columns.push(None);
            } else {
                columns.push(Some(kept));
            }
        }
        Ok(ScanProjection { columns })
    }

    fn build_from(
        &self,
        select: &Select,
        pushdown: &PushdownPlan,
        projection: &ScanProjection,
    ) -> Result<Table, SqlError> {
        self.build_from_inner(select, pushdown, projection, None)
    }

    fn build_from_inner(
        &self,
        select: &Select,
        pushdown: &PushdownPlan,
        projection: &ScanProjection,
        mut stats: Option<&mut ExecutionStats>,
    ) -> Result<Table, SqlError> {
        let mut sources = self.source_tables(select)?.into_iter();
        let (left, left_qualifier) = sources.next().expect("base table is always present");
        let mut joined = qualify_table_projected(&left, &left_qualifier, projection.columns(0));
        let left_filtered = pushdown_filter(pushdown, 0).is_some();
        if let Some(filter) = pushdown_filter(pushdown, 0) {
            joined = filter_table(joined, filter)?;
        }
        if let Some(stats) = stats.as_deref_mut() {
            stats.scan_rows.push(joined.rows.len());
            stats.scan_filters.push(left_filtered);
        }
        let mut stats_available = self.stats.contains_key(&left.name);
        let mut left_ndvs = self
            .stats
            .get(&left.name)
            .map(|stats| {
                stats
                    .columns
                    .iter()
                    .map(|column| column.distinct)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (join_index, (join, (right, qualifier))) in select.joins.iter().zip(sources).enumerate()
        {
            let mut right =
                qualify_table_projected(&right, &qualifier, projection.columns(join_index + 1));
            let right_filtered = pushdown_filter(pushdown, join_index + 1).is_some();
            if let Some(filter) = pushdown_filter(pushdown, join_index + 1) {
                right = filter_table(right, filter)?;
            }
            if let Some(stats) = stats.as_deref_mut() {
                stats.scan_rows.push(right.rows.len());
                stats.scan_filters.push(right_filtered);
            }
            let right_ndvs = self
                .stats
                .get(&right.name)
                .map(|stats| {
                    stats
                        .columns
                        .iter()
                        .map(|column| column.distinct)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let has_stats = stats_available && self.stats.contains_key(&right.name);
            let start = stats.is_some().then(Instant::now);
            let (joined_table, output_sources, uses_hash) = join_tables(
                &joined,
                &right,
                &join.condition,
                join.kind,
                self.join_strategy,
                JoinStats {
                    left_ndvs: &left_ndvs,
                    right_ndvs: &right_ndvs,
                    available: has_stats,
                },
            )?;
            joined = joined_table;
            if let (Some(stats), Some(start)) = (stats.as_deref_mut(), start) {
                stats.join_rows.push(joined.rows.len());
                stats.join_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                stats.join_uses_hash.push(uses_hash);
            }
            left_ndvs = join_output_ndvs(&output_sources, &left_ndvs, &right_ndvs);
            stats_available = has_stats;
        }
        Ok(joined)
    }

    fn build_plan(
        &self,
        select: &Select,
        pushdown: &PushdownPlan,
        projection: &ScanProjection,
        stats: Option<&ExecutionStats>,
        estimates: Option<&PlanEstimate>,
    ) -> Result<PlanNode, SqlError> {
        let mut sources = self.source_tables(select)?.into_iter();
        let (left, left_qualifier) = sources.next().expect("base table is always present");
        let mut left_table = qualify_table_schema(&left, &left_qualifier);
        let mut scan_index = 0;
        let left_filtered = stats
            .and_then(|stats| stats.scan_filters.get(scan_index).copied())
            .unwrap_or_else(|| pushdown_filter(pushdown, scan_index).is_some());
        let left_filter = plan_scan_filter(pushdown, scan_index, &left_table.schema)?;
        let mut plan = PlanNode::scan(
            left.name.clone(),
            source_alias(left.name.as_str(), &left_qualifier),
            left_filtered,
            left_filter,
            stats
                .and_then(|stats| stats.scan_rows.get(scan_index).copied())
                .map(|rows| rows as u64),
            estimates.and_then(|estimate| estimate.scan_rows.get(scan_index).copied()),
            projection
                .columns(scan_index)
                .map(|columns| columns.to_vec()),
        );
        scan_index += 1;
        for (join_index, (join, (right, qualifier))) in select.joins.iter().zip(sources).enumerate()
        {
            let right_table = qualify_table_schema(&right, &qualifier);
            let pairs = match &join.condition {
                JoinCondition::Using(names) => using_pairs(&left_table, &right_table, names)?,
                JoinCondition::Natural => natural_pairs(&left_table, &right_table)?,
                JoinCondition::On(_) | JoinCondition::None => Vec::new(),
            };
            let pair_schema = Schema::new(
                left_table
                    .schema
                    .columns
                    .iter()
                    .chain(right_table.schema.columns.iter())
                    .cloned()
                    .collect(),
            );
            let predicate = match &join.condition {
                JoinCondition::On(expr) => Some(bind_expr(expr, &pair_schema)?),
                JoinCondition::Using(_) | JoinCondition::Natural => {
                    using_predicate(&pairs, left_table.schema.len())
                }
                JoinCondition::None => None,
            };
            let hash_pairs = match &join.condition {
                JoinCondition::Using(_) | JoinCondition::Natural => pairs.clone(),
                JoinCondition::On(_) => predicate
                    .as_ref()
                    .map(|expr| equi_join_pairs(expr, left_table.schema.len()))
                    .unwrap_or_default(),
                JoinCondition::None => Vec::new(),
            };
            let uses_hash = stats
                .and_then(|stats| stats.join_uses_hash.get(join_index).copied())
                .or_else(|| {
                    estimates.and_then(|estimate| estimate.join_uses_hash.get(join_index).copied())
                })
                .unwrap_or_else(|| use_hash_join(self.join_strategy, &hash_pairs));
            let mut join_plan = join_plan(
                join,
                uses_hash,
                &hash_pairs,
                &left_table,
                &right_table,
                predicate.as_ref(),
            )?;
            if let Some(stats) = stats {
                if let (Some(rows), Some(ms)) = (
                    stats.join_rows.get(join_index).copied(),
                    stats.join_ms.get(join_index).copied(),
                ) {
                    join_plan.metrics.actual = Some(rows as u64);
                    join_plan.metrics.actual_ms = Some(ms);
                }
            } else if let Some(rows) =
                estimates.and_then(|estimate| estimate.join_rows.get(join_index).copied())
            {
                join_plan.metrics.est = Some(rows);
            }
            let right_filtered = stats
                .and_then(|stats| stats.scan_filters.get(scan_index).copied())
                .unwrap_or_else(|| pushdown_filter(pushdown, scan_index).is_some());
            let right_filter = plan_scan_filter(pushdown, scan_index, &right_table.schema)?;
            let right_plan = PlanNode::scan(
                right.name.clone(),
                source_alias(right.name.as_str(), &qualifier),
                right_filtered,
                right_filter,
                stats
                    .and_then(|stats| stats.scan_rows.get(scan_index).copied())
                    .map(|rows| rows as u64),
                estimates.and_then(|estimate| estimate.scan_rows.get(scan_index).copied()),
                projection
                    .columns(scan_index)
                    .map(|columns| columns.to_vec()),
            );
            scan_index += 1;
            plan = PlanNode::join(join_plan, plan, right_plan);
            let (schema, _) = build_join_layout(&left_table, &right_table, &pairs, join.kind);
            left_table = Table {
                name: left_table.name,
                schema,
                rows: Vec::new(),
            };
        }

        let filtered_select = select_with_filter(select, pushdown.residual.clone());
        let bound = bind_select(&filtered_select, &left_table)?;
        if let Some(filter) = &bound.filter {
            let metrics = if let Some(stats) = stats {
                Metrics {
                    actual: Some(stats.filter_rows as u64),
                    actual_ms: Some(stats.filter_ms),
                    est: None,
                }
            } else if let Some(estimate) = estimates {
                Metrics {
                    actual: None,
                    actual_ms: None,
                    est: Some(estimate.filter_rows),
                }
            } else {
                Metrics::default()
            };
            let pred = PlanExpr::from_exec(filter, &left_table.schema)?;
            plan = PlanNode::unary(PlanKind::Filter { pred, metrics }, plan);
        }
        if bound.is_grouped() {
            let metrics = if let Some(stats) = stats {
                Metrics {
                    actual: Some(stats.aggregate_groups as u64),
                    actual_ms: Some(stats.aggregate_ms),
                    est: None,
                }
            } else if let Some(estimate) = estimates {
                Metrics {
                    actual: None,
                    actual_ms: None,
                    est: Some(estimate.aggregate_groups),
                }
            } else {
                Metrics::default()
            };
            let keys = bound
                .group_by
                .iter()
                .map(|expr| PlanExpr::from_exec(expr, &left_table.schema))
                .collect::<Result<Vec<_>, _>>()?;
            let mut aggregates = Vec::new();
            for item in &bound.items {
                collect_plan_aggregates(&item.expr, &left_table.schema, &mut aggregates)?;
            }
            if let Some(having) = &bound.having {
                collect_plan_aggregates(having, &left_table.schema, &mut aggregates)?;
            }
            for order in &bound.order_by {
                collect_plan_aggregates(&order.expr, &left_table.schema, &mut aggregates)?;
            }
            plan = PlanNode::unary(
                PlanKind::Aggregate(AggregatePlan {
                    keys,
                    aggregates,
                    metrics,
                }),
                plan,
            );
            if let Some(having) = &bound.having {
                let metrics = if let Some(stats) = stats {
                    Metrics {
                        actual: Some(stats.having_groups as u64),
                        actual_ms: Some(stats.having_ms),
                        est: None,
                    }
                } else if let Some(estimate) = estimates {
                    Metrics {
                        actual: None,
                        actual_ms: None,
                        est: Some(estimate.having_groups),
                    }
                } else {
                    Metrics::default()
                };
                let pred = PlanExpr::from_group_expr(having, &left_table.schema)?;
                plan = PlanNode::unary(PlanKind::Having { pred, metrics }, plan);
            }
        }
        let mut items = Vec::with_capacity(bound.items.len());
        for item in &bound.items {
            items.push(ProjectItem {
                name: item.name().to_string(),
                expr: PlanExpr::from_group_expr(&item.expr, &left_table.schema)?,
            });
        }
        let metrics = if let Some(stats) = stats {
            Metrics {
                actual: Some(stats.project_rows as u64),
                actual_ms: Some(stats.project_ms),
                est: None,
            }
        } else if let Some(estimate) = estimates {
            Metrics {
                actual: None,
                actual_ms: None,
                est: Some(estimate.project_rows),
            }
        } else {
            Metrics::default()
        };
        plan = PlanNode::unary(PlanKind::Project(ProjectPlan { items, metrics }), plan);
        if !bound.order_by.is_empty() {
            let metrics = if let Some(stats) = stats {
                Metrics {
                    actual: Some(stats.sort_rows as u64),
                    actual_ms: Some(stats.sort_ms),
                    est: None,
                }
            } else if let Some(estimate) = estimates {
                Metrics {
                    actual: None,
                    actual_ms: None,
                    est: Some(estimate.sort_rows),
                }
            } else {
                Metrics::default()
            };
            let mut keys = Vec::with_capacity(bound.order_by.len());
            for order in &bound.order_by {
                keys.push(SortKey {
                    expr: PlanExpr::from_group_expr(&order.expr, &left_table.schema)?,
                    ascending: order.ascending,
                });
            }
            plan = PlanNode::unary(PlanKind::Sort(SortPlan { keys, metrics }), plan);
        }
        if let Some(limit) = bound.limit {
            let actual_rows = stats.map(|stats| stats.output_rows as u64);
            let est_rows = estimates.map(|estimate| estimate.output_rows);
            plan = PlanNode::unary(
                PlanKind::Limit {
                    limit,
                    actual_rows,
                    est_rows,
                },
                plan,
            );
        }
        Ok(plan)
    }

    fn put_table(&mut self, table: Table) -> Result<(), SqlError> {
        let name = table.name.clone();
        let key = table_key(&name);
        let stats_key_bytes = stats_key(&name);
        let value = encode_table(&table)?;
        let mut txn = self.store.begin()?;
        txn.put(&key, &value)?;
        // Statistics describe the previous table contents. Invalidate them in
        // the same transaction that rewrites the table so a reopen cannot see
        // old statistics against new rows.
        txn.delete(&stats_key_bytes)?;
        txn.commit()?;
        self.tables.insert(name.clone(), table);
        self.stats.remove(&name);
        Ok(())
    }
}

/// Collect the distinct aggregate calls referenced by one bound group
/// expression. The plan reports this as a query-level inventory; the executor
/// still evaluates each occurrence where it appears.
fn collect_plan_aggregates(
    expr: &GroupExpr,
    schema: &Schema,
    out: &mut Vec<PlanExpr>,
) -> Result<(), SqlError> {
    match expr {
        GroupExpr::Aggregate { .. } => {
            let aggregate = PlanExpr::from_group_expr(expr, schema)?;
            if !out.contains(&aggregate) {
                out.push(aggregate);
            }
        }
        GroupExpr::Scalar(_) => {}
        GroupExpr::IsNull { expr, .. } | GroupExpr::IsTruth { expr, .. } => {
            collect_plan_aggregates(expr, schema, out)?;
        }
        GroupExpr::Truth { expr, threshold } => {
            collect_plan_aggregates(expr, schema, out)?;
            if let Some(threshold) = threshold {
                collect_plan_aggregates(threshold, schema, out)?;
            }
        }
        GroupExpr::Certainty { expr, threshold } => {
            collect_plan_aggregates(expr, schema, out)?;
            if let Some(threshold) = threshold {
                collect_plan_aggregates(threshold, schema, out)?;
            }
        }
        GroupExpr::Not(expr) => collect_plan_aggregates(expr, schema, out)?,
        GroupExpr::InList { expr, values, .. } => {
            collect_plan_aggregates(expr, schema, out)?;
            for value in values {
                collect_plan_aggregates(value, schema, out)?;
            }
        }
        GroupExpr::Between {
            expr, low, high, ..
        } => {
            collect_plan_aggregates(expr, schema, out)?;
            collect_plan_aggregates(low, schema, out)?;
            collect_plan_aggregates(high, schema, out)?;
        }
        GroupExpr::Like { expr, pattern, .. } => {
            collect_plan_aggregates(expr, schema, out)?;
            collect_plan_aggregates(pattern, schema, out)?;
        }
        GroupExpr::Case {
            operand,
            branches,
            else_expr,
        } => {
            if let Some(operand) = operand {
                collect_plan_aggregates(operand, schema, out)?;
            }
            for (when, then) in branches {
                collect_plan_aggregates(when, schema, out)?;
                collect_plan_aggregates(then, schema, out)?;
            }
            if let Some(else_expr) = else_expr {
                collect_plan_aggregates(else_expr, schema, out)?;
            }
        }
        GroupExpr::Binary { left, right, .. } => {
            collect_plan_aggregates(left, schema, out)?;
            collect_plan_aggregates(right, schema, out)?;
        }
    }
    Ok(())
}

fn qualify_table(table: &Table, qualifier: &str) -> Table {
    let mut qualified = qualify_table_schema(table, qualifier);
    qualified.rows = table.rows.clone();
    qualified
}

/// Qualifies and projects a source table in one pass. When `keep` is `None`,
/// or does not select a strict subset, this falls back to a full scan copy.
fn qualify_table_projected(table: &Table, qualifier: &str, keep: Option<&[String]>) -> Table {
    let Some(keep) = keep else {
        return qualify_table(table, qualifier);
    };
    let indices = table
        .schema
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| {
            keep.iter()
                .any(|name| name == &column.name)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if indices.len() == table.schema.len() {
        return qualify_table(table, qualifier);
    }

    let columns = indices
        .iter()
        .map(|&index| {
            let column = &table.schema.columns[index];
            Column {
                name: format!("{qualifier}.{}", column.name),
                data_type: column.data_type,
                not_null: column.not_null,
                unique: column.unique,
                check: column.check.clone(),
            }
        })
        .collect();
    let rows = table
        .rows
        .iter()
        .map(|row| indices.iter().map(|&index| row[index].clone()).collect())
        .collect();
    Table {
        name: table.name.clone(),
        schema: Schema::new(columns),
        rows,
    }
}

fn qualify_table_schema(table: &Table, qualifier: &str) -> Table {
    let columns = table
        .schema
        .columns
        .iter()
        .map(|column| Column {
            name: format!("{qualifier}.{}", column.name),
            data_type: column.data_type,
            not_null: column.not_null,
            unique: column.unique,
            check: column.check.clone(),
        })
        .collect();
    Table {
        name: table.name.clone(),
        schema: Schema::new(columns),
        rows: Vec::new(),
    }
}

/// Splits top-level `AND` conjuncts and assigns source-local ones to scans.
///
/// A conjunct is moved only when every column is explicitly qualified by
/// exactly one source and every join is inner or cross. Conjuncts that mix
/// sources, use unqualified columns, or cross an outer join stay in the
/// residual `WHERE`. Multiple conjuncts assigned to one source are combined
/// with `AND` and evaluated before that source enters the join.
fn plan_filter_pushdown(select: &Select) -> PushdownPlan {
    let mut filters = vec![None; select.joins.len() + 1];
    let Some(filter) = select.filter.as_ref() else {
        return PushdownPlan {
            filters,
            residual: None,
        };
    };
    if select.joins.is_empty()
        || select
            .joins
            .iter()
            .any(|join| !matches!(join.kind, JoinKind::Inner | JoinKind::Cross))
    {
        return PushdownPlan {
            filters,
            residual: Some(filter.clone()),
        };
    }

    let qualifiers = source_qualifiers(select);
    let mut conjuncts = Vec::new();
    split_conjuncts(filter.clone(), &mut conjuncts);
    let mut residual = Vec::new();
    for conjunct in conjuncts {
        let mut referenced = BTreeSet::new();
        if !expr_sources(&conjunct, &mut referenced) || referenced.len() != 1 {
            residual.push(conjunct);
            continue;
        }
        let source = referenced.iter().next().expect("one source");
        let Some(index) = qualifiers.iter().position(|qualifier| qualifier == source) else {
            residual.push(conjunct);
            continue;
        };
        filters[index] = match filters[index].take() {
            Some(existing) => Some(and_expr(existing, conjunct)),
            None => Some(conjunct),
        };
    }
    PushdownPlan {
        filters,
        residual: conjoin(residual),
    }
}

fn split_conjuncts(expr: Expr, conjuncts: &mut Vec<Expr>) {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            split_conjuncts(*left, conjuncts);
            split_conjuncts(*right, conjuncts);
        }
        other => conjuncts.push(other),
    }
}

fn and_expr(left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op: BinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn conjoin(exprs: Vec<Expr>) -> Option<Expr> {
    exprs.into_iter().reduce(and_expr)
}

fn source_qualifiers(select: &Select) -> Vec<String> {
    let mut qualifiers = Vec::with_capacity(select.joins.len() + 1);
    qualifiers.push(
        select
            .table_alias
            .as_deref()
            .unwrap_or(&select.table)
            .to_string(),
    );
    for join in &select.joins {
        qualifiers.push(join.alias.as_deref().unwrap_or(&join.table).to_string());
    }
    qualifiers
}

/// Returns false for an unqualified column: its source is resolved only later
/// by the binder, so it is not safe to move at this stage.
fn expr_sources(expr: &Expr, sources: &mut BTreeSet<String>) -> bool {
    match expr {
        Expr::Window { .. } => false,
        Expr::Literal(_) => true,
        Expr::Column {
            table: Some(table), ..
        } => {
            sources.insert(table.clone());
            true
        }
        Expr::Column { table: None, .. } => false,
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
            expr_sources(expr, sources)
        }
        Expr::Truth { expr, threshold } => {
            let mut ok = expr_sources(expr, sources);
            if let Some(threshold) = threshold {
                ok &= expr_sources(threshold, sources);
            }
            ok
        }
        Expr::Certainty { expr, threshold } => {
            let mut ok = expr_sources(expr, sources);
            if let Some(threshold) = threshold {
                ok &= expr_sources(threshold, sources);
            }
            ok
        }
        Expr::InList { expr, values, .. } => {
            expr_sources(expr, sources) && values.iter().all(|value| expr_sources(value, sources))
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            expr_sources(expr, sources) && expr_sources(low, sources) && expr_sources(high, sources)
        }
        Expr::Like { expr, pattern, .. } => {
            expr_sources(expr, sources) && expr_sources(pattern, sources)
        }
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand
                .as_deref()
                .is_none_or(|expr| expr_sources(expr, sources))
                && branches
                    .iter()
                    .all(|(when, then)| expr_sources(when, sources) && expr_sources(then, sources))
                && else_expr
                    .as_deref()
                    .is_none_or(|expr| expr_sources(expr, sources))
        }
        Expr::Binary { left, right, .. } => {
            expr_sources(left, sources) && expr_sources(right, sources)
        }
        Expr::InSubquery { .. }
        | Expr::InSubqueryTuple { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::Exists { .. }
        | Expr::Quantified { .. } => false,
        Expr::Tuple(exprs) => exprs.iter().all(|expr| expr_sources(expr, sources)),
        Expr::Aggregate { input, .. } => input
            .as_deref()
            .is_none_or(|expr| expr_sources(expr, sources)),
    }
}

/// Collects the unqualified column names needed from each source. Returns
/// `false` when a qualified reference names an unknown source, or when an
/// unqualified reference does not match exactly one source schema.
fn collect_expr_columns(
    expr: &Expr,
    qualifiers: &[String],
    source_columns: &[BTreeSet<String>],
    keep: &mut [BTreeSet<String>],
) -> bool {
    match expr {
        Expr::Window { .. } => false,
        Expr::Literal(_) => true,
        Expr::Column {
            table: Some(table),
            name,
        } => match qualifiers.iter().position(|qualifier| qualifier == table) {
            Some(index) => {
                keep[index].insert(name.clone());
                true
            }
            None => false,
        },
        Expr::Column { table: None, name } => {
            let mut matches = source_columns
                .iter()
                .enumerate()
                .filter(|(_, columns)| columns.contains(name));
            let Some((index, _)) = matches.next() else {
                return false;
            };
            if matches.next().is_some() {
                return false;
            }
            keep[index].insert(name.clone());
            true
        }
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
            collect_expr_columns(expr, qualifiers, source_columns, keep)
        }
        Expr::Truth { expr, threshold } => {
            let mut ok = collect_expr_columns(expr, qualifiers, source_columns, keep);
            if let Some(threshold) = threshold {
                ok &= collect_expr_columns(threshold, qualifiers, source_columns, keep);
            }
            ok
        }
        Expr::Certainty { expr, threshold } => {
            let mut ok = collect_expr_columns(expr, qualifiers, source_columns, keep);
            if let Some(threshold) = threshold {
                ok &= collect_expr_columns(threshold, qualifiers, source_columns, keep);
            }
            ok
        }
        Expr::InList { expr, values, .. } => {
            collect_expr_columns(expr, qualifiers, source_columns, keep)
                && values
                    .iter()
                    .all(|value| collect_expr_columns(value, qualifiers, source_columns, keep))
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_expr_columns(expr, qualifiers, source_columns, keep)
                && collect_expr_columns(low, qualifiers, source_columns, keep)
                && collect_expr_columns(high, qualifiers, source_columns, keep)
        }
        Expr::Like { expr, pattern, .. } => {
            collect_expr_columns(expr, qualifiers, source_columns, keep)
                && collect_expr_columns(pattern, qualifiers, source_columns, keep)
        }
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand
                .as_deref()
                .is_none_or(|expr| collect_expr_columns(expr, qualifiers, source_columns, keep))
                && branches.iter().all(|(when, then)| {
                    collect_expr_columns(when, qualifiers, source_columns, keep)
                        && collect_expr_columns(then, qualifiers, source_columns, keep)
                })
                && else_expr
                    .as_deref()
                    .is_none_or(|expr| collect_expr_columns(expr, qualifiers, source_columns, keep))
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_columns(left, qualifiers, source_columns, keep)
                && collect_expr_columns(right, qualifiers, source_columns, keep)
        }
        Expr::InSubquery { .. }
        | Expr::InSubqueryTuple { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::Exists { .. }
        | Expr::Quantified { .. } => false,
        Expr::Tuple(exprs) => exprs
            .iter()
            .all(|expr| collect_expr_columns(expr, qualifiers, source_columns, keep)),
        Expr::Aggregate { input, .. } => input
            .as_deref()
            .is_none_or(|expr| collect_expr_columns(expr, qualifiers, source_columns, keep)),
    }
}

/// Folds constant expressions and removes only trivially redundant boolean
/// terms. `x AND TRUE`, `TRUE AND x`, `x OR FALSE`, and `FALSE OR x` are exact
/// in SQL three-valued logic. Expressions with columns or aggregates are left
/// for the binder; evaluation errors are preserved by the fallback.
/// Evaluates every `CHECK` on one row. `UNKNOWN` passes, `FALSE` rejects.
fn check_row(schema: &Schema, table_name: &str, row: &[Value]) -> Result<(), SqlError> {
    for column in &schema.columns {
        if let Some(text) = &column.check {
            evaluate_check(schema, table_name, row, text)?;
        }
    }
    for text in &schema.checks {
        evaluate_check(schema, table_name, row, text)?;
    }
    Ok(())
}

fn evaluate_check(
    schema: &Schema,
    table_name: &str,
    row: &[Value],
    text: &str,
) -> Result<(), SqlError> {
    let expr = crate::parser::parse_expression(text)?;
    let bound = bind_expr(&expr, schema)?;
    let value = eval(&bound, schema, row)?;
    if truth_of(&value)? == TruthValue::False {
        return Err(SqlError::CheckViolation {
            table: table_name.to_string(),
            expression: text.to_string(),
        });
    }
    Ok(())
}

/// Rejects duplicate non-`NULL` values in every `UNIQUE` column.
fn check_unique(table: &Table) -> Result<(), SqlError> {
    for (index, column) in table.schema.columns.iter().enumerate() {
        if !column.unique {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        for row in &table.rows {
            let value = &row[index];
            if value.is_null() {
                continue;
            }
            if !seen.insert(value.clone()) {
                return Err(SqlError::DuplicateKey {
                    table: table.name.clone(),
                    column: column.name.clone(),
                    value: value.display(),
                });
            }
        }
    }

    for set in &table.schema.unique_sets {
        let mut seen = std::collections::HashSet::new();
        for row in &table.rows {
            let values: Vec<Value> = set.iter().map(|&index| row[index].clone()).collect();
            // SQL: a tuple with any NULL does not participate in UNIQUE.
            if values.iter().any(Value::is_null) {
                continue;
            }
            if !seen.insert(values.clone()) {
                let column = set
                    .iter()
                    .map(|&index| table.schema.columns[index].name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let value = values
                    .iter()
                    .map(Value::display)
                    .collect::<Vec<_>>()
                    .join(",");
                return Err(SqlError::DuplicateKey {
                    table: table.name.clone(),
                    column,
                    value,
                });
            }
        }
    }
    Ok(())
}

/// Certain rows under the functional dependency `keys -> other columns`.
///
/// A group with conflicting non-key values is discarded entirely: every row in
/// it can be deleted by some minimal repair. A group whose rows agree on the
/// non-key values is kept.
fn filter_under_repair(result: QueryResult, keys: &[String]) -> Result<QueryResult, SqlError> {
    let QueryResult::Rows { columns, rows } = result else {
        return Ok(result);
    };
    let key_indices: Vec<usize> = keys
        .iter()
        .map(|key| {
            columns
                .iter()
                .position(|column| column == key)
                .ok_or_else(|| SqlError::UnknownColumn(key.clone()))
        })
        .collect::<Result<_, _>>()?;

    let mut groups: std::collections::HashMap<Vec<Value>, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let key: Vec<Value> = key_indices
            .iter()
            .map(|&index| row[index].clone())
            .collect();
        groups.entry(key).or_default().push(index);
    }

    let mut conflict = std::collections::HashSet::new();
    for (key, indices) in groups {
        let mut signatures = indices.iter().map(|&index| {
            let mut rest = Vec::with_capacity(rows[index].len() - key_indices.len());
            for (column, value) in rows[index].iter().enumerate() {
                if !key_indices.contains(&column) {
                    rest.push(value.clone());
                }
            }
            rest
        });
        let first = signatures.next().expect("non-empty group");
        if !signatures.all(|signature| signature == first) {
            conflict.insert(key);
        }
    }

    let kept = rows
        .into_iter()
        .filter(|row| {
            let key: Vec<Value> = key_indices
                .iter()
                .map(|&index| row[index].clone())
                .collect();
            !conflict.contains(&key)
        })
        .collect();
    Ok(QueryResult::Rows {
        columns,
        rows: kept,
    })
}

fn fold_select(select: &Select) -> Select {
    let mut folded = select.clone();
    if let Some(filter) = folded.filter.take() {
        let filter = fold_expr(&filter);
        if !is_true_literal(&filter) {
            folded.filter = Some(filter);
        }
    }
    if let Some(having) = folded.having.take() {
        let having = fold_expr(&having);
        if !is_true_literal(&having) {
            folded.having = Some(having);
        }
    }
    for item in &mut folded.items {
        if let SelectItem::Expr { expr, .. } = item {
            *expr = fold_expr(expr);
        }
    }
    for expr in &mut folded.group_by {
        *expr = fold_expr(expr);
    }
    for order in &mut folded.order_by {
        order.expr = fold_expr(&order.expr);
    }
    for join in &mut folded.joins {
        if let JoinCondition::On(expr) = &mut join.condition {
            *expr = fold_expr(expr);
        }
    }
    folded
}

fn is_true_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(Value::Bool(true)))
}

fn is_false_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(Value::Bool(false)))
}

fn fold_constant(expr: Expr) -> Expr {
    if expr_is_constant(&expr) {
        if let Ok(value) = eval_constant(&expr) {
            return Expr::Literal(value);
        }
    }
    expr
}

fn fold_expr(expr: &Expr) -> Expr {
    match expr {
        Expr::Window { .. } => expr.clone(),
        Expr::Literal(_) | Expr::Column { .. } => expr.clone(),
        Expr::IsNull { expr, negated } => fold_constant(Expr::IsNull {
            expr: Box::new(fold_expr(expr)),
            negated: *negated,
        }),
        Expr::IsTruth {
            expr,
            expected,
            negated,
        } => fold_constant(Expr::IsTruth {
            expr: Box::new(fold_expr(expr)),
            expected: *expected,
            negated: *negated,
        }),
        Expr::Truth { expr, threshold } => fold_constant(Expr::Truth {
            expr: Box::new(fold_expr(expr)),
            threshold: threshold
                .as_ref()
                .map(|threshold| Box::new(fold_expr(threshold))),
        }),
        Expr::Certainty { expr, threshold } => fold_constant(Expr::Certainty {
            expr: Box::new(fold_expr(expr)),
            threshold: threshold
                .as_ref()
                .map(|threshold| Box::new(fold_expr(threshold))),
        }),
        Expr::Not(expr) => fold_constant(Expr::Not(Box::new(fold_expr(expr)))),
        Expr::InList {
            expr,
            values,
            negated,
        } => fold_constant(Expr::InList {
            expr: Box::new(fold_expr(expr)),
            values: values.iter().map(fold_expr).collect(),
            negated: *negated,
        }),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => fold_constant(Expr::Between {
            expr: Box::new(fold_expr(expr)),
            low: Box::new(fold_expr(low)),
            high: Box::new(fold_expr(high)),
            negated: *negated,
        }),
        Expr::Like {
            expr,
            pattern,
            negated,
        } => fold_constant(Expr::Like {
            expr: Box::new(fold_expr(expr)),
            pattern: Box::new(fold_expr(pattern)),
            negated: *negated,
        }),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => fold_constant(Expr::Case {
            operand: operand.as_deref().map(|expr| Box::new(fold_expr(expr))),
            branches: branches
                .iter()
                .map(|(when, then)| (fold_expr(when), fold_expr(then)))
                .collect(),
            else_expr: else_expr.as_deref().map(|expr| Box::new(fold_expr(expr))),
        }),
        Expr::Binary { op, left, right } => {
            let left = fold_expr(left);
            let right = fold_expr(right);
            if matches!(*op, BinaryOp::And) {
                if is_true_literal(&left) {
                    return right;
                }
                if is_true_literal(&right) {
                    return left;
                }
            }
            if matches!(*op, BinaryOp::Or) {
                if is_false_literal(&left) {
                    return right;
                }
                if is_false_literal(&right) {
                    return left;
                }
            }
            fold_constant(Expr::Binary {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }
        Expr::InSubquery { .. }
        | Expr::InSubqueryTuple { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::Exists { .. }
        | Expr::Quantified { .. } => expr.clone(),
        Expr::Tuple(exprs) => Expr::Tuple(exprs.iter().map(fold_expr).collect()),
        Expr::Aggregate { .. } => expr.clone(),
    }
}

fn expr_is_constant(expr: &Expr) -> bool {
    match expr {
        Expr::Window { .. } => false,
        Expr::Literal(_) => true,
        Expr::Column { .. } | Expr::Aggregate { .. } => false,
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } | Expr::Not(expr) => {
            expr_is_constant(expr)
        }
        Expr::Truth { expr, threshold } => {
            expr_is_constant(expr) && threshold.as_deref().is_none_or(expr_is_constant)
        }
        Expr::Certainty { expr, threshold } => {
            expr_is_constant(expr) && threshold.as_deref().is_none_or(expr_is_constant)
        }
        Expr::InList { expr, values, .. } => {
            expr_is_constant(expr) && values.iter().all(expr_is_constant)
        }
        Expr::Between {
            expr, low, high, ..
        } => expr_is_constant(expr) && expr_is_constant(low) && expr_is_constant(high),
        Expr::Like { expr, pattern, .. } => expr_is_constant(expr) && expr_is_constant(pattern),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand.as_deref().is_none_or(expr_is_constant)
                && branches
                    .iter()
                    .all(|(when, then)| expr_is_constant(when) && expr_is_constant(then))
                && else_expr.as_deref().is_none_or(expr_is_constant)
        }
        Expr::Binary { left, right, .. } => expr_is_constant(left) && expr_is_constant(right),
        Expr::InSubquery { expr, .. } => expr_is_constant(expr),
        Expr::InSubqueryTuple { exprs, .. } => exprs.iter().all(expr_is_constant),
        Expr::Quantified { expr, .. } => expr_is_constant(expr),
        Expr::Tuple(exprs) => exprs.iter().all(expr_is_constant),
        Expr::ScalarSubquery { .. } | Expr::Exists { .. } => false,
    }
}

fn select_with_filter(select: &Select, filter: Option<Expr>) -> Select {
    let mut filtered = select.clone();
    filtered.filter = filter;
    filtered
}

fn pushdown_filter(pushdown: &PushdownPlan, index: usize) -> Option<&Expr> {
    pushdown.filters.get(index).and_then(Option::as_ref)
}

/// Binds a pushed-down source filter against the full source schema and
/// converts it to the read-only plan expression shape.
fn plan_scan_filter(
    pushdown: &PushdownPlan,
    index: usize,
    schema: &Schema,
) -> Result<Option<PlanExpr>, SqlError> {
    pushdown_filter(pushdown, index)
        .map(|filter| {
            let bound = bind_expr(filter, schema)?;
            PlanExpr::from_exec(&bound, schema)
        })
        .transpose()
}

fn filter_table(table: Table, filter: &Expr) -> Result<Table, SqlError> {
    let Table { name, schema, rows } = table;
    let bound = bind_expr(filter, &schema)?;
    let rows = rows
        .iter()
        .filter_map(|row| match passes_filter(Some(&bound), &schema, row) {
            Ok(true) => Some(Ok(row.clone())),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Table { name, schema, rows })
}

fn estimate_scan_rows(
    table: &Table,
    stats: &TableStats,
    qualifier: &str,
    filter: Option<&Expr>,
) -> Result<f64, SqlError> {
    let rows = stats.rows as f64;
    let Some(filter) = filter else {
        return Ok(rows);
    };
    let source_table = qualify_table_schema(table, qualifier);
    let bound = bind_expr(filter, &source_table.schema)?;
    let ndvs = stats
        .columns
        .iter()
        .map(|column| column.distinct)
        .collect::<Vec<_>>();
    Ok(rows * estimate_selectivity(&bound, &ndvs))
}

#[derive(Clone, Copy, Debug)]
enum JoinSource {
    Left(usize),
    Right(usize),
    Coalesced { left: usize, right: usize },
}

fn unqualified_name(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, column)| column)
}

fn find_join_column(schema: &Schema, name: &str) -> Result<usize, SqlError> {
    if let Some(index) = schema.index_of(name) {
        return Ok(index);
    }
    let mut matches = schema
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| (unqualified_name(&column.name) == name).then_some(index));
    let Some(index) = matches.next() else {
        return Err(SqlError::UnknownColumn(name.to_string()));
    };
    if matches.next().is_some() {
        return Err(SqlError::AmbiguousColumn(name.to_string()));
    }
    Ok(index)
}

fn check_join_types(left: &Column, right: &Column) -> Result<(), SqlError> {
    if left.data_type != right.data_type {
        return Err(ExecError::TypeMismatch {
            expected: left.data_type.name(),
            actual: right.data_type.name(),
        }
        .into());
    }
    Ok(())
}

fn using_pairs(
    left: &Table,
    right: &Table,
    names: &[String],
) -> Result<Vec<(usize, usize)>, SqlError> {
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::with_capacity(names.len());
    for name in names {
        if !seen.insert(name.clone()) {
            return Err(SqlError::Unsupported(format!(
                "duplicate USING column: {name}"
            )));
        }
        let left_index = find_join_column(&left.schema, name)?;
        let right_index = find_join_column(&right.schema, name)?;
        check_join_types(
            &left.schema.columns[left_index],
            &right.schema.columns[right_index],
        )?;
        pairs.push((left_index, right_index));
    }
    Ok(pairs)
}

fn natural_pairs(left: &Table, right: &Table) -> Result<Vec<(usize, usize)>, SqlError> {
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::new();
    for (left_index, left_column) in left.schema.columns.iter().enumerate() {
        let name = unqualified_name(&left_column.name);
        if !seen.insert(name.to_string()) {
            return Err(SqlError::AmbiguousColumn(name.to_string()));
        }
        let right_index = match find_join_column(&right.schema, name) {
            Ok(index) => index,
            Err(
                SqlError::UnknownColumn(_)
                | SqlError::UnknownName {
                    kind: NameKind::Column,
                    ..
                },
            ) => continue,
            Err(error) => return Err(error),
        };
        check_join_types(left_column, &right.schema.columns[right_index])?;
        pairs.push((left_index, right_index));
    }
    Ok(pairs)
}

fn build_join_layout(
    left: &Table,
    right: &Table,
    pairs: &[(usize, usize)],
    kind: JoinKind,
) -> (Schema, Vec<JoinSource>) {
    let mut columns = Vec::new();
    let mut sources = Vec::new();
    let mut aliases = BTreeMap::new();
    let mut used_left = BTreeSet::new();
    let mut used_right = BTreeSet::new();

    for &(left_index, right_index) in pairs {
        used_left.insert(left_index);
        used_right.insert(right_index);
        let left_column = &left.schema.columns[left_index];
        let right_column = &right.schema.columns[right_index];
        let name = unqualified_name(&left_column.name).to_string();
        let not_null = match kind {
            JoinKind::Inner | JoinKind::Asof => left_column.not_null || right_column.not_null,
            JoinKind::Left => left_column.not_null,
            JoinKind::Right => right_column.not_null,
            JoinKind::Full | JoinKind::Cross => false,
        };
        let output_index = columns.len();
        columns.push(Column {
            name: name.clone(),
            data_type: left_column.data_type,
            not_null,
            unique: false,
            check: None,
        });
        sources.push(JoinSource::Coalesced {
            left: left_index,
            right: right_index,
        });
        aliases.insert(left_column.name.clone(), output_index);
        aliases.insert(right_column.name.clone(), output_index);
        for (alias, index) in left.schema.aliases() {
            if *index == left_index {
                aliases.insert(alias.clone(), output_index);
            }
        }
        for (alias, index) in right.schema.aliases() {
            if *index == right_index {
                aliases.insert(alias.clone(), output_index);
            }
        }
    }

    for (index, column) in left.schema.columns.iter().enumerate() {
        if used_left.contains(&index) {
            continue;
        }
        let not_null = match kind {
            JoinKind::Right | JoinKind::Full => false,
            _ => column.not_null,
        };
        let output_index = columns.len();
        columns.push(Column {
            name: column.name.clone(),
            data_type: column.data_type,
            not_null,
            unique: false,
            check: None,
        });
        sources.push(JoinSource::Left(index));
        aliases.insert(column.name.clone(), output_index);
        for (alias, alias_index) in left.schema.aliases() {
            if *alias_index == index {
                aliases.insert(alias.clone(), output_index);
            }
        }
    }

    for (index, column) in right.schema.columns.iter().enumerate() {
        if used_right.contains(&index) {
            continue;
        }
        let not_null = match kind {
            JoinKind::Left | JoinKind::Full | JoinKind::Asof => false,
            _ => column.not_null,
        };
        let output_index = columns.len();
        columns.push(Column {
            name: column.name.clone(),
            data_type: column.data_type,
            not_null,
            unique: false,
            check: None,
        });
        sources.push(JoinSource::Right(index));
        aliases.insert(column.name.clone(), output_index);
        for (alias, alias_index) in right.schema.aliases() {
            if *alias_index == index {
                aliases.insert(alias.clone(), output_index);
            }
        }
    }

    (Schema::new(columns).with_aliases(aliases), sources)
}

fn using_predicate(pairs: &[(usize, usize)], left_len: usize) -> Option<ExecExpr> {
    let mut expressions = pairs
        .iter()
        .map(|&(left_index, right_index)| ExecExpr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(ExecExpr::Column(left_index)),
            right: Box::new(ExecExpr::Column(left_len + right_index)),
        });
    let first = expressions.next()?;
    Some(expressions.fold(first, |left, right| ExecExpr::Binary {
        op: BinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    }))
}

fn project_join_row(left: &Row, right: &Row, sources: &[JoinSource]) -> Row {
    sources
        .iter()
        .map(|source| match source {
            JoinSource::Left(index) => left[*index].clone(),
            JoinSource::Right(index) => right[*index].clone(),
            JoinSource::Coalesced {
                left: left_index,
                right: right_index,
            } => {
                if left[*left_index].is_null() {
                    right[*right_index].clone()
                } else {
                    left[*left_index].clone()
                }
            }
        })
        .collect()
}

fn equi_pair(left: &ExecExpr, right: &ExecExpr, left_len: usize) -> Option<(usize, usize)> {
    match (left, right) {
        (ExecExpr::Column(left_index), ExecExpr::Column(right_index))
            if *left_index < left_len && *right_index >= left_len =>
        {
            Some((*left_index, *right_index - left_len))
        }
        (ExecExpr::Column(left_index), ExecExpr::Column(right_index))
            if *right_index < left_len && *left_index >= left_len =>
        {
            Some((*right_index, *left_index - left_len))
        }
        _ => None,
    }
}

fn collect_equi_pairs(expr: &ExecExpr, left_len: usize, pairs: &mut Vec<(usize, usize)>) {
    match expr {
        ExecExpr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            collect_equi_pairs(left, left_len, pairs);
            collect_equi_pairs(right, left_len, pairs);
        }
        ExecExpr::Binary {
            op: BinaryOp::Eq,
            left,
            right,
        } => {
            if let Some(pair) = equi_pair(left, right, left_len) {
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        _ => {}
    }
}

fn equi_join_pairs(expr: &ExecExpr, left_len: usize) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    collect_equi_pairs(expr, left_len, &mut pairs);
    pairs
}

fn hash_key(row: &Row, keys: &[usize]) -> Option<Vec<Value>> {
    let mut key = Vec::with_capacity(keys.len());
    for &index in keys {
        if row[index].is_null() {
            return None;
        }
        key.push(row[index].clone());
    }
    Some(key)
}

/// Appends one join row, refusing to grow past [`MAX_JOIN_ROWS`].
fn push_join_row(rows: &mut Vec<Row>, row: Row) -> Result<(), SqlError> {
    if rows.len() >= MAX_JOIN_ROWS {
        return Err(SqlError::Unsupported(format!(
            "join produces more than {MAX_JOIN_ROWS} rows"
        )));
    }
    rows.push(row);
    Ok(())
}

fn hash_join_rows(
    left: &Table,
    right: &Table,
    sources: &[JoinSource],
    kind: JoinKind,
    predicate: Option<&ExecExpr>,
    pair_schema: &Schema,
    pairs: &[(usize, usize)],
) -> Result<Vec<Row>, SqlError> {
    let left_keys: Vec<usize> = pairs.iter().map(|&(left_index, _)| left_index).collect();
    let right_keys: Vec<usize> = pairs.iter().map(|&(_, right_index)| right_index).collect();
    let mut buckets: HashMap<Vec<Value>, Vec<usize>> = HashMap::new();
    for (index, right_row) in right.rows.iter().enumerate() {
        if let Some(key) = hash_key(right_row, &right_keys) {
            buckets.entry(key).or_default().push(index);
        }
    }

    let mut rows = Vec::new();
    let mut right_matched = vec![false; right.rows.len()];
    let null_left = vec![Value::Null; left.schema.len()];
    let null_right = vec![Value::Null; right.schema.len()];
    for left_row in &left.rows {
        let mut matched = false;
        let key = hash_key(left_row, &left_keys);
        if let Some(candidates) = key.as_ref().and_then(|key| buckets.get(key)) {
            for &index in candidates {
                let right_row = &right.rows[index];
                let mut pair = left_row.clone();
                pair.extend(right_row.iter().cloned());
                if passes_filter(predicate, pair_schema, &pair)? {
                    matched = true;
                    right_matched[index] = true;
                    push_join_row(&mut rows, project_join_row(left_row, right_row, sources))?;
                }
            }
        }
        if matches!(kind, JoinKind::Left | JoinKind::Full) && !matched {
            push_join_row(&mut rows, project_join_row(left_row, &null_right, sources))?;
        }
    }
    if matches!(kind, JoinKind::Right | JoinKind::Full) {
        for (index, right_row) in right.rows.iter().enumerate() {
            if right_matched[index] {
                continue;
            }
            push_join_row(&mut rows, project_join_row(&null_left, right_row, sources))?;
        }
    }
    Ok(rows)
}

fn nested_loop_join_rows(
    left: &Table,
    right: &Table,
    sources: &[JoinSource],
    kind: JoinKind,
    predicate: Option<&ExecExpr>,
    pair_schema: &Schema,
) -> Result<Vec<Row>, SqlError> {
    let mut rows = Vec::new();
    let mut right_matched = vec![false; right.rows.len()];
    let null_left = vec![Value::Null; left.schema.len()];
    let null_right = vec![Value::Null; right.schema.len()];
    for left_row in &left.rows {
        let mut matched = false;
        for (index, right_row) in right.rows.iter().enumerate() {
            let mut pair = left_row.clone();
            pair.extend(right_row.iter().cloned());
            if passes_filter(predicate, pair_schema, &pair)? {
                matched = true;
                right_matched[index] = true;
                push_join_row(&mut rows, project_join_row(left_row, right_row, sources))?;
            }
        }
        if matches!(kind, JoinKind::Left | JoinKind::Full) && !matched {
            push_join_row(&mut rows, project_join_row(left_row, &null_right, sources))?;
        }
    }
    if matches!(kind, JoinKind::Right | JoinKind::Full) {
        for (index, right_row) in right.rows.iter().enumerate() {
            if right_matched[index] {
                continue;
            }
            push_join_row(&mut rows, project_join_row(&null_left, right_row, sources))?;
        }
    }
    Ok(rows)
}

fn use_hash_join(strategy: JoinStrategy, pairs: &[(usize, usize)]) -> bool {
    match strategy {
        JoinStrategy::NestedLoop => false,
        JoinStrategy::Auto | JoinStrategy::Hash => !pairs.is_empty(),
    }
}

/// One hash-build row, one hash-probe row, and one output row are treated as
/// comparable units. This is deliberately a local join heuristic, not a
/// global optimizer: nested loop performs `left * right` predicate probes,
/// while hash join builds the right side and probes the left side.
const HASH_BUILD_ROW_COST: f64 = 1.0;
const HASH_PROBE_ROW_COST: f64 = 1.0;
const HASH_OUTPUT_ROW_COST: f64 = 1.0;

fn choose_join_algorithm(
    strategy: JoinStrategy,
    kind: JoinKind,
    left_rows: f64,
    right_rows: f64,
    hash_pairs: &[(usize, usize)],
    predicate: Option<&ExecExpr>,
    stats: JoinStats<'_>,
) -> bool {
    if hash_pairs.is_empty() {
        return false;
    }
    match strategy {
        JoinStrategy::Hash => true,
        JoinStrategy::NestedLoop => false,
        JoinStrategy::Auto => {
            // Without statistics the historical fallback is hash-by-key.
            if !stats.available {
                return true;
            }
            let output_rows = estimate_join_rows(
                kind,
                left_rows,
                right_rows,
                hash_pairs,
                stats.left_ndvs,
                stats.right_ndvs,
                predicate,
            );
            let hash_cost = right_rows * HASH_BUILD_ROW_COST
                + left_rows * HASH_PROBE_ROW_COST
                + output_rows * HASH_OUTPUT_ROW_COST;
            let nested_cost = left_rows * right_rows;
            // Ties keep the historical hash-by-key default.
            hash_cost <= nested_cost
        }
    }
}

fn ceil_estimate(value: f64) -> u64 {
    if value.is_finite() {
        value.max(0.0).ceil() as u64
    } else {
        0
    }
}

fn estimate_join_rows(
    kind: JoinKind,
    left_rows: f64,
    right_rows: f64,
    hash_pairs: &[(usize, usize)],
    left_ndvs: &[u64],
    right_ndvs: &[u64],
    predicate: Option<&ExecExpr>,
) -> f64 {
    let product = left_rows * right_rows;
    let mut rows = if hash_pairs.is_empty() {
        if predicate.is_some() {
            product * 0.1
        } else {
            product
        }
    } else {
        let mut denominator = 1.0;
        for &(left_index, right_index) in hash_pairs {
            let left_ndv = left_ndvs.get(left_index).copied().unwrap_or(0).max(1) as f64;
            let right_ndv = right_ndvs.get(right_index).copied().unwrap_or(0).max(1) as f64;
            denominator *= left_ndv.max(right_ndv);
        }
        (product / denominator).min(product)
    };
    if !hash_pairs.is_empty()
        && predicate.is_some_and(|predicate| !is_pure_equi_conjunction(predicate))
    {
        rows *= 0.5;
    }
    match kind {
        JoinKind::Inner => rows,
        JoinKind::Left => rows.max(left_rows),
        JoinKind::Right => rows.max(right_rows),
        JoinKind::Full => rows.max(left_rows).max(right_rows),
        JoinKind::Cross => product,
        JoinKind::Asof => left_rows,
    }
}

fn join_output_ndvs(sources: &[JoinSource], left_ndvs: &[u64], right_ndvs: &[u64]) -> Vec<u64> {
    sources
        .iter()
        .map(|source| match source {
            JoinSource::Left(index) => left_ndvs.get(*index).copied().unwrap_or(0),
            JoinSource::Right(index) => right_ndvs.get(*index).copied().unwrap_or(0),
            JoinSource::Coalesced { left, right } => left_ndvs
                .get(*left)
                .copied()
                .unwrap_or(0)
                .max(right_ndvs.get(*right).copied().unwrap_or(0)),
        })
        .collect()
}

fn estimate_group_count(grouped: &BoundSelect, input_rows: f64, ndvs: &[u64]) -> f64 {
    if grouped.group_by.is_empty() {
        return 1.0;
    }
    let mut groups = 1.0;
    for expr in &grouped.group_by {
        match expr {
            ExecExpr::Column(index) => {
                groups *= ndvs.get(*index).copied().unwrap_or(0).max(1) as f64;
            }
            _ => {
                groups = input_rows.max(1.0);
                break;
            }
        }
    }
    groups
        .min(input_rows)
        .max(if input_rows > 0.0 { 1.0 } else { 0.0 })
}

fn estimate_selectivity(expr: &ExecExpr, ndvs: &[u64]) -> f64 {
    let selectivity = match expr {
        ExecExpr::Literal(Value::Bool(true)) => 1.0,
        ExecExpr::Literal(Value::Bool(false)) | ExecExpr::Literal(Value::Null) => 0.0,
        ExecExpr::IsNull { negated, .. } => {
            if *negated {
                0.9
            } else {
                0.1
            }
        }
        ExecExpr::IsTruth { .. } => 0.5,
        ExecExpr::Not(expr) => 1.0 - estimate_selectivity(expr, ndvs),
        ExecExpr::InList {
            expr,
            values,
            negated,
        } => {
            let mut value = 0.5;
            if let ExecExpr::Column(index) = expr.as_ref() {
                value = (values.len() as f64
                    / ndvs.get(*index).copied().unwrap_or(0).max(1) as f64)
                    .min(1.0);
            }
            if *negated {
                1.0 - value
            } else {
                value
            }
        }
        ExecExpr::Between { negated, .. } => {
            if *negated {
                0.67
            } else {
                0.33
            }
        }
        ExecExpr::Like { negated, .. } => {
            if *negated {
                0.9
            } else {
                0.1
            }
        }
        ExecExpr::Case { .. } => 0.5,
        ExecExpr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => estimate_selectivity(left, ndvs) * estimate_selectivity(right, ndvs),
        ExecExpr::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } => {
            let left = estimate_selectivity(left, ndvs);
            let right = estimate_selectivity(right, ndvs);
            1.0 - (1.0 - left) * (1.0 - right)
        }
        ExecExpr::Binary {
            op: BinaryOp::Eq,
            left,
            right,
        } => equality_selectivity(left, right, ndvs),
        ExecExpr::Binary {
            op: BinaryOp::Ne,
            left,
            right,
        } => 1.0 - equality_selectivity(left, right, ndvs),
        ExecExpr::Binary {
            op: BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge,
            ..
        } => 0.33,
        _ => 0.5,
    };
    selectivity.clamp(0.0, 1.0)
}

fn equality_selectivity(left: &ExecExpr, right: &ExecExpr, ndvs: &[u64]) -> f64 {
    if matches!(left, ExecExpr::Literal(Value::Null))
        || matches!(right, ExecExpr::Literal(Value::Null))
    {
        return 0.0;
    }
    match (left, right) {
        (ExecExpr::Column(index), ExecExpr::Literal(_))
        | (ExecExpr::Literal(_), ExecExpr::Column(index)) => {
            1.0 / ndvs.get(*index).copied().unwrap_or(0).max(1) as f64
        }
        _ => 0.5,
    }
}

fn source_alias(table: &str, qualifier: &str) -> Option<String> {
    (table != qualifier).then(|| qualifier.to_string())
}

fn is_pure_equi_conjunction(expr: &ExecExpr) -> bool {
    match expr {
        ExecExpr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => is_pure_equi_conjunction(left) && is_pure_equi_conjunction(right),
        ExecExpr::Binary {
            op: BinaryOp::Eq,
            left,
            right,
        } => matches!(
            (left.as_ref(), right.as_ref()),
            (ExecExpr::Column(_), ExecExpr::Column(_))
        ),
        _ => false,
    }
}

/// Remove the top-level conjuncts already represented by hash keys. The
/// remaining tree keeps its original conjunction shape and is cloned only at
/// the leaves that survive.
fn residual_join_predicate(
    expr: &ExecExpr,
    left_len: usize,
    keys: &[(usize, usize)],
) -> Option<ExecExpr> {
    match expr {
        ExecExpr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => match (
            residual_join_predicate(left, left_len, keys),
            residual_join_predicate(right, left_len, keys),
        ) {
            (None, None) => None,
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (Some(left), Some(right)) => Some(ExecExpr::Binary {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            }),
        },
        ExecExpr::Binary {
            op: BinaryOp::Eq,
            left,
            right,
        } => match equi_pair(left, right, left_len) {
            Some(pair) if keys.contains(&pair) => None,
            _ => Some(expr.clone()),
        },
        _ => Some(expr.clone()),
    }
}

fn join_plan(
    join: &Join,
    uses_hash: bool,
    hash_pairs: &[(usize, usize)],
    left: &Table,
    right: &Table,
    predicate: Option<&ExecExpr>,
) -> Result<JoinPlan, SqlError> {
    let (keys, residual, on_expr) = match &join.condition {
        JoinCondition::Using(names) => {
            let keys = if names.is_empty() {
                None
            } else {
                Some(JoinKeys::Names(names.clone()))
            };
            (keys, false, false)
        }
        JoinCondition::Natural => {
            let keys = if hash_pairs.is_empty() {
                None
            } else {
                let names = hash_pairs
                    .iter()
                    .map(|(left_index, _)| {
                        unqualified_name(&left.schema.columns[*left_index].name).to_string()
                    })
                    .collect::<Vec<_>>();
                Some(JoinKeys::Names(names))
            };
            (keys, false, false)
        }
        JoinCondition::On(_) => {
            if uses_hash {
                let pairs = hash_pairs
                    .iter()
                    .map(|(left_index, right_index)| JoinKey {
                        left: left.schema.columns[*left_index].name.clone(),
                        right: right.schema.columns[*right_index].name.clone(),
                    })
                    .collect::<Vec<_>>();
                let residual = predicate.is_some_and(|expr| !is_pure_equi_conjunction(expr));
                (Some(JoinKeys::Pairs(pairs)), residual, false)
            } else if predicate.is_some() {
                (None, false, true)
            } else {
                (None, false, false)
            }
        }
        JoinCondition::None => (None, false, false),
    };
    let pair_schema = Schema::new(
        left.schema
            .columns
            .iter()
            .chain(right.schema.columns.iter())
            .cloned()
            .collect(),
    );
    let pred = predicate
        .map(|expr| PlanExpr::from_exec(expr, &pair_schema))
        .transpose()?;
    let residual_pred = if uses_hash {
        predicate
            .and_then(|expr| residual_join_predicate(expr, left.schema.len(), hash_pairs))
            .map(|expr| PlanExpr::from_exec(&expr, &pair_schema))
            .transpose()?
    } else {
        None
    };
    Ok(JoinPlan {
        uses_hash,
        kind: join.kind,
        keys,
        residual,
        residual_pred,
        on_expr,
        pred,
        metrics: Metrics::default(),
    })
}

fn join_tables(
    left: &Table,
    right: &Table,
    condition: &JoinCondition,
    kind: JoinKind,
    strategy: JoinStrategy,
    stats: JoinStats<'_>,
) -> Result<(Table, Vec<JoinSource>, bool), SqlError> {
    if matches!(kind, JoinKind::Asof) {
        return asof_join_tables(left, right, condition, kind);
    }
    let pairs = match condition {
        JoinCondition::Using(names) => using_pairs(left, right, names)?,
        JoinCondition::Natural => natural_pairs(left, right)?,
        JoinCondition::None | JoinCondition::On(_) => Vec::new(),
    };
    let (schema, sources) = build_join_layout(left, right, &pairs, kind);
    let pair_schema = Schema::new(
        left.schema
            .columns
            .iter()
            .chain(right.schema.columns.iter())
            .cloned()
            .collect(),
    );
    let predicate = match condition {
        JoinCondition::On(expr) => Some(bind_expr(expr, &pair_schema)?),
        JoinCondition::Using(_) | JoinCondition::Natural => {
            using_predicate(&pairs, left.schema.len())
        }
        JoinCondition::None => None,
    };
    let hash_pairs = match condition {
        JoinCondition::Using(_) | JoinCondition::Natural => pairs.clone(),
        JoinCondition::On(_) => predicate
            .as_ref()
            .map(|expr| equi_join_pairs(expr, left.schema.len()))
            .unwrap_or_default(),
        JoinCondition::None => Vec::new(),
    };
    let uses_hash = choose_join_algorithm(
        strategy,
        kind,
        left.rows.len() as f64,
        right.rows.len() as f64,
        &hash_pairs,
        predicate.as_ref(),
        stats,
    );
    let rows = if uses_hash {
        hash_join_rows(
            left,
            right,
            &sources,
            kind,
            predicate.as_ref(),
            &pair_schema,
            &hash_pairs,
        )?
    } else {
        nested_loop_join_rows(
            left,
            right,
            &sources,
            kind,
            predicate.as_ref(),
            &pair_schema,
        )?
    };
    Ok((
        Table {
            name: format!("{}+{}", left.name, right.name),
            schema,
            rows,
        },
        sources,
        uses_hash,
    ))
}

/// As-of join: each left row pairs with the last right row whose time is
/// `<=` the left row's time.
///
/// The right side is sorted by time at execution (stable), rows with a `NULL`
/// time never match, and every left row survives (right columns `NULL` when
/// there is no match).
fn asof_join_tables(
    left: &Table,
    right: &Table,
    condition: &JoinCondition,
    kind: JoinKind,
) -> Result<(Table, Vec<JoinSource>, bool), SqlError> {
    let (equi_pairs, (left_time, right_time)) = asof_keys(left, right, condition)?;
    let (schema, sources) = build_join_layout(left, right, &[], kind);

    // Group the right side by the equality keys, then sort each group by time
    // (stable: equal times keep their original order, so the last one wins).
    // Rows with a NULL key or NULL time never match.
    let mut groups: std::collections::BTreeMap<Vec<String>, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (index, row) in right.rows.iter().enumerate() {
        if row[right_time].is_null() {
            continue;
        }
        let Some(key) = asof_key(row, &equi_pairs, true) else {
            continue;
        };
        groups.entry(key).or_default().push(index);
    }
    let groups: std::collections::BTreeMap<Vec<String>, (Vec<usize>, Vec<i64>)> = groups
        .into_iter()
        .map(|(key, mut indices)| {
            indices.sort_by_key(|&index| {
                asof_time(&right.rows[index][right_time]).unwrap_or(i64::MIN)
            });
            let times = indices
                .iter()
                .map(|&index| asof_time(&right.rows[index][right_time]).expect("non-null filtered"))
                .collect();
            (key, (indices, times))
        })
        .collect();

    let null_right = vec![Value::Null; right.schema.len()];
    let mut rows = Vec::with_capacity(left.rows.len());
    for left_row in &left.rows {
        let matched = asof_match(left_row, &equi_pairs, left_time, &groups);
        let right_row = matched
            .map(|right_index| right.rows[right_index].as_slice())
            .unwrap_or(null_right.as_slice());
        let mut row = left_row.clone();
        row.extend(right_row.iter().cloned());
        rows.push(row);
    }

    Ok((
        Table {
            name: format!("{}+{}", left.name, right.name),
            schema,
            rows,
        },
        sources,
        false,
    ))
}

/// Equality key pairs and the single time pair of an ASOF JOIN.
type AsofKeys = (Vec<(usize, usize)>, (usize, usize));

/// Parses `ON <left>.k = <right>.k [AND ...] AND <left>.ts >= <right>.ts`.
///
/// Returns the equality key pairs and the single time pair. Anything else is
/// rejected with `Unsupported`; a non-`INT` time column is a `TypeMismatch`.
fn asof_keys(left: &Table, right: &Table, condition: &JoinCondition) -> Result<AsofKeys, SqlError> {
    let unsupported = || {
        SqlError::Unsupported(
            "ASOF JOIN needs `ON <left>.k = <right>.k [AND ...] AND <left>.ts >= <right>.ts`"
                .to_string(),
        )
    };
    let JoinCondition::On(expr) = condition else {
        return Err(unsupported());
    };
    let mut terms = Vec::new();
    flatten_and(expr, &mut terms);

    let mut equi_pairs = Vec::new();
    let mut time_pair = None;
    for term in terms {
        let Expr::Binary {
            op,
            left: left_expr,
            right: right_expr,
        } = term
        else {
            return Err(unsupported());
        };
        let (
            Expr::Column {
                table: left_table,
                name: left_name,
            },
            Expr::Column {
                table: right_table,
                name: right_name,
            },
        ) = (&**left_expr, &**right_expr)
        else {
            return Err(unsupported());
        };
        match op {
            BinaryOp::Eq => {
                equi_pairs.push(asof_pair(
                    left,
                    right,
                    left_table.as_deref(),
                    left_name,
                    right_table.as_deref(),
                    right_name,
                )?);
            }
            BinaryOp::Ge => {
                if time_pair.is_some() {
                    return Err(unsupported());
                }
                let pair = asof_pair(
                    left,
                    right,
                    left_table.as_deref(),
                    left_name,
                    right_table.as_deref(),
                    right_name,
                )?;
                let column = &left.schema.columns[pair.0];
                if column.data_type != DataType::Int {
                    return Err(SqlError::Exec(ExecError::TypeMismatch {
                        expected: "INT",
                        actual: column.data_type.name(),
                    }));
                }
                let column = &right.schema.columns[pair.1];
                if column.data_type != DataType::Int {
                    return Err(SqlError::Exec(ExecError::TypeMismatch {
                        expected: "INT",
                        actual: column.data_type.name(),
                    }));
                }
                time_pair = Some(pair);
            }
            _ => return Err(unsupported()),
        }
    }
    let Some(time_pair) = time_pair else {
        return Err(SqlError::Unsupported(
            "ASOF JOIN needs a time key `ON ... <left>.ts >= <right>.ts`".to_string(),
        ));
    };
    Ok((equi_pairs, time_pair))
}

fn flatten_and<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            flatten_and(left, out);
            flatten_and(right, out);
        }
        _ => out.push(expr),
    }
}

/// Resolves a `left.col op right.col` pair, tolerating the swapped spelling.
fn asof_pair(
    left: &Table,
    right: &Table,
    left_table: Option<&str>,
    left_name: &str,
    right_table: Option<&str>,
    right_name: &str,
) -> Result<(usize, usize), SqlError> {
    if let (Ok(left_index), Ok(right_index)) = (
        asof_column(left, left_table, left_name),
        asof_column(right, right_table, right_name),
    ) {
        return Ok((left_index, right_index));
    }
    let left_index = asof_column(left, right_table, right_name)?;
    let right_index = asof_column(right, left_table, left_name)?;
    Ok((left_index, right_index))
}

/// Equality-key for one row; `None` when any key is `NULL` (they never match).
fn asof_key(row: &Row, pairs: &[(usize, usize)], right_side: bool) -> Option<Vec<String>> {
    pairs
        .iter()
        .map(|&(left_index, right_index)| {
            let value = &row[if right_side { right_index } else { left_index }];
            if value.is_null() {
                None
            } else {
                Some(format!("{value:?}"))
            }
        })
        .collect()
}

fn asof_match(
    left_row: &Row,
    pairs: &[(usize, usize)],
    left_time: usize,
    groups: &std::collections::BTreeMap<Vec<String>, (Vec<usize>, Vec<i64>)>,
) -> Option<usize> {
    if left_row[left_time].is_null() {
        return None;
    }
    let key = asof_key(left_row, pairs, false)?;
    let (order, times) = groups.get(&key)?;
    let timestamp = asof_time(&left_row[left_time])?;
    let count = times.partition_point(|value| *value <= timestamp);
    if count == 0 {
        None
    } else {
        Some(order[count - 1])
    }
}

fn asof_column(schema_table: &Table, table: Option<&str>, name: &str) -> Result<usize, SqlError> {
    if let Some(table) = table {
        let qualified = format!("{table}.{name}");
        return schema_table
            .schema
            .index_of(&qualified)
            .ok_or(SqlError::UnknownColumn(qualified));
    }
    schema_table
        .schema
        .index_of(name)
        .ok_or_else(|| SqlError::UnknownColumn(name.to_string()))
}

fn asof_time(value: &Value) -> Option<i64> {
    match value {
        Value::Int(value) => Some(*value),
        _ => None,
    }
}

fn passes_filter(filter: Option<&ExecExpr>, schema: &Schema, row: &Row) -> Result<bool, SqlError> {
    let Some(filter) = filter else {
        return Ok(true);
    };
    Ok(truth_of(&eval(filter, schema, row)?)? == TruthValue::True)
}

/// Builds a batch predicate for `expr`, or `None` when it must stay row-wise.
///
/// Only `AND`/`OR`/`NOT` over `LOGIC K3` / `LOGIC BELNAP` columns qualify;
/// comparisons, arithmetic, aggregates and subqueries do not.
fn build_tri_predicate(
    expr: &ExecExpr,
    schema: &Schema,
    atoms: &mut Vec<usize>,
) -> Option<TriPredicate> {
    match expr {
        ExecExpr::Column(index) => {
            let column = schema.columns.get(*index)?;
            if !matches!(column.data_type, DataType::Logic | DataType::Belnap) {
                return None;
            }
            let position = match atoms.iter().position(|atom| atom == index) {
                Some(position) => position,
                None => {
                    atoms.push(*index);
                    atoms.len() - 1
                }
            };
            Some(TriPredicate::Atom(position))
        }
        ExecExpr::Not(inner) => Some(TriPredicate::Not(Box::new(build_tri_predicate(
            inner, schema, atoms,
        )?))),
        ExecExpr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            let left = build_tri_predicate(left, schema, atoms)?;
            let right = build_tri_predicate(right, schema, atoms)?;
            Some(TriPredicate::And(vec![left, right]))
        }
        ExecExpr::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } => {
            let left = build_tri_predicate(left, schema, atoms)?;
            let right = build_tri_predicate(right, schema, atoms)?;
            Some(TriPredicate::Or(vec![left, right]))
        }
        _ => None,
    }
}

/// Vectorized `WHERE` for pure three-valued predicates over annotation columns.
///
/// Returns the surviving row indices, or `None` when the predicate needs the
/// row-at-a-time path.
/// Row indices the row-at-a-time path keeps (the batch oracle).
fn scalar_filter_indices(
    filter: &Option<ExecExpr>,
    schema: &Schema,
    rows: &[Row],
) -> Result<Vec<usize>, SqlError> {
    rows.iter()
        .enumerate()
        .filter_map(
            |(index, row)| match passes_filter(filter.as_ref(), schema, row) {
                Ok(true) => Some(Ok(index)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

fn batch_filter_indices(filter: &ExecExpr, schema: &Schema, rows: &[Row]) -> Option<Vec<usize>> {
    let mut atoms = Vec::new();
    let predicate = build_tri_predicate(filter, schema, &mut atoms)?;
    if atoms.is_empty() {
        return None;
    }
    let columns: Vec<TritVec> = atoms
        .iter()
        .map(|index| K3Column::from_rows(rows, *index).map(|column| column.packed().clone()))
        .collect::<Option<Vec<_>>>()?;
    let truth = eval_predicate(&predicate, &columns).ok()?;
    Some(filter_indices(&truth))
}

fn execute_select(
    table: &Table,
    plan: &BoundSelect,
    deadline: Option<Instant>,
) -> Result<QueryResult, SqlError> {
    execute_select_inner(table, plan, None, deadline)
}

fn execute_select_inner(
    table: &Table,
    plan: &BoundSelect,
    mut stats: Option<&mut ExecutionStats>,
    deadline: Option<Instant>,
) -> Result<QueryResult, SqlError> {
    let filter_start = stats.is_some().then(Instant::now);
    let batch_indices = plan
        .filter
        .as_ref()
        .and_then(|filter| batch_filter_indices(filter, &table.schema, &table.rows));
    let filtered: Vec<Row> = match &batch_indices {
        Some(indices) => indices
            .iter()
            .map(|index| table.rows[*index].clone())
            .collect(),
        None => {
            let mut filtered = Vec::new();
            for (index, row) in table.rows.iter().enumerate() {
                if index % 1024 == 0 {
                    if let Some(deadline) = deadline {
                        if Instant::now() >= deadline {
                            return Err(SqlError::Unsupported(
                                "query cancelled: deadline exceeded".to_string(),
                            ));
                        }
                    }
                }
                match passes_filter(plan.filter.as_ref(), &table.schema, row) {
                    Ok(true) => filtered.push(row.clone()),
                    Ok(false) => {}
                    Err(error) => return Err(error),
                }
            }
            filtered
        }
    };
    // The batch path is opt-in; in debug builds it is checked against the
    // row-at-a-time implementation on every query that takes it.
    #[cfg(debug_assertions)]
    if let Some(indices) = &batch_indices {
        let scalar = table
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                match passes_filter(plan.filter.as_ref(), &table.schema, row) {
                    Ok(true) => Some(Ok(index)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        debug_assert_eq!(indices, &scalar, "batch WHERE diverged from the row path");
    }
    if let (Some(stats), Some(start)) = (stats.as_deref_mut(), filter_start) {
        stats.filter_rows = filtered.len();
        stats.filter_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    if plan.is_grouped() {
        return execute_grouped_inner(table, plan, &filtered, stats);
    }

    let mut rows = filtered;
    let sort_start = stats.is_some().then(Instant::now);
    let sort_rows = stats.is_some().then_some(rows.len());
    if !plan.order_by.is_empty() {
        let mut keyed = Vec::with_capacity(rows.len());
        for row in rows {
            let keys = {
                let group = Group { rows: vec![&row] };
                plan.order_by
                    .iter()
                    .map(|order| eval_group_expr(&order.expr, &table.schema, &group))
                    .collect::<Result<Vec<_>, _>>()?
            };
            keyed.push((keys, row));
        }
        validate_sort_types(&keyed, plan.order_by.len())?;
        keyed.sort_by(|left, right| compare_keys(&left.0, &right.0, plan));
        rows = keyed.into_iter().map(|(_, row)| row).collect();
    }
    if let (Some(stats), Some(start), Some(rows)) = (stats.as_deref_mut(), sort_start, sort_rows) {
        stats.sort_rows = rows;
        stats.sort_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    let project_start = stats.is_some().then(Instant::now);
    let project_rows = rows.len();
    let columns = plan
        .items
        .iter()
        .map(|item| item.name().to_string())
        .collect::<Vec<_>>();
    let mut output = Vec::with_capacity(rows.len());
    for row in rows {
        let group = Group { rows: vec![&row] };
        let mut projected = Vec::with_capacity(plan.items.len());
        for item in &plan.items {
            projected.push(eval_group_expr(&item.expr, &table.schema, &group)?);
        }
        output.push(projected);
    }
    if let (Some(stats), Some(start)) = (stats.as_deref_mut(), project_start) {
        stats.project_rows = project_rows;
        stats.project_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
    if let Some(limit) = plan.limit {
        output.truncate(limit);
    }
    if let Some(stats) = stats {
        stats.output_rows = output.len();
    }
    Ok(QueryResult::Rows {
        columns,
        rows: output,
    })
}

fn execute_grouped_inner(
    table: &Table,
    plan: &BoundSelect,
    rows: &[Row],
    mut stats: Option<&mut ExecutionStats>,
) -> Result<QueryResult, SqlError> {
    let columns = plan
        .items
        .iter()
        .map(|item| item.name().to_string())
        .collect::<Vec<_>>();

    let aggregate_start = stats.is_some().then(Instant::now);
    let groups = if plan.group_by.is_empty() {
        vec![Group {
            rows: rows.iter().collect(),
        }]
    } else {
        let mut grouped: BTreeMap<Vec<GroupKeyPart>, Vec<&Row>> = BTreeMap::new();
        for row in rows {
            let mut key = Vec::with_capacity(plan.group_by.len());
            for expr in &plan.group_by {
                let value = eval(expr, &table.schema, row)?;
                key.push(GroupKeyPart::from(&value));
            }
            grouped.entry(key).or_default().push(row);
        }
        grouped.into_values().map(|rows| Group { rows }).collect()
    };
    if let (Some(stats), Some(start)) = (stats.as_deref_mut(), aggregate_start) {
        stats.aggregate_groups = groups.len();
        stats.aggregate_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    let having_start = stats.is_some().then(Instant::now);
    let mut kept = Vec::new();
    for group in groups {
        if let Some(having) = &plan.having {
            let value = eval_group_expr(having, &table.schema, &group)?;
            if truth_of(&value)? != TruthValue::True {
                continue;
            }
        }
        kept.push(group);
    }
    if let (Some(stats), Some(start)) = (stats.as_deref_mut(), having_start) {
        stats.having_groups = kept.len();
        stats.having_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    let project_start = stats.is_some().then(Instant::now);
    let project_rows = kept.len();
    let mut keyed = Vec::with_capacity(kept.len());
    for group in kept {
        let mut projected = Vec::with_capacity(plan.items.len());
        for item in &plan.items {
            projected.push(eval_group_expr(&item.expr, &table.schema, &group)?);
        }
        let mut keys = Vec::with_capacity(plan.order_by.len());
        for order in &plan.order_by {
            keys.push(eval_group_expr(&order.expr, &table.schema, &group)?);
        }
        keyed.push((keys, projected));
    }
    if let (Some(stats), Some(start)) = (stats.as_deref_mut(), project_start) {
        stats.project_rows = project_rows;
        stats.project_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    let sort_start = stats.is_some().then(Instant::now);
    let sort_rows = keyed.len();
    if !plan.order_by.is_empty() {
        validate_sort_types(&keyed, plan.order_by.len())?;
        keyed.sort_by(|left, right| compare_keys(&left.0, &right.0, plan));
    }
    if let (Some(stats), Some(start)) = (stats.as_deref_mut(), sort_start) {
        stats.sort_rows = sort_rows;
        stats.sort_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    let mut output: Vec<Vec<Value>> = keyed.into_iter().map(|(_, row)| row).collect();
    if let Some(limit) = plan.limit {
        output.truncate(limit);
    }
    if let Some(stats) = stats {
        stats.output_rows = output.len();
    }
    Ok(QueryResult::Rows {
        columns,
        rows: output,
    })
}

/// A value used as part of a grouping key. `NULL` compares and groups equal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeyPart {
    Null,
    Int(i64),
    Text(String),
    Bool(bool),
    Logic(tritdb_exec::ann::B4),
    Lukasiewicz(u16),
    Nat(u64),
}

impl From<&Value> for GroupKeyPart {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => GroupKeyPart::Null,
            Value::Int(value) => GroupKeyPart::Int(*value),
            Value::Text(value) => GroupKeyPart::Text(value.clone()),
            Value::Bool(value) => GroupKeyPart::Bool(*value),
            Value::Logic(value) => GroupKeyPart::Logic(*value),
            Value::Lukasiewicz(value) => GroupKeyPart::Lukasiewicz(*value),
            Value::Nat(value) => GroupKeyPart::Nat(*value),
        }
    }
}

/// A group of source rows. A global aggregate over an empty table has one
/// group with no rows.
struct Group<'a> {
    rows: Vec<&'a Row>,
}

impl Group<'_> {
    fn representative(&self) -> Option<&Row> {
        self.rows.first().copied()
    }
}

fn eval_group_expr(
    expr: &GroupExpr,
    schema: &Schema,
    group: &Group<'_>,
) -> Result<Value, SqlError> {
    match expr {
        GroupExpr::Scalar(expr) => match group.representative() {
            Some(row) => Ok(eval(expr, schema, row)?),
            None => Ok(eval(expr, &Schema::default(), &[])?),
        },
        GroupExpr::Aggregate { kind, input } => {
            eval_aggregate(*kind, input.as_ref(), schema, &group.rows)
        }
        GroupExpr::IsNull { expr, negated } => {
            let value = eval_group_expr(expr, schema, group)?;
            let is_null = value.is_null();
            Ok(Value::Bool(if *negated { !is_null } else { is_null }))
        }
        GroupExpr::IsTruth {
            expr,
            expected,
            negated,
        } => {
            let value = eval_group_expr(expr, schema, group)?;
            let truth = truth_of(&value)?;
            let matched = truth == *expected;
            Ok(Value::Bool(if *negated { !matched } else { matched }))
        }
        GroupExpr::Truth { expr, threshold } => {
            let value = eval_group_expr(expr, schema, group)?;
            let threshold = match threshold {
                Some(threshold) => Some(eval_group_expr(threshold, schema, group)?),
                None => None,
            };
            let truth = truth_projection(&value, threshold.as_ref())?;
            Ok(Value::Text(truth_text(truth).to_string()))
        }
        GroupExpr::Certainty { expr, threshold } => {
            let value = eval_group_expr(expr, schema, group)?;
            let threshold = match threshold {
                Some(threshold) => Some(eval_group_expr(threshold, schema, group)?),
                None => None,
            };
            let verdict = certainty_of_value(&value, threshold.as_ref())?;
            Ok(Value::Text(verdict.as_str().to_string()))
        }
        GroupExpr::Not(expr) => {
            let value = eval_group_expr(expr, schema, group)?;
            Ok(truth_of(&value)?.not().to_value())
        }
        GroupExpr::InList {
            expr,
            values,
            negated,
        } => {
            let probe = eval_group_expr(expr, schema, group)?;
            let list = values
                .iter()
                .map(|value| eval_group_expr(value, schema, group))
                .collect::<Result<Vec<_>, _>>()?;
            let bound = ExecExpr::InList {
                expr: Box::new(ExecExpr::Literal(probe)),
                values: list.into_iter().map(ExecExpr::Literal).collect(),
                negated: *negated,
            };
            Ok(eval(&bound, &Schema::default(), &[])?)
        }
        GroupExpr::Between {
            expr,
            low,
            high,
            negated,
        } => {
            let probe = eval_group_expr(expr, schema, group)?;
            let low = eval_group_expr(low, schema, group)?;
            let high = eval_group_expr(high, schema, group)?;
            let bound = ExecExpr::Between {
                expr: Box::new(ExecExpr::Literal(probe)),
                low: Box::new(ExecExpr::Literal(low)),
                high: Box::new(ExecExpr::Literal(high)),
                negated: *negated,
            };
            Ok(eval(&bound, &Schema::default(), &[])?)
        }
        GroupExpr::Like {
            expr,
            pattern,
            negated,
        } => {
            let probe = eval_group_expr(expr, schema, group)?;
            let pattern = eval_group_expr(pattern, schema, group)?;
            let bound = ExecExpr::Like {
                expr: Box::new(ExecExpr::Literal(probe)),
                pattern: Box::new(ExecExpr::Literal(pattern)),
                negated: *negated,
            };
            Ok(eval(&bound, &Schema::default(), &[])?)
        }
        GroupExpr::Case {
            operand,
            branches,
            else_expr,
        } => {
            if let Some(operand) = operand {
                let probe = eval_group_expr(operand, schema, group)?;
                for (when, then) in branches {
                    let candidate = eval_group_expr(when, schema, group)?;
                    if compare_values(&probe, &candidate)? == Some(Ordering::Equal) {
                        return eval_group_expr(then, schema, group);
                    }
                }
            } else {
                for (when, then) in branches {
                    let value = eval_group_expr(when, schema, group)?;
                    if truth_of(&value)? == TruthValue::True {
                        return eval_group_expr(then, schema, group);
                    }
                }
            }
            match else_expr {
                Some(expr) => eval_group_expr(expr, schema, group),
                None => Ok(Value::Null),
            }
        }
        GroupExpr::Binary { op, left, right } => {
            let left = eval_group_expr(left, schema, group)?;
            let right = eval_group_expr(right, schema, group)?;
            let expr = ExecExpr::Binary {
                op: *op,
                left: Box::new(ExecExpr::Literal(left)),
                right: Box::new(ExecExpr::Literal(right)),
            };
            Ok(eval(&expr, &Schema::default(), &[])?)
        }
    }
}

fn validate_sort_types(keyed: &[(Vec<Value>, Row)], count: usize) -> Result<(), SqlError> {
    for index in 0..count {
        let mut data_type: Option<&'static str> = None;
        for (keys, _) in keyed {
            let value = &keys[index];
            if value.is_null() {
                continue;
            }
            match data_type {
                None => data_type = Some(value.type_name()),
                Some(expected) if expected == value.type_name() => {}
                Some(expected) => {
                    return Err(SqlError::Exec(ExecError::TypeMismatch {
                        expected,
                        actual: value.type_name(),
                    }));
                }
            }
        }
    }
    Ok(())
}

fn compare_keys(left: &[Value], right: &[Value], plan: &BoundSelect) -> Ordering {
    for (index, order) in plan.order_by.iter().enumerate() {
        let ordering = match (&left[index], &right[index]) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            (left, right) => match compare_values(left, right) {
                Ok(Some(ordering)) => ordering,
                _ => Ordering::Equal,
            },
        };
        if ordering != Ordering::Equal {
            return if order.ascending {
                ordering
            } else {
                ordering.reverse()
            };
        }
    }
    Ordering::Equal
}

fn eval_aggregate(
    kind: AggregateKind,
    input: Option<&ExecExpr>,
    schema: &Schema,
    rows: &[&Row],
) -> Result<Value, SqlError> {
    match kind {
        AggregateKind::Count => match input {
            None => Ok(Value::Int(
                i64::try_from(rows.len()).map_err(|_| ExecError::IntegerOverflow)?,
            )),
            Some(expr) => {
                let mut count = 0i64;
                for row in rows {
                    if !eval(expr, schema, row)?.is_null() {
                        count = count.checked_add(1).ok_or(ExecError::IntegerOverflow)?;
                    }
                }
                Ok(Value::Int(count))
            }
        },
        AggregateKind::Sum | AggregateKind::Avg => {
            let Some(expr) = input else {
                return Err(SqlError::Unsupported(format!(
                    "{} requires an argument",
                    kind.name()
                )));
            };
            let average = kind == AggregateKind::Avg;
            let mut mode: Option<&'static str> = None;
            let mut sum_int = 0i64;
            let mut sum_luk = 0u64;
            let mut sum_nat = 0u128;
            let mut count = 0u64;
            for row in rows {
                match eval(expr, schema, row)? {
                    Value::Null => {}
                    Value::Int(value) => {
                        if mode.is_some_and(|mode| mode != "INT") {
                            return Err(SqlError::Exec(ExecError::TypeMismatch {
                                expected: mode.unwrap_or("INT"),
                                actual: "INT",
                            }));
                        }
                        mode = Some("INT");
                        sum_int = sum_int
                            .checked_add(value)
                            .ok_or(ExecError::IntegerOverflow)?;
                        count += 1;
                    }
                    Value::Lukasiewicz(value) => {
                        if mode.is_some_and(|mode| mode != "LUKASIEWICZ") {
                            return Err(SqlError::Exec(ExecError::TypeMismatch {
                                expected: mode.unwrap_or("LUKASIEWICZ"),
                                actual: "LUKASIEWICZ",
                            }));
                        }
                        mode = Some("LUKASIEWICZ");
                        sum_luk = sum_luk.saturating_add(u64::from(value));
                        count += 1;
                    }
                    Value::Nat(value) => {
                        if mode.is_some_and(|mode| mode != "NAT") {
                            return Err(SqlError::Exec(ExecError::TypeMismatch {
                                expected: mode.unwrap_or("NAT"),
                                actual: "NAT",
                            }));
                        }
                        mode = Some("NAT");
                        sum_nat = sum_nat.saturating_add(u128::from(value));
                        count += 1;
                    }
                    other => {
                        return Err(SqlError::Exec(ExecError::TypeMismatch {
                            expected: "INT/LUKASIEWICZ/NAT",
                            actual: other.type_name(),
                        }));
                    }
                }
            }
            if count == 0 {
                return Ok(Value::Null);
            }
            match mode.expect("non-empty aggregate") {
                "INT" => Ok(Value::Int(if average {
                    sum_int / i64::try_from(count).map_err(|_| ExecError::IntegerOverflow)?
                } else {
                    sum_int
                })),
                "LUKASIEWICZ" => {
                    let raw = if average { sum_luk / count } else { sum_luk };
                    Ok(Value::Lukasiewicz(raw.min(1000) as u16))
                }
                "NAT" => {
                    let raw = if average {
                        sum_nat / u128::from(count)
                    } else {
                        sum_nat
                    };
                    Ok(Value::Nat(raw.min(u128::from(u64::MAX)) as u64))
                }
                _ => unreachable!("mode is one of INT/LUKASIEWICZ/NAT"),
            }
        }
        AggregateKind::ProbSum => {
            let Some(expr) = input else {
                return Err(SqlError::Unsupported(
                    "PROB_SUM requires an argument".to_string(),
                ));
            };
            let mut product = 1.0f64;
            let mut count = 0u64;
            for row in rows {
                match eval(expr, schema, row)? {
                    Value::Null => {}
                    Value::Lukasiewicz(value) => {
                        let probability = f64::from(value) / 1000.0;
                        product *= 1.0 - probability;
                        count += 1;
                    }
                    other => {
                        return Err(SqlError::Exec(ExecError::TypeMismatch {
                            expected: "LUKASIEWICZ",
                            actual: other.type_name(),
                        }));
                    }
                }
            }
            if count == 0 {
                return Ok(Value::Null);
            }
            let probability = (1.0 - product).clamp(0.0, 1.0);
            Ok(Value::Lukasiewicz((probability * 1000.0).round() as u16))
        }
        AggregateKind::Min | AggregateKind::Max => {
            let Some(expr) = input else {
                return Err(SqlError::Unsupported(format!(
                    "{} requires an argument",
                    kind.name()
                )));
            };
            let mut best: Option<Value> = None;
            for row in rows {
                let value = eval(expr, schema, row)?;
                if value.is_null() {
                    continue;
                }
                match &best {
                    None => best = Some(value),
                    Some(current) => {
                        let ordering = compare_values(&value, current)?.ok_or_else(|| {
                            SqlError::Exec(ExecError::TypeMismatch {
                                expected: current.type_name(),
                                actual: value.type_name(),
                            })
                        })?;
                        let replace = match kind {
                            AggregateKind::Min => ordering == Ordering::Less,
                            AggregateKind::Max => ordering == Ordering::Greater,
                            _ => false,
                        };
                        if replace {
                            best = Some(value);
                        }
                    }
                }
            }
            Ok(best.unwrap_or(Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script;

    #[test]
    fn having_correlated_subquery_group_cap() {
        let dir = tritdb_testkit::TempDir::new("engine-having-cap");
        let db = dir.join("having.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script("CREATE TABLE g (x INT NOT NULL); CREATE TABLE s (x INT NOT NULL);")
            .expect("create");
        let values: String = (0..MAX_CORRELATED_ROWS + 1)
            .map(|value| format!("({value}),"))
            .collect::<String>();
        engine
            .execute_script(&format!(
                "INSERT INTO g VALUES {};",
                values.trim_end_matches(',')
            ))
            .expect("seed g");
        engine
            .execute_script("INSERT INTO s VALUES (0);")
            .expect("seed s");

        let error = match engine.execute_script(
            "SELECT x, COUNT(*) FROM g GROUP BY x \
             HAVING COUNT(*) = (SELECT COUNT(*) FROM s WHERE s.x = g.x);",
        ) {
            Ok(_) => panic!("a correlated HAVING subquery over the group cap must be rejected"),
            Err(error) => error,
        };
        assert!(
            matches!(error, SqlError::Unsupported(ref message) if message.contains("correlated subquery")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn correlated_subquery_row_cap() {
        let dir = tritdb_testkit::TempDir::new("engine-corr-cap");
        let db = dir.join("corr.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script("CREATE TABLE t (x INT NOT NULL); CREATE TABLE s (x INT NOT NULL);")
            .expect("create");
        let values: String = (0..MAX_CORRELATED_ROWS + 1)
            .map(|value| format!("({value}),"))
            .collect::<String>();
        engine
            .execute_script(&format!(
                "INSERT INTO t VALUES {};",
                values.trim_end_matches(',')
            ))
            .expect("seed t");
        engine
            .execute_script("INSERT INTO s VALUES (0);")
            .expect("seed s");

        let error = match engine
            .execute_script("SELECT COUNT(*) FROM t WHERE x = (SELECT x FROM s WHERE s.x = t.x);")
        {
            Ok(_) => panic!("a correlated subquery over the row cap must be rejected"),
            Err(error) => error,
        };
        assert!(
            matches!(error, SqlError::Unsupported(ref message) if message.contains("correlated subquery")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn constant_in_list_keeps_null_semantics() {
        let dir = tritdb_testkit::TempDir::new("engine-in-set");
        let db = dir.join("in.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script("CREATE TABLE t (x INT NOT NULL);")
            .expect("create");
        let values: String = (0..20)
            .map(|value| format!("({value}),"))
            .collect::<String>();
        engine
            .execute_script(&format!(
                "INSERT INTO t VALUES {};",
                values.trim_end_matches(',')
            ))
            .expect("seed");

        // A large constant list takes the pre-indexed path; it must still
        // match, and NULL in the list must keep `NOT IN` three-valued.
        let mut list: Vec<String> = (100..10_020).map(|value| value.to_string()).collect();
        list.push("7".to_string());
        list.push("NULL".to_string());
        let sql = format!("SELECT x FROM t WHERE x IN ({});", list.join(","));
        let rows = match &engine.execute_script(&sql).expect("in list")[0] {
            QueryResult::Rows { rows, .. } => rows.clone(),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(rows, vec![vec![Value::Int(7)]]);

        let sql = format!("SELECT x FROM t WHERE x NOT IN ({});", list.join(","));
        let rows = match &engine.execute_script(&sql).expect("not in list")[0] {
            QueryResult::Rows { rows, .. } => rows.clone(),
            other => panic!("unexpected {other:?}"),
        };
        assert!(
            rows.is_empty(),
            "NOT IN with a NULL candidate must not return rows: {rows:?}"
        );
    }

    #[test]
    fn deadline_cancels_a_scan() {
        let dir = tritdb_testkit::TempDir::new("engine-deadline");
        let db = dir.join("deadline.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script("CREATE TABLE t (x INT NOT NULL); INSERT INTO t VALUES (1);")
            .expect("seed");

        engine.set_deadline(Some(Instant::now() - std::time::Duration::from_millis(1)));
        let error = match engine.execute_script("SELECT * FROM t;") {
            Ok(_) => panic!("an expired deadline must cancel the scan"),
            Err(error) => error,
        };
        assert!(
            matches!(error, SqlError::Unsupported(ref message) if message.contains("cancelled")),
            "unexpected error: {error}"
        );

        engine.set_deadline(None);
        assert!(engine.execute_script("SELECT * FROM t;").is_ok());
    }

    #[test]
    fn multi_statement_scripts_stay_supported() {
        // `parse_script` returning several statements is by design (script
        // files); the injection-prone entry points have their own checks.
        let dir = tritdb_testkit::TempDir::new("engine-multi");
        let db = dir.join("multi.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        let results = engine
            .execute_script(
                "CREATE TABLE t (x INT NOT NULL); \
                 INSERT INTO t VALUES (1), (2); \
                 SELECT COUNT(*) FROM t;",
            )
            .expect("multi statement script");
        assert_eq!(results.len(), 3);
        let rows = match &results[2] {
            QueryResult::Rows { rows, .. } => rows,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(rows[0][0], Value::Int(2));
    }

    #[test]
    fn describe_select_binds_without_executing() {
        let dir = tritdb_testkit::TempDir::new("engine-describe");
        let db = dir.join("describe.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script("CREATE TABLE t (x INT NOT NULL, name TEXT);")
            .expect("create");

        // Binding alone produces the metadata; execution would aggregate the
        // (empty) table, which this fast path must skip.
        let columns = engine
            .describe_select("SELECT x, COUNT(*) FROM t GROUP BY x;")
            .expect("describe");
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0], ("x".to_string(), DataType::Int));
        assert_eq!(columns[1].1, DataType::Int);

        // Shapes the fast path does not cover return None (probe fallback).
        assert!(engine.describe_select("SELECT x FROM missing;").is_none());
        assert!(engine.describe_select("SELECT * FROM t, t AS u;").is_none());
    }

    #[test]
    fn explain_high_cardinality_join_only_estimates() {
        let dir = tritdb_testkit::TempDir::new("engine-explain-card");
        let db = dir.join("explain.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        for table in ["a", "b", "c", "d"] {
            engine
                .execute_script(&format!("CREATE TABLE {table} (x INT NOT NULL);"))
                .expect("create");
            let values: String = (0..1000)
                .map(|value| format!("({value}),"))
                .collect::<String>();
            engine
                .execute_script(&format!(
                    "INSERT INTO {table} VALUES {};",
                    values.trim_end_matches(',')
                ))
                .expect("seed");
            engine
                .execute_script(&format!("ANALYZE {table};"))
                .expect("analyze");
        }

        // Four 1000-row tables multiply out to 10^12 estimated rows. EXPLAIN
        // must only plan the join (and saturate the estimate), never run it.
        let results = engine
            .execute_script("EXPLAIN SELECT * FROM a, b, c, d WHERE a.x = b.x;")
            .expect("EXPLAIN must plan the join, not execute it");
        assert!(!results.is_empty());
    }

    #[test]
    fn cross_join_stops_at_the_row_cap() {
        let dir = tritdb_testkit::TempDir::new("engine-join-cap");
        let db = dir.join("join.db");
        let mut engine = SqlEngine::open(&db).expect("open");
        engine
            .execute_script("CREATE TABLE a (x INT NOT NULL); CREATE TABLE b (y INT NOT NULL);")
            .expect("create");
        let values: String = (0..2000)
            .map(|value| format!("({value}),"))
            .collect::<String>();
        let values = values.trim_end_matches(',');
        engine
            .execute_script(&format!(
                "INSERT INTO a VALUES {values}; INSERT INTO b VALUES {values};"
            ))
            .expect("seed");

        let error = match engine.execute_script("SELECT * FROM a, b;") {
            Ok(_) => panic!("a 2000 x 2000 cross join must hit the row cap"),
            Err(error) => error,
        };
        assert!(
            matches!(error, SqlError::Unsupported(ref message) if message.contains("join produces")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn folds_projection_constants_and_preserves_column_items() {
        let statements =
            parse_script("SELECT 1 + 1 AS two, id, 1 / 0 AS bad FROM t;").expect("parse select");
        let Statement::Select(select) = &statements[0] else {
            panic!("expected SELECT");
        };
        let folded = fold_select(select);
        match &folded.items[0] {
            SelectItem::Expr { expr, alias } => {
                assert_eq!(expr, &Expr::Literal(Value::Int(2)));
                assert_eq!(alias.as_deref(), Some("two"));
            }
            other => panic!("unexpected first item: {other:?}"),
        }
        match &folded.items[1] {
            SelectItem::Expr { expr, alias } => {
                assert_eq!(
                    expr,
                    &Expr::Column {
                        table: None,
                        name: "id".to_string(),
                    }
                );
                assert_eq!(*alias, None);
            }
            other => panic!("unexpected second item: {other:?}"),
        }
        match &folded.items[2] {
            SelectItem::Expr { expr, .. } => {
                assert!(matches!(
                    expr,
                    Expr::Binary {
                        op: BinaryOp::Div,
                        ..
                    }
                ));
            }
            other => panic!("unexpected third item: {other:?}"),
        }
    }
}
