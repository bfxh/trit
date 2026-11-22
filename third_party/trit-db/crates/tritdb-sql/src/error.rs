use std::error::Error;
use std::fmt;

use tritdb_exec::ExecError;
use tritdb_txn::TxnError;

/// Which namespace an [`SqlError::UnknownName`] refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    /// Column namespace.
    Column,
    /// Table namespace.
    Table,
}

/// SQL front-end error.
#[derive(Debug)]
pub enum SqlError {
    /// Lexer or parser error.
    Parse {
        /// Human-readable message.
        message: String,
        /// Character offset in the input.
        position: usize,
    },
    /// Table does not exist.
    UnknownTable(String),
    /// Table already exists.
    TableExists(String),
    /// Column does not exist in the table.
    UnknownColumn(String),
    /// A column or table name that does not exist, with the closest known
    /// names so a caller (or an agent) can fix a typo without guessing.
    UnknownName {
        /// Which namespace the name belongs to.
        kind: NameKind,
        /// The name that was not found.
        name: String,
        /// Closest known names; empty when nothing is close enough.
        candidates: Vec<String>,
    },
    /// Unqualified column name matches more than one joined column.
    AmbiguousColumn(String),
    /// Duplicate table name or alias in a `FROM` clause.
    DuplicateTableAlias(String),
    /// Duplicate column in `CREATE TABLE`.
    DuplicateColumn(String),
    /// `INSERT` value count does not match the target column count.
    ColumnCount {
        /// Table name.
        table: String,
        /// Expected count.
        expected: usize,
        /// Actual count.
        actual: usize,
    },
    /// `NOT NULL` column received `NULL`.
    NotNull {
        /// Table name.
        table: String,
        /// Column name.
        column: String,
    },
    /// A `CHECK` constraint evaluated to `FALSE`.
    CheckViolation {
        /// Table name.
        table: String,
        /// Constraint text.
        expression: String,
    },
    /// A `UNIQUE` column received a duplicate non-`NULL` value.
    DuplicateKey {
        /// Table name.
        table: String,
        /// Column name.
        column: String,
        /// Duplicated value in display form.
        value: String,
    },
    /// Aggregate expression is nested inside another aggregate.
    NestedAggregate,
    /// A column reference appears outside an aggregate in an aggregate query.
    ColumnOutsideAggregate(String),
    /// A column reference is not part of the `GROUP BY` key.
    ColumnOutsideGroup(String),
    /// Unsupported SQL feature.
    Unsupported(String),
    /// Persisted table value is corrupt.
    CorruptTable {
        /// Table name.
        table: String,
        /// Human-readable reason.
        reason: String,
    },
    /// Persisted table statistics are corrupt or do not match the table.
    CorruptStats {
        /// Table name.
        table: String,
        /// Human-readable reason.
        reason: String,
    },
    /// Expression evaluation error.
    Exec(ExecError),
    /// Transaction/WAL error.
    Txn(TxnError),
    /// I/O error.
    Io(std::io::Error),
}

/// Truncates an identifier echoed in a hint, so a 16 MiB name cannot turn
/// every error line into a 16 MiB log entry.
fn hint_name(name: &str) -> String {
    const MAX: usize = 128;
    if name.chars().count() <= MAX {
        return name.to_string();
    }
    let truncated: String = name.chars().take(MAX).collect();
    format!("{truncated}…")
}

impl SqlError {
    pub(crate) fn parse(message: impl Into<String>, position: usize) -> Self {
        Self::Parse {
            message: message.into(),
            position,
        }
    }

