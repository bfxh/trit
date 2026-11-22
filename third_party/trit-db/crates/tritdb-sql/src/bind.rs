use std::collections::{BTreeMap, BTreeSet};

use tritdb_exec::{eval, BinaryOp, Expr as ExecExpr, Schema, Table, TruthValue, Value};

use crate::ast::{AggregateKind, Expr, Select, SelectItem};
use crate::error::{closest_names, NameKind, SqlError};

/// Builds a hash set for an all-constant `IN` list, or `None` when the list
/// holds non-literals, mixed types, or is small enough that the row-wise scan
/// is cheaper than building a set.
fn constant_set(values: &[ExecExpr]) -> Option<(std::collections::HashSet<Value>, &'static str)> {
    if values.len() <= 8 {
        return None;
    }
    let mut set = std::collections::HashSet::with_capacity(values.len());
    let mut type_name: Option<&'static str> = None;
    for value in values {
        let ExecExpr::Literal(value) = value else {
            return None;
        };
        if value.is_null() {
            set.insert(Value::Null);
            continue;
        }
        let name = value.type_name();
        match type_name {
            None => type_name = Some(name),
            Some(expected) if expected == name => {}
            Some(_) => return None,
        }
        set.insert(value.clone());
    }
    type_name.map(|type_name| (set, type_name))
}

/// A bound expression that may contain aggregate calls.
#[derive(Clone, Debug)]
pub(crate) enum GroupExpr {
    /// Scalar expression evaluated against a source row.
    Scalar(ExecExpr),
    /// Aggregate call.
    Aggregate {
        /// Function.
        kind: AggregateKind,
        /// Argument; `None` for `COUNT(*)`.
        input: Option<ExecExpr>,
    },
    /// `expr IS [NOT] NULL`.
    IsNull {
        /// Input.
        expr: Box<GroupExpr>,
        /// Negated.
        negated: bool,
    },
    /// `truth(expr [, threshold])`.
    Truth {
        /// Input.
        expr: Box<GroupExpr>,
        /// Optional threshold.
        threshold: Option<Box<GroupExpr>>,
    },
    /// `certainty(expr [, threshold])`.
    Certainty {
        /// Input.
        expr: Box<GroupExpr>,
        /// Optional threshold.
        threshold: Option<Box<GroupExpr>>,
    },
    /// `expr IS [NOT] TRUE|FALSE|UNKNOWN`.
    IsTruth {
        /// Input.
        expr: Box<GroupExpr>,
        /// Expected truth value.
        expected: TruthValue,
        /// Negated.
        negated: bool,
    },
    /// `NOT expr`.
    Not(Box<GroupExpr>),
    /// `expr [NOT] IN (value, ...)`.
    InList {
        /// Probe.
        expr: Box<GroupExpr>,
        /// Candidates.
        values: Vec<GroupExpr>,
        /// `NOT IN`.
        negated: bool,
    },
    /// `expr [NOT] BETWEEN low AND high`.
    Between {
        /// Probe.
        expr: Box<GroupExpr>,
        /// Lower bound.
        low: Box<GroupExpr>,
        /// Upper bound.
        high: Box<GroupExpr>,
        /// `NOT BETWEEN`.
        negated: bool,
    },
    /// `expr [NOT] LIKE pattern`.
    Like {
        /// Probe.
        expr: Box<GroupExpr>,
        /// Pattern.
        pattern: Box<GroupExpr>,
        /// `NOT LIKE`.
        negated: bool,
    },
    /// `CASE [operand] WHEN ... THEN ... [ELSE ...] END`.
    Case {
        /// Simple-case operand.
        operand: Option<Box<GroupExpr>>,
        /// `WHEN`/`THEN` pairs.
        branches: Vec<(GroupExpr, GroupExpr)>,
        /// `ELSE` result.
        else_expr: Option<Box<GroupExpr>>,
    },
    /// Binary operator.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left.
        left: Box<GroupExpr>,
        /// Right.
        right: Box<GroupExpr>,
    },
}

