use tritdb_exec::{BinaryOp, DataType, TruthValue, Value};

/// Aggregate function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateKind {
    /// `COUNT(*)` or `COUNT(expr)`.
    Count,
    /// `SUM(expr)`.
    Sum,
    /// `AVG(expr)`.
    Avg,
    /// `PROB_SUM(expr)`: `1 - product(1 - p)` over Łukasiewicz values.
    ProbSum,
    /// `MIN(expr)`.
    Min,
    /// `MAX(expr)`.
    Max,
}

impl AggregateKind {
    /// SQL name.
    pub const fn name(self) -> &'static str {
        match self {
            AggregateKind::Count => "COUNT",
            AggregateKind::Sum => "SUM",
            AggregateKind::Avg => "AVG",
            AggregateKind::ProbSum => "PROB_SUM",
            AggregateKind::Min => "MIN",
            AggregateKind::Max => "MAX",
        }
    }
}

/// Window function kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowFunction {
    /// `ROW_NUMBER()`.
    RowNumber,
    /// `RANK()`.
    Rank,
    /// `DENSE_RANK()`.
    DenseRank,
    /// `SUM/AVG/COUNT/MIN/MAX(...)` aggregate window.
    Aggregate(AggregateKind),
    /// `LAG(expr)`.
    Lag,
    /// `LEAD(expr)`.
    Lead,
}

impl WindowFunction {
    /// SQL name.
    pub const fn name(self) -> &'static str {
        match self {
            WindowFunction::RowNumber => "ROW_NUMBER",
            WindowFunction::Rank => "RANK",
            WindowFunction::DenseRank => "DENSE_RANK",
            WindowFunction::Aggregate(kind) => kind.name(),
            WindowFunction::Lag => "LAG",
            WindowFunction::Lead => "LEAD",
        }
    }
}

/// Parsed, unbound expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// Literal.
    Literal(Value),
    /// Column reference, optionally qualified by a table name or alias.
    Column {
        /// Qualifier.
        table: Option<String>,
        /// Column name.
        name: String,
    },
    /// `expr IS [NOT] NULL`.
    IsNull {
        /// Input.
        expr: Box<Expr>,
        /// Negated.
        negated: bool,
    },
    /// `expr IS [NOT] TRUE|FALSE|UNKNOWN`.
    IsTruth {
        /// Input.
        expr: Box<Expr>,
        /// Expected truth value.
        expected: TruthValue,
        /// Negated.
        negated: bool,
    },
    /// `NOT expr`.
    Not(Box<Expr>),
    /// Parenthesized expression list, used by multi-column `IN`.
    Tuple(Vec<Expr>),
    /// `expr [NOT] IN (value, ...)`.
    InList {
        /// Probe expression.
        expr: Box<Expr>,
        /// Candidate expressions.
        values: Vec<Expr>,
        /// `true` for `NOT IN`.
        negated: bool,
    },
    /// `expr [NOT] IN (SELECT ...)`.
    InSubquery {
        /// Probe expression.
        expr: Box<Expr>,
        /// Subquery; must return exactly one column.
        query: Box<Query>,
        /// `true` for `NOT IN`.
        negated: bool,
    },
    /// `(SELECT ...)` used as a scalar value.
    ScalarSubquery {
        /// Subquery; must return zero or one row and one column.
        query: Box<Query>,
    },
    /// `(expr, ...) [NOT] IN (SELECT ...)`.
    InSubqueryTuple {
        /// Left-hand expressions.
        exprs: Vec<Expr>,
        /// Subquery; must return as many columns as `exprs`.
        query: Box<Query>,
        /// `true` for `NOT IN`.
        negated: bool,
    },
    /// `[NOT] EXISTS (SELECT ...)`.
    Exists {
        /// Subquery.
        query: Box<Query>,
    },
    /// `expr op ANY|ALL (SELECT ...)`.
    Quantified {
        /// Left operand.
        expr: Box<Expr>,
        /// Comparison operator.
        op: BinaryOp,
        /// `ANY` or `ALL`.
        kind: Quantifier,
        /// Subquery; must return exactly one column.
        query: Box<Query>,
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
    /// `truth(expr [, threshold])`: projects an annotation to
    /// `TRUE` / `FALSE` / `UNKNOWN`.
    ///
    /// K3/Belnap map `Both`/`Neither` to `UNKNOWN`; Łukasiewicz compares the
    /// thousandths value against a threshold (default `0.5`); ℕ compares the
    /// count against a threshold (default `1`).
    Truth {
        /// Input annotation.
        expr: Box<Expr>,
        /// Optional `Int` threshold (thousandths for Ł, a count for ℕ).
        threshold: Option<Box<Expr>>,
    },
    /// `certainty(expr [, threshold])`: `CERTAIN` / `CONTINGENT` /
    /// `IMPOSSIBLE` verdict, the same vocabulary as `UNDER REPAIR`.
    Certainty {
        /// Input annotation.
        expr: Box<Expr>,
        /// Optional threshold (thousandths for Ł, a count for ℕ).
        threshold: Option<Box<Expr>>,
    },
    /// `CASE [operand] WHEN ... THEN ... [ELSE ...] END`.
    Case {
        /// Simple-case operand; `None` for a searched case.
        operand: Option<Box<Expr>>,
        /// `WHEN`/`THEN` pairs in source order.
        branches: Vec<(Expr, Expr)>,
        /// `ELSE` result.
        else_expr: Option<Box<Expr>>,
    },
    /// Binary operator.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left.
        left: Box<Expr>,
        /// Right.
        right: Box<Expr>,
    },
    /// Aggregate call. `None` means `COUNT(*)`.
    Aggregate {
        /// Function.
        kind: AggregateKind,
        /// Argument.
        input: Option<Box<Expr>>,
    },
    /// Window function call.
    Window {
        /// Function.
        function: WindowFunction,
        /// Aggregate argument; `None` for rank functions and `COUNT(*)`.
        input: Option<Box<Expr>>,
        /// `PARTITION BY` expressions.
        partition_by: Vec<Expr>,
        /// `ORDER BY` keys inside `OVER (...)`.
        order_by: Vec<OrderBy>,
    },
}