    /// Stable PostgreSQL SQLSTATE for this error.
    ///
    /// Models have seen a lot of PostgreSQL errors, so the five-character
    /// SQLSTATE is a shared vocabulary: a client can branch on it without
    /// parsing prose, and a retry loop can tell "fixable" (`42703` unknown
    /// column) from "fatal" (`XX001` corrupt data).
    pub fn code(&self) -> &'static str {
        match self {
            SqlError::Parse { .. } => "42601",     // syntax_error
            SqlError::UnknownTable(_) => "42P01",  // undefined_table
            SqlError::TableExists(_) => "42P07",   // duplicate_table
            SqlError::UnknownColumn(_) => "42703", // undefined_column
            SqlError::UnknownName {
                kind: NameKind::Column,
                ..
            } => "42703", // undefined_column
            SqlError::UnknownName {
                kind: NameKind::Table,
                ..
            } => "42P01", // undefined_table
            SqlError::AmbiguousColumn(_) => "42702", // ambiguous_column
            SqlError::DuplicateTableAlias(_) => "42712", // duplicate_alias
            SqlError::DuplicateColumn(_) => "42701", // duplicate_column
            SqlError::ColumnCount { .. } => "42601", // syntax_error
            SqlError::NotNull { .. } => "23502",   // not_null_violation
            SqlError::CheckViolation { .. } => "23514", // check_violation
            SqlError::DuplicateKey { .. } => "23505", // unique_violation
            SqlError::NestedAggregate => "42803",  // grouping_error
            SqlError::ColumnOutsideAggregate(_) => "42803",
            SqlError::ColumnOutsideGroup(_) => "42803",
            SqlError::Unsupported(_) => "0A000", // feature_not_supported
            SqlError::CorruptTable { .. } => "XX001", // data_corrupted
            SqlError::CorruptStats { .. } => "XX000", // internal_error
            SqlError::Exec(error) => match error {
                ExecError::TypeMismatch { .. } | ExecError::NonBooleanPredicate { .. } => "42804", // datatype_mismatch
                ExecError::DivisionByZero => "22012", // division_by_zero
                ExecError::IntegerOverflow => "22003", // numeric_value_out_of_range
                ExecError::RowShape { .. } | ExecError::ColumnIndex { .. } => "XX000",
                ExecError::LikePatternTooLong { .. } | ExecError::LikeTooComplex => "22023", // invalid_parameter_value
            },
            SqlError::Txn(_) => "40000", // transaction_rollback
            SqlError::Io(_) => "58030",  // io_error
        }
    }

    /// Machine-actionable repair hint, when one is safe to give.
    ///
    /// This is the "what to do next" half of the error: the code says which
    /// class of problem it is, the hint says which edit usually fixes it.
    pub fn hint(&self) -> Option<String> {
        match self {
            SqlError::Parse { .. } => None,
            SqlError::UnknownTable(name) => Some(format!(
                "check the table name `{}`; create it first if it is missing",
                hint_name(name)
            )),
            SqlError::TableExists(name) => Some(format!(
                "table `{}` already exists; drop it first or pick another name",
                hint_name(name)
            )),
            SqlError::UnknownColumn(name) => Some(format!(
                "check the column name `{}`; qualify it as `table.{}` when several tables share it",
                hint_name(name),
                hint_name(name)
            )),
            SqlError::UnknownName {
                kind,
                name,
                candidates,
            } => {
                let mut hint = format!(
                    "check the {} name `{}`",
                    unknown_kind(*kind),
                    hint_name(name)
                );
                if candidates.is_empty() {
                    hint.push_str(match kind {
                        NameKind::Table => "; create it first if it is missing",
                        NameKind::Column => "; qualify it as `table.<column>` when several tables share it",
                    });
                } else {
                    hint.push_str(&format!("; did you mean {}?", quoted_list(candidates)));
                }
                Some(hint)
            }
            SqlError::AmbiguousColumn(name) => Some(format!(
                "qualify `{}` as `table.{}`",
                hint_name(name),
                hint_name(name)
            )),
            SqlError::DuplicateTableAlias(name) => Some(format!(
                "`{}` is used twice in FROM; give each item a distinct alias",
                hint_name(name)
            )),
            SqlError::DuplicateColumn(name) => Some(format!(
                "column `{}` is declared twice; rename one of them",
                hint_name(name)
            )),
            SqlError::ColumnCount {
                table,
                expected,
                actual,
            } => Some(format!(
                "INSERT INTO {} needs {expected} values but got {actual}; list the target columns if the row is partial",
                hint_name(table)
            )),
            SqlError::NotNull { table, column } => Some(format!(
                "supply a value for `{}.{}` or make the column nullable",
                hint_name(table),
                hint_name(column)
            )),
            SqlError::CheckViolation { table, expression } => Some(format!(
                "the row violates `CHECK ({})` on `{}`; adjust the values",
                hint_name(expression),
                hint_name(table)
            )),
            SqlError::DuplicateKey {
                table,
                column,
                value,
            } => Some(format!(
                "`{}.{}` is UNIQUE; `{}` already exists, pick another value",
                hint_name(table),
                hint_name(column),
                hint_name(value)
            )),
            SqlError::NestedAggregate => {
                Some("compute the inner aggregate in a subquery first".to_string())
            }
            SqlError::ColumnOutsideAggregate(name) => Some(format!(
                "wrap `{}` in an aggregate (SUM/COUNT/MIN/MAX/AVG) or add it to GROUP BY",
                hint_name(name)
            )),
            SqlError::ColumnOutsideGroup(name) => Some(format!(
                "add `{}` to GROUP BY or wrap it in an aggregate",
                hint_name(name)
            )),
            SqlError::Unsupported(_) => Some(
                "this build does not implement it; see docs/specs/SQL.md for the supported subset"
                    .to_string(),
            ),
            SqlError::CorruptTable { .. } | SqlError::CorruptStats { .. } => Some(
                "restore from a snapshot or checkpoint, then verify the table".to_string(),
            ),
            SqlError::Exec(ExecError::TypeMismatch { expected, .. }) => Some(format!(
                "cast explicitly: `CAST(expr AS {expected})`"
            )),
            SqlError::Exec(ExecError::DivisionByZero) => Some(
                "guard the divisor, e.g. `CASE WHEN d = 0 THEN NULL ELSE n / d END`".to_string(),
            ),
            SqlError::Exec(ExecError::IntegerOverflow) => {
                Some("split the expression or use a narrower range".to_string())
            }
            SqlError::Exec(ExecError::NonBooleanPredicate { .. }) => Some(
                "use `truth(expr)` / `certainty(expr)` for annotated columns, or compare explicitly"
                    .to_string(),
            ),
            SqlError::Exec(ExecError::RowShape { .. }) | SqlError::Exec(ExecError::ColumnIndex { .. }) => {
                Some("internal row/schema shape mismatch; retry once and report the query".to_string())
            }
            SqlError::Exec(ExecError::LikePatternTooLong { .. })
            | SqlError::Exec(ExecError::LikeTooComplex) => Some(
                "shorten the LIKE pattern or split the match into simpler patterns".to_string(),
            ),
            SqlError::Txn(_) => Some("retry the statement or ROLLBACK the transaction".to_string()),
            SqlError::Io(_) => None,
        }
    }

    /// One-line-plus-hint rendering used by the CLI: keeps the human message,
    /// appends the SQLSTATE, and adds the repair hint on its own line.
    pub fn format_structured(&self) -> String {
        let mut out = format!("{self} [SQLSTATE {}]", self.code());
        if let Some(hint) = self.hint() {
            out.push_str(&format!("\nhint: {hint}"));
        }
        out
    }
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlError::Parse { message, position } => {
                write!(f, "SQL parse error at character {position}: {message}")
            }
            SqlError::UnknownTable(name) => write!(f, "unknown table: {name}"),
            SqlError::TableExists(name) => write!(f, "table already exists: {name}"),
            SqlError::UnknownColumn(name) => write!(f, "unknown column: {name}"),
            SqlError::UnknownName {
                kind,
                name,
                candidates,
            } => {
                write!(f, "{}: {name}", unknown_kind_label(*kind))?;
                if !candidates.is_empty() {
                    write!(f, " (did you mean {}?)", quoted_list(candidates))?;
                }
                Ok(())
            }
            SqlError::AmbiguousColumn(name) => write!(f, "ambiguous column: {name}"),
            SqlError::DuplicateTableAlias(name) => {
                write!(f, "duplicate table name or alias: {name}")
            }
            SqlError::DuplicateColumn(name) => write!(f, "duplicate column: {name}"),
            SqlError::ColumnCount {
                table,
                expected,
                actual,
            } => write!(
                f,
                "INSERT into {table} expects {expected} values, got {actual}"
            ),
            SqlError::NotNull { table, column } => {
                write!(f, "{table}.{column} is NOT NULL")
            }
            SqlError::DuplicateKey {
                table,
                column,
                value,
            } => write!(f, "{table}.{column} UNIQUE violated by {value}"),
            SqlError::CheckViolation { table, expression } => {
                write!(f, "{table} CHECK violated: {expression}")
            }
            SqlError::NestedAggregate => write!(f, "nested aggregate is not supported"),
            SqlError::ColumnOutsideAggregate(name) => {
                write!(f, "column {name} must appear inside an aggregate")
            }
            SqlError::ColumnOutsideGroup(name) => {
                write!(f, "column {name} must appear in GROUP BY")
            }
            SqlError::Unsupported(feature) => write!(f, "unsupported SQL: {feature}"),
            SqlError::CorruptTable { table, reason } => {
                write!(f, "corrupt table {table}: {reason}")
            }
            SqlError::CorruptStats { table, reason } => {
                write!(f, "corrupt statistics for table {table}: {reason}")
            }
            SqlError::Exec(error) => write!(f, "SQL evaluation error: {error}"),
            SqlError::Txn(error) => write!(f, "SQL transaction error: {error}"),
            SqlError::Io(error) => write!(f, "SQL I/O error: {error}"),
        }
    }
}