impl GroupExpr {
    pub(crate) fn has_aggregate(&self) -> bool {
        match self {
            GroupExpr::Scalar(_) => false,
            GroupExpr::Aggregate { .. } => true,
            GroupExpr::IsNull { expr, .. } | GroupExpr::IsTruth { expr, .. } => {
                expr.has_aggregate()
            }
            GroupExpr::Truth { expr, threshold } => {
                expr.has_aggregate()
                    || threshold
                        .as_ref()
                        .is_some_and(|threshold| threshold.has_aggregate())
            }
            GroupExpr::Certainty { expr, threshold } => {
                expr.has_aggregate()
                    || threshold
                        .as_ref()
                        .is_some_and(|threshold| threshold.has_aggregate())
            }
            GroupExpr::Not(expr) => expr.has_aggregate(),
            GroupExpr::InList { expr, values, .. } => {
                expr.has_aggregate() || values.iter().any(GroupExpr::has_aggregate)
            }
            GroupExpr::Between {
                expr, low, high, ..
            } => expr.has_aggregate() || low.has_aggregate() || high.has_aggregate(),
            GroupExpr::Like { expr, pattern, .. } => {
                expr.has_aggregate() || pattern.has_aggregate()
            }
            GroupExpr::Case {
                operand,
                branches,
                else_expr,
            } => {
                operand.as_ref().is_some_and(|expr| expr.has_aggregate())
                    || branches
                        .iter()
                        .any(|(when, then)| when.has_aggregate() || then.has_aggregate())
                    || else_expr.as_ref().is_some_and(|expr| expr.has_aggregate())
            }
            GroupExpr::Binary { left, right, .. } => left.has_aggregate() || right.has_aggregate(),
        }
    }
}

/// A bound select item.
#[derive(Clone, Debug)]
pub(crate) struct BoundItem {
    /// Bound expression.
    pub(crate) expr: GroupExpr,
    /// Output name.
    pub(crate) name: String,
}

impl BoundItem {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// A bound sort key.
#[derive(Clone, Debug)]
pub(crate) struct BoundOrder {
    /// Expression evaluated against a row or a group.
    pub(crate) expr: GroupExpr,
    /// Ascending.
    pub(crate) ascending: bool,
}

/// A fully bound SELECT.
#[derive(Clone, Debug)]
pub(crate) struct BoundSelect {
    /// Filter.
    pub(crate) filter: Option<ExecExpr>,
    /// Projection list.
    pub(crate) items: Vec<BoundItem>,
    /// Grouping columns, empty for a global aggregate.
    pub(crate) group_by: Vec<ExecExpr>,
    /// Group filter.
    pub(crate) having: Option<GroupExpr>,
    /// Sort keys.
    pub(crate) order_by: Vec<BoundOrder>,
    /// Limit.
    pub(crate) limit: Option<usize>,
}

impl BoundSelect {
    pub(crate) fn has_aggregate(&self) -> bool {
        self.items.iter().any(|item| item.expr.has_aggregate())
            || self.having.as_ref().is_some_and(GroupExpr::has_aggregate)
            || self.order_by.iter().any(|order| order.expr.has_aggregate())
    }

    pub(crate) fn is_grouped(&self) -> bool {
        !self.group_by.is_empty() || self.has_aggregate()
    }
}

/// Resolves a column reference against a schema.
/// Column-not-found error carrying the closest names in scope.
fn unknown_column(name: &str, schema: &Schema, qualified: bool) -> SqlError {
    let candidates: Vec<String> = schema
        .columns
        .iter()
        .map(|column| {
            if qualified {
                column.name.clone()
            } else {
                column
                    .name
                    .rsplit_once('.')
                    .map_or_else(|| column.name.clone(), |(_, bare)| bare.to_string())
            }
        })
        .collect();
    SqlError::UnknownName {
        kind: NameKind::Column,
        name: name.to_string(),
        candidates: closest_names(name, candidates.iter().map(String::as_str)),
    }
}

fn resolve_column(schema: &Schema, table: Option<&str>, name: &str) -> Result<usize, SqlError> {
    if let Some(table) = table {
        let qualified = format!("{table}.{name}");
        return schema
            .index_of(&qualified)
            .ok_or_else(|| unknown_column(&qualified, schema, true));
    }
    if let Some(index) = schema.index_of(name) {
        return Ok(index);
    }
    let mut matches = schema
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| {
            column
                .name
                .rsplit_once('.')
                .filter(|(_, suffix)| *suffix == name)
                .map(|_| index)
        });
    let Some(index) = matches.next() else {
        return Err(unknown_column(name, schema, false));
    };
    if matches.next().is_some() {
        return Err(SqlError::AmbiguousColumn(name.to_string()));
    }
    Ok(index)
}