/// `ANY` / `ALL` quantifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantifier {
    /// `ANY`: true when at least one comparison is true.
    Any,
    /// `ALL`: true when every comparison is true.
    All,
}

/// A `CREATE TABLE` column definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnDef {
    /// Column name.
    pub name: String,
    /// Type.
    pub data_type: DataType,
    /// Reject `NULL`.
    pub not_null: bool,
    /// Reject duplicate non-`NULL` values.
    pub unique: bool,
    /// Column-level `CHECK (expr)` text.
    pub check: Option<String>,
}

/// Select-list item.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectItem {
    /// `*`.
    Wildcard,
    /// Expression with optional alias.
    Expr {
        /// Expression.
        expr: Expr,
        /// Alias.
        alias: Option<String>,
    },
}

/// `ORDER BY` item.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderBy {
    /// Sort expression.
    pub expr: Expr,
    /// Ascending when true.
    pub ascending: bool,
}

/// Join flavor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    /// Inner join: only matching row pairs survive.
    Inner,
    /// As-of join: each left row pairs with the last right row whose time is
    /// `<=` the left row's time (`ON left.ts >= right.ts`).
    Asof,
    /// Cross join: every left row pairs with every right row.
    Cross,
    /// Left outer join: unmatched left rows survive with `NULL` right columns.
    Left,
    /// Right outer join: unmatched right rows survive with `NULL` left columns.
    Right,
    /// Full outer join: unmatched rows from either side survive with `NULL`
    /// columns from the other side.
    Full,
}

/// Join condition.
#[derive(Clone, Debug, PartialEq)]
pub enum JoinCondition {
    /// No condition: `CROSS JOIN` and comma-separated sources.
    None,
    /// Explicit `ON` predicate.
    On(Expr),
    /// `USING (column, ...)`.
    Using(Vec<String>),
    /// `NATURAL JOIN`.
    Natural,
}

/// A join clause.
#[derive(Clone, Debug, PartialEq)]
pub struct Join {
    /// Join flavor.
    pub kind: JoinKind,
    /// Joined table.
    pub table: String,
    /// Optional alias.
    pub alias: Option<String>,
    /// Join condition.
    pub condition: JoinCondition,
}

/// Parsed `SELECT`.
#[derive(Clone, Debug, PartialEq)]
pub struct Select {
    /// Select list.
    pub items: Vec<SelectItem>,
    /// Source table.
    pub table: String,
    /// Source table alias.
    pub table_alias: Option<String>,
    /// Join clauses, applied left to right.
    pub joins: Vec<Join>,
    /// Optional filter.
    pub filter: Option<Expr>,
    /// Grouping columns.
    pub group_by: Vec<Expr>,
    /// Optional group filter.
    pub having: Option<Expr>,
    /// Sort keys.
    pub order_by: Vec<OrderBy>,
    /// Optional limit.
    pub limit: Option<usize>,
    /// `UNDER REPAIR ON (col, ...)`: keep rows that are certain in every
    /// minimal repair of the functional dependency `cols -> other columns`.
    pub under_repair: Option<Vec<String>>,
}

/// A query expression: a plain `SELECT` or a nested `WITH`.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    /// Plain select.
    Select(Box<Select>),
    /// Nested `WITH ...`.
    With(Box<With>),
}