impl Error for SqlError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SqlError::Exec(error) => Some(error),
            SqlError::Txn(error) => Some(error),
            SqlError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ExecError> for SqlError {
    fn from(error: ExecError) -> Self {
        SqlError::Exec(error)
    }
}

impl From<TxnError> for SqlError {
    fn from(error: TxnError) -> Self {
        SqlError::Txn(error)
    }
}

impl From<std::io::Error> for SqlError {
    fn from(error: std::io::Error) -> Self {
        SqlError::Io(error)
    }
}

fn unknown_kind(kind: NameKind) -> &'static str {
    match kind {
        NameKind::Column => "column",
        NameKind::Table => "table",
    }
}

fn unknown_kind_label(kind: NameKind) -> &'static str {
    match kind {
        NameKind::Column => "unknown column",
        NameKind::Table => "unknown table",
    }
}

fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Returns up to three known names close to `name` (case-insensitive
/// Levenshtein, budget grows with the name length).
pub(crate) fn closest_names<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let target = name.to_ascii_lowercase();
    let budget = (target.chars().count() / 2).clamp(1, 3);
    let mut scored: Vec<(usize, String)> = candidates
        .into_iter()
        .map(|candidate| {
            (
                levenshtein(&target, &candidate.to_ascii_lowercase()),
                candidate.to_string(),
            )
        })
        .filter(|(distance, _)| *distance <= budget)
        .collect();
    scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    scored.dedup_by(|left, right| left.1 == right.1);
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}