fn displayed_column(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, column)| column)
}

fn column_name(table: Option<&str>, name: &str) -> String {
    table.map_or_else(|| name.to_string(), |table| format!("{table}.{name}"))
}

/// Binds an expression to a schema.
pub(crate) fn bind_expr(expr: &Expr, schema: &Schema) -> Result<ExecExpr, SqlError> {
    match expr {
        Expr::Literal(value) => Ok(ExecExpr::Literal(value.clone())),
        Expr::Column { table, name } => {
            let index = resolve_column(schema, table.as_deref(), name)?;
            Ok(ExecExpr::Column(index))
        }
        Expr::Truth { expr, threshold } => Ok(ExecExpr::Truth {
            expr: Box::new(bind_expr(expr, schema)?),
            threshold: match threshold {
                Some(threshold) => Some(Box::new(bind_expr(threshold, schema)?)),
                None => None,
            },
        }),
        Expr::Certainty { expr, threshold } => Ok(ExecExpr::Certainty {
            expr: Box::new(bind_expr(expr, schema)?),
            threshold: match threshold {
                Some(threshold) => Some(Box::new(bind_expr(threshold, schema)?)),
                None => None,
            },
        }),
        Expr::IsNull { expr, negated } => Ok(ExecExpr::IsNull {
            expr: Box::new(bind_expr(expr, schema)?),
            negated: *negated,
        }),
        Expr::IsTruth {
            expr,
            expected,
            negated,
        } => Ok(ExecExpr::IsTruth {
            expr: Box::new(bind_expr(expr, schema)?),
            expected: *expected,
            negated: *negated,
        }),
        Expr::Not(expr) => Ok(ExecExpr::Not(Box::new(bind_expr(expr, schema)?))),
        Expr::InList {
            expr,
            values,
            negated,
        } => {
            let probe = Box::new(bind_expr(expr, schema)?);
            let bound = values
                .iter()
                .map(|value| bind_expr(value, schema))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some((set, type_name)) = constant_set(&bound) {
                return Ok(ExecExpr::InSet {
                    expr: probe,
                    values: set,
                    type_name,
                    negated: *negated,
                });
            }
            Ok(ExecExpr::InList {
                expr: probe,
                values: bound,
                negated: *negated,
            })
        }
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Ok(ExecExpr::Between {
            expr: Box::new(bind_expr(expr, schema)?),
            low: Box::new(bind_expr(low, schema)?),
            high: Box::new(bind_expr(high, schema)?),
            negated: *negated,
        }),
        Expr::Like {
            expr,
            pattern,
            negated,
        } => Ok(ExecExpr::Like {
            expr: Box::new(bind_expr(expr, schema)?),
            pattern: Box::new(bind_expr(pattern, schema)?),
            negated: *negated,
        }),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => Ok(ExecExpr::Case {
            operand: operand
                .as_deref()
                .map(|expr| bind_expr(expr, schema))
                .transpose()?
                .map(Box::new),
            branches: branches
                .iter()
                .map(|(when, then)| Ok((bind_expr(when, schema)?, bind_expr(then, schema)?)))
                .collect::<Result<_, SqlError>>()?,
            else_expr: else_expr
                .as_deref()
                .map(|expr| bind_expr(expr, schema))
                .transpose()?
                .map(Box::new),
        }),
        Expr::Binary { op, left, right } => Ok(ExecExpr::Binary {
            op: *op,
            left: Box::new(bind_expr(left, schema)?),
            right: Box::new(bind_expr(right, schema)?),
        }),
        Expr::InSubquery { .. }
        | Expr::InSubqueryTuple { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::Exists { .. }
        | Expr::Quantified { .. } => Err(SqlError::Unsupported(
            "subquery must be materialized before binding".to_string(),
        )),
        Expr::Tuple(_) => Err(SqlError::Unsupported(
            "parenthesized expression list is only allowed in IN".to_string(),
        )),
        Expr::Window { .. } => Err(SqlError::Unsupported(
            "window function must be a top-level select item".to_string(),
        )),
        Expr::Aggregate { .. } => Err(SqlError::Unsupported(
            "aggregate is only allowed in SELECT, HAVING, or ORDER BY".to_string(),
        )),
    }
}

/// Evaluates a constant expression with no row context.
pub(crate) fn eval_constant(expr: &Expr) -> Result<Value, SqlError> {
    let bound = bind_expr(expr, &Schema::default())?;
    Ok(eval(&bound, &Schema::default(), &[])?)
}

/// Default output label for an aggregate.
fn aggregate_label(kind: AggregateKind, input: Option<&Expr>) -> String {
    match input {
        Some(input) => format!("{}({})", kind.name(), expr_label(input)),
        None => "COUNT(*)".to_string(),
    }
}

fn expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Column { name, .. } => name.clone(),
        Expr::Literal(value) => value.display(),
        Expr::Aggregate { kind, input } => aggregate_label(*kind, input.as_deref()),
        _ => "expr".to_string(),
    }
}