/// One common table expression.
///
/// `seed UNION [ALL] step`; the recursive `step` may reference `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cte {
    /// CTE name, usable as a table in `seed`, `step`, and the outer query.
    pub name: String,
    /// Optional explicit column names.
    pub columns: Vec<String>,
    /// Anchor query.
    pub seed: Box<Query>,
    /// Recursive step (`UNION [ALL] step`); `None` for a plain CTE.
    pub step: Option<Box<Query>>,
    /// `UNION ALL` keeps duplicates; plain `UNION` removes them.
    pub union_all: bool,
}

/// `WITH [RECURSIVE] cte, ... <body>`.
#[derive(Clone, Debug, PartialEq)]
pub struct With {
    /// `RECURSIVE` was present.
    pub recursive: bool,
    /// Common table expressions, evaluated left to right.
    pub ctes: Vec<Cte>,
    /// Statement that consumes the CTEs: `SELECT`, `WITH`, `INSERT`, `UPDATE`,
    /// or `DELETE`.
    pub body: Box<Statement>,
}

/// One `SET` assignment in an `UPDATE`.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    /// Target column.
    pub column: String,
    /// Replacement expression.
    pub value: Expr,
}

/// Parsed statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// `CREATE TABLE`.
    CreateTable {
        /// Table name.
        name: String,
        /// Skip when the table exists.
        if_not_exists: bool,
        /// Columns.
        columns: Vec<ColumnDef>,
        /// Table-level `UNIQUE (a, b, ...)` groups.
        unique: Vec<Vec<String>>,
        /// Table-level `CHECK (expr)` texts.
        checks: Vec<String>,
    },
    /// `DROP TABLE`.
    DropTable {
        /// Table name.
        name: String,
        /// Skip when the table is absent.
        if_exists: bool,
    },
    /// `ALTER TABLE ... DROP [COLUMN] ...`.
    AlterTableDropColumn {
        /// Table name.
        name: String,
        /// Skip when the column is absent.
        if_exists: bool,
        /// Column to remove.
        column: String,
    },
    /// `ALTER TABLE ... RENAME TO ...` (table rename).
    RenameTable {
        /// Old table name.
        from: String,
        /// New table name.
        to: String,
    },
    /// `ALTER TABLE ... ALTER [COLUMN] c TYPE ...`.
    AlterColumnType {
        /// Table name.
        table: String,
        /// Column name.
        column: String,
        /// New type.
        data_type: DataType,
    },
    /// `ALTER TABLE ... RENAME [COLUMN] a TO b`.
    AlterTableRenameColumn {
        /// Table name.
        name: String,
        /// Old column name.
        from: String,
        /// New column name.
        to: String,
    },
    /// `ALTER TABLE ... ADD [COLUMN] ...`.
    AlterTableAddColumn {
        /// Table name.
        name: String,
        /// Skip when the column already exists.
        if_not_exists: bool,
        /// Appended column definition.
        column: ColumnDef,
    },
    /// `INSERT INTO ... VALUES` or `INSERT INTO ... SELECT`.
    Insert {
        /// Table name.
        table: String,
        /// Optional column list.
        columns: Option<Vec<String>>,
        /// Rows of expressions (`VALUES`).
        rows: Vec<Vec<Expr>>,
        /// Row source (`INSERT ... SELECT` / `WITH ...`).
        select: Option<Box<Query>>,
    },
    /// `SELECT count_models('<expr>'[, vars])`: exact model count over a
    /// boolean expression. This is rule-level, not `COUNT(*)`.
    CountModels {
        /// Boolean expression over `a..z` / `vN` / `N`.
        expr: String,
        /// Declared variable count; `0` infers the maximum index + 1.
        vars: usize,
    },
    /// `SELECT`.
    Select(Select),
    /// `WITH [RECURSIVE] ... SELECT`.
    With(Box<With>),
    /// `EXPLAIN SELECT`.
    Explain(Select),
    /// `EXPLAIN SEXPR SELECT`: read-only parenthesized plan output.
    ExplainSexpr(Select),
    /// `EXPLAIN ANALYZE SELECT`.
    ExplainAnalyze(Select),
    /// `EXPLAIN ANALYZE SEXPR SELECT`: parenthesized plan plus execution totals.
    ExplainAnalyzeSexpr(Select),
    /// `ANALYZE [table]`: recompute exact table statistics.
    Analyze {
        /// Optional table name; `None` analyzes every table.
        table: Option<String>,
    },
    /// `DELETE FROM`.
    Delete {
        /// Table name.
        table: String,
        /// Optional filter.
        filter: Option<Expr>,
    },
    /// `UPDATE ... SET ...`.
    Update {
        /// Table name.
        table: String,
        /// Assignments in order.
        assignments: Vec<Assignment>,
        /// Optional filter.
        filter: Option<Expr>,
    },
}