fn levenshtein(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_sqlstates() {
        assert_eq!(SqlError::UnknownTable("t".into()).code(), "42P01");
        assert_eq!(SqlError::TableExists("t".into()).code(), "42P07");
        assert_eq!(SqlError::UnknownColumn("c".into()).code(), "42703");
        assert_eq!(SqlError::AmbiguousColumn("c".into()).code(), "42702");
        assert_eq!(SqlError::ColumnOutsideGroup("c".into()).code(), "42803");
        assert_eq!(SqlError::ColumnOutsideAggregate("c".into()).code(), "42803");
        assert_eq!(
            SqlError::NotNull {
                table: "t".into(),
                column: "c".into()
            }
            .code(),
            "23502"
        );
        assert_eq!(
            SqlError::DuplicateKey {
                table: "t".into(),
                column: "c".into(),
                value: "1".into()
            }
            .code(),
            "23505"
        );
        assert_eq!(
            SqlError::CheckViolation {
                table: "t".into(),
                expression: "x > 0".into()
            }
            .code(),
            "23514"
        );
        assert_eq!(SqlError::Unsupported("x".into()).code(), "0A000");
        assert_eq!(SqlError::Exec(ExecError::DivisionByZero).code(), "22012");
        assert_eq!(SqlError::Exec(ExecError::IntegerOverflow).code(), "22003");
        assert_eq!(
            SqlError::Exec(ExecError::TypeMismatch {
                expected: "INT",
                actual: "TEXT"
            })
            .code(),
            "42804"
        );
        assert_eq!(SqlError::parse("bad", 3).code(), "42601");
    }

    #[test]
    fn hint_echoes_are_bounded() {
        let long = "x".repeat(4096);
        let hint = SqlError::UnknownColumn(long.clone()).hint().expect("hint");
        assert!(hint.chars().count() < 512, "hint must be bounded: {hint}");
        assert!(hint.contains('…'));
        let hint = SqlError::UnknownTable("t".to_string())
            .hint()
            .expect("hint");
        assert!(hint.contains('t'));
    }

    #[test]
    fn hints_are_actionable() {
        let hint = SqlError::UnknownColumn("stat".into()).hint().unwrap();
        assert!(hint.contains("stat") && hint.contains("table.stat"));
        let hint = SqlError::ColumnOutsideGroup("v".into()).hint().unwrap();
        assert!(hint.contains("GROUP BY"));
        let hint = SqlError::Exec(ExecError::DivisionByZero).hint().unwrap();
        assert!(hint.contains("divisor"));
        assert!(SqlError::parse("bad", 3).hint().is_none());
        assert!(SqlError::Io(std::io::Error::other("x")).hint().is_none());
    }

    #[test]
    fn closest_names_suggests_typos() {
        assert_eq!(
            closest_names("stat", ["status", "id", "name"]),
            vec!["status".to_string()]
        );
        assert_eq!(
            closest_names("custmer", ["customer", "order"]),
            vec!["customer".to_string()]
        );
        // Case-insensitive.
        assert_eq!(
            closest_names("STAT", ["status"]),
            vec!["status".to_string()]
        );
        // Too far away: no suggestion instead of a bad one.
        assert!(closest_names("zzz", ["status", "id"]).is_empty());
        // At most three, sorted by distance then name.
        let many = closest_names("user1", ["user2", "user3", "user4", "user9"]);
        assert_eq!(many.len(), 3);
    }

    #[test]
    fn structured_format_carries_code_and_hint() {
        let formatted = SqlError::UnknownTable("nope".into()).format_structured();
        assert!(formatted.contains("[SQLSTATE 42P01]"), "{formatted}");
        assert!(formatted.contains("hint: "), "{formatted}");
        assert!(formatted.contains("nope"), "{formatted}");
    }
}