fn expr_has_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Literal(_) | Expr::Column { .. } => false,
        Expr::IsNull { expr, .. } | Expr::IsTruth { expr, .. } => expr_has_aggregate(expr),
        Expr::Truth { expr, threshold } => {
            expr_has_aggregate(expr) || threshold.as_deref().is_some_and(expr_has_aggregate)
        }
        Expr::Certainty { expr, threshold } => {
            expr_has_aggregate(expr) || threshold.as_deref().is_some_and(expr_has_aggregate)
        }
        Expr::Not(expr) => expr_has_aggregate(expr),
        Expr::InList { expr, values, .. } => {
            expr_has_aggregate(expr) || values.iter().any(expr_has_aggregate)
        }
        Expr::Between {
            expr, low, high, ..
        } => expr_has_aggregate(expr) || expr_has_aggregate(low) || expr_has_aggregate(high),
        Expr::Like { expr, pattern, .. } => expr_has_aggregate(expr) || expr_has_aggregate(pattern),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand.as_deref().is_some_and(expr_has_aggregate)
                || branches
                    .iter()
                    .any(|(when, then)| expr_has_aggregate(when) || expr_has_aggregate(then))
                || else_expr.as_deref().is_some_and(expr_has_aggregate)
        }
        Expr::Binary { left, right, .. } => expr_has_aggregate(left) || expr_has_aggregate(right),
        Expr::InSubquery { expr, .. } => expr_has_aggregate(expr),
        Expr::InSubqueryTuple { exprs, .. } => exprs.iter().any(expr_has_aggregate),
        Expr::Quantified { expr, .. } => expr_has_aggregate(expr),
        Expr::Tuple(exprs) => exprs.iter().any(expr_has_aggregate),
        Expr::ScalarSubquery { .. } | Expr::Exists { .. } | Expr::Window { .. } => false,
        Expr::Aggregate { .. } => true,
    }
}

pub(crate) fn select_has_aggregate(select: &Select) -> bool {
    select.items.iter().any(|item| match item {
        SelectItem::Wildcard => false,
        SelectItem::Expr { expr, .. } => expr_has_aggregate(expr),
    }) || select.having.as_ref().is_some_and(expr_has_aggregate)
        || select
            .order_by
            .iter()
            .any(|order| expr_has_aggregate(&order.expr))
}

fn bind_group_expr(
    expr: &Expr,
    schema: &Schema,
    grouped: bool,
    aggregate_query: bool,
    group_columns: &BTreeSet<usize>,
) -> Result<GroupExpr, SqlError> {
    match expr {
        Expr::Literal(value) => Ok(GroupExpr::Scalar(ExecExpr::Literal(value.clone()))),
        Expr::Column { table, name } => {
            let index = resolve_column(schema, table.as_deref(), name)?;
            if aggregate_query && !grouped {
                return Err(SqlError::ColumnOutsideAggregate(column_name(
                    table.as_deref(),
                    name,
                )));
            }
            if grouped && !group_columns.contains(&index) {
                return Err(SqlError::ColumnOutsideGroup(column_name(
                    table.as_deref(),
                    name,
                )));
            }
            Ok(GroupExpr::Scalar(ExecExpr::Column(index)))
        }
        Expr::IsNull { expr, negated } => Ok(GroupExpr::IsNull {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            negated: *negated,
        }),
        Expr::IsTruth {
            expr,
            expected,
            negated,
        } => Ok(GroupExpr::IsTruth {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            expected: *expected,
            negated: *negated,
        }),
        Expr::Truth { expr, threshold } => Ok(GroupExpr::Truth {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            threshold: match threshold {
                Some(threshold) => Some(Box::new(bind_group_expr(
                    threshold,
                    schema,
                    grouped,
                    aggregate_query,
                    group_columns,
                )?)),
                None => None,
            },
        }),
        Expr::Certainty { expr, threshold } => Ok(GroupExpr::Certainty {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            threshold: match threshold {
                Some(threshold) => Some(Box::new(bind_group_expr(
                    threshold,
                    schema,
                    grouped,
                    aggregate_query,
                    group_columns,
                )?)),
                None => None,
            },
        }),
        Expr::Not(expr) => Ok(GroupExpr::Not(Box::new(bind_group_expr(
            expr,
            schema,
            grouped,
            aggregate_query,
            group_columns,
        )?))),
        Expr::InList {
            expr,
            values,
            negated,
        } => Ok(GroupExpr::InList {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            values: values
                .iter()
                .map(|value| {
                    bind_group_expr(value, schema, grouped, aggregate_query, group_columns)
                })
                .collect::<Result<_, _>>()?,
            negated: *negated,
        }),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Ok(GroupExpr::Between {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            low: Box::new(bind_group_expr(
                low,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            high: Box::new(bind_group_expr(
                high,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            negated: *negated,
        }),
        Expr::Like {
            expr,
            pattern,
            negated,
        } => Ok(GroupExpr::Like {
            expr: Box::new(bind_group_expr(
                expr,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            pattern: Box::new(bind_group_expr(
                pattern,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            negated: *negated,
        }),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            let operand = match operand {
                Some(expr) => Some(Box::new(bind_group_expr(
                    expr,
                    schema,
                    grouped,
                    aggregate_query,
                    group_columns,
                )?)),
                None => None,
            };
            let mut bound_branches = Vec::with_capacity(branches.len());
            for (when, then) in branches {
                bound_branches.push((
                    bind_group_expr(when, schema, grouped, aggregate_query, group_columns)?,
                    bind_group_expr(then, schema, grouped, aggregate_query, group_columns)?,
                ));
            }
            let else_expr = match else_expr {
                Some(expr) => Some(Box::new(bind_group_expr(
                    expr,
                    schema,
                    grouped,
                    aggregate_query,
                    group_columns,
                )?)),
                None => None,
            };
            Ok(GroupExpr::Case {
                operand,
                branches: bound_branches,
                else_expr,
            })
        }
        Expr::Binary { op, left, right } => Ok(GroupExpr::Binary {
            op: *op,
            left: Box::new(bind_group_expr(
                left,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
            right: Box::new(bind_group_expr(
                right,
                schema,
                grouped,
                aggregate_query,
                group_columns,
            )?),
        }),
        Expr::InSubquery { .. }
        | Expr::InSubqueryTuple { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::Exists { .. }
        | Expr::Quantified { .. } => Err(SqlError::Unsupported(
            "subquery must be materialized before binding".to_string(),
        )),
        Expr::Tuple(_) => Err(SqlError::Unsupported(
            "parenthesized expression list is only allowed in IN".to_string(),
        )),
        Expr::Window { .. } => Err(SqlError::Unsupported(
            "window function must be a top-level select item".to_string(),
        )),
        Expr::Aggregate { kind, input } => {
            let input = match input {
                Some(input) => {
                    if expr_has_aggregate(input) {
                        return Err(SqlError::NestedAggregate);
                    }
                    Some(bind_expr(input, schema)?)
                }
                None => None,
            };
            Ok(GroupExpr::Aggregate { kind: *kind, input })
        }
    }
}

/// Binds `SELECT` against a table.
pub(crate) fn bind_select(select: &Select, table: &Table) -> Result<BoundSelect, SqlError> {
    let schema = &table.schema;

    let mut group_by = Vec::new();
    let mut group_columns = BTreeSet::new();
    for expr in &select.group_by {
        let Expr::Column { table, name } = expr else {
            return Err(SqlError::Unsupported(
                "GROUP BY only supports column references".to_string(),
            ));
        };
        let index = resolve_column(schema, table.as_deref(), name)?;
        if group_columns.insert(index) {
            group_by.push(ExecExpr::Column(index));
        }
    }
    let grouped = !group_by.is_empty();
    let aggregate_query = grouped || select_has_aggregate(select);

    let mut items = Vec::new();
    for (position, item) in select.items.iter().enumerate() {
        match item {
            SelectItem::Wildcard => {
                if aggregate_query {
                    return Err(SqlError::Unsupported(
                        "SELECT * with GROUP BY or aggregate is not supported".to_string(),
                    ));
                }
                for (index, column) in schema.columns.iter().enumerate() {
                    items.push(BoundItem {
                        expr: GroupExpr::Scalar(ExecExpr::Column(index)),
                        name: displayed_column(&column.name).to_string(),
                    });
                }
            }
            SelectItem::Expr { expr, alias } => {
                let name = alias
                    .clone()
                    .or_else(|| match expr {
                        Expr::Column { name, .. } => Some(name.clone()),
                        Expr::Aggregate { kind, input } => {
                            Some(aggregate_label(*kind, input.as_deref()))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| format!("col{}", position + 1));
                items.push(BoundItem {
                    expr: bind_group_expr(expr, schema, grouped, aggregate_query, &group_columns)?,
                    name,
                });
            }
        }
    }

    if items.is_empty() {
        return Err(SqlError::Unsupported("empty SELECT list".to_string()));
    }

    let filter = select
        .filter
        .as_ref()
        .map(|expr| bind_expr(expr, schema))
        .transpose()?;

    let having = match &select.having {
        Some(having) => {
            if !aggregate_query {
                return Err(SqlError::Unsupported(
                    "HAVING requires GROUP BY or an aggregate".to_string(),
                ));
            }
            Some(bind_group_expr(
                having,
                schema,
                grouped,
                aggregate_query,
                &group_columns,
            )?)
        }
        None => None,
    };

    let aliases: BTreeMap<&str, &GroupExpr> =
        items.iter().map(|item| (item.name(), &item.expr)).collect();

    let mut order_by = Vec::new();
    for order in &select.order_by {
        let expr = if let Expr::Column { table, name } = &order.expr {
            if table.is_some() {
                bind_group_expr(
                    &order.expr,
                    schema,
                    grouped,
                    aggregate_query,
                    &group_columns,
                )?
            } else {
                match resolve_column(schema, None, name) {
                    Ok(_) => bind_group_expr(
                        &order.expr,
                        schema,
                        grouped,
                        aggregate_query,
                        &group_columns,
                    )?,
                    Err(
                        SqlError::UnknownColumn(_)
                        | SqlError::UnknownName {
                            kind: NameKind::Column,
                            ..
                        },
                    ) => {
                        if let Some(alias) = aliases.get(name.as_str()) {
                            (*alias).clone()
                        } else {
                            return Err(SqlError::UnknownColumn(name.clone()));
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        } else {
            bind_group_expr(
                &order.expr,
                schema,
                grouped,
                aggregate_query,
                &group_columns,
            )?
        };
        order_by.push(BoundOrder {
            expr,
            ascending: order.ascending,
        });
    }

    Ok(BoundSelect {
        filter,
        items,
        group_by,
        having,
        order_by,
        limit: select.limit,
    })
}
