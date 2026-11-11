//! 表 / 值模型：M0 只有 `Int`、`Text` 与 `NULL` 三种值。
//!
//! 语义口径与 SQL 对齐：`NULL` 参与任何比较都得到 UNKNOWN（由 `Value::compare` 返回 `None`），
//! 而不是“等于自己”或“小于一切”。

use std::cmp::Ordering;

/// 列类型。`NULL` 对任何类型都合法，除非有 `NOT NULL` 规则。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    /// 整数（i64）。
    Int,
    /// 文本（二进制序比较，与 SQLite 的 BINARY collation 一致）。
    Text,
}

/// 单元格值。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Value {
    /// SQL 的 `NULL`（三值语义里的 UNKNOWN 来源）。
    Null,
    /// 整数。
    Int(i64),
    /// 文本。
    Text(String),
}

impl Value {
    /// 值自身的类型；`NULL` 没有类型。
    #[must_use]
    pub fn ty(&self) -> Option<Ty> {
        match self {
            Self::Null => None,
            Self::Int(_) => Some(Ty::Int),
            Self::Text(_) => Some(Ty::Text),
        }
    }

    /// 是否为 `NULL`。
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// SQL 三值比较：任一侧为 `NULL`（或类型不匹配）⇒ `None`（UNKNOWN）。
    #[must_use]
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Null, _) | (_, Self::Null) => None,
            (Self::Int(a), Self::Int(b)) => Some(a.cmp(b)),
            (Self::Text(a), Self::Text(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }
}

/// 列定义。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    /// 列名（规则 DSL 里用它引用）。
    pub name: String,
    /// 列类型。
    pub ty: Ty,
}

impl Column {
    /// 新建列。
    #[must_use]
    pub fn new(name: &str, ty: Ty) -> Self {
        Self {
            name: name.to_string(),
            ty,
        }
    }
}

/// 内存表：列定义 + 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// 列。
    pub columns: Vec<Column>,
    /// 行；每行长度必须等于列数，非 `NULL` 值必须匹配列类型。
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// 建表。
    #[must_use]
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    /// 按列名查下标（DSL 解析用）。
    #[must_use]
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// 追加一行；宽度或类型不符返回结构化错误。
    pub fn push_row(&mut self, row: Vec<Value>) -> Result<(), TableError> {
        let row_idx = self.rows.len();
        if row.len() != self.columns.len() {
            return Err(TableError::Width {
                row: row_idx,
                expected: self.columns.len(),
                actual: row.len(),
            });
        }
        for (col, (value, column)) in row.iter().zip(&self.columns).enumerate() {
            if let Some(ty) = value.ty()
                && ty != column.ty
            {
                return Err(TableError::Type {
                    row: row_idx,
                    col,
                    expected: column.ty,
                    actual: ty,
                });
            }
        }
        self.rows.push(row);
        Ok(())
    }
}

/// 建表 / 填行时的结构化错误。
#[derive(Debug, thiserror::Error)]
pub enum TableError {
    /// 行宽与列数不符。
    #[error("第 {row} 行列数不符：期望 {expected}，实际 {actual}")]
    Width {
        /// 行号（0 起）。
        row: usize,
        /// 期望列数。
        expected: usize,
        /// 实际列数。
        actual: usize,
    },
    /// 值类型与列类型不符。
    #[error("第 {row} 行第 {col} 列类型不符：列是 {expected:?}，值是 {actual:?}")]
    Type {
        /// 行号（0 起）。
        row: usize,
        /// 列下标（0 起）。
        col: usize,
        /// 列类型。
        expected: Ty,
        /// 值类型。
        actual: Ty,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_compares_unknown() {
        assert_eq!(Value::Null.compare(&Value::Int(0)), None);
        assert_eq!(Value::Int(0).compare(&Value::Null), None);
        assert_eq!(Value::Null.compare(&Value::Null), None);
    }

    #[test]
    fn int_and_text_compare() {
        assert_eq!(Value::Int(2).compare(&Value::Int(3)), Some(Ordering::Less));
        assert_eq!(
            Value::Text("a".into()).compare(&Value::Text("b".into())),
            Some(Ordering::Less)
        );
        assert_eq!(
            Value::Text("b".into()).compare(&Value::Text("b".into())),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn push_row_rejects_width_mismatch() {
        let mut t = Table::new(vec![Column::new("a", Ty::Int)]);
        let err = t.push_row(vec![]).unwrap_err();
        assert!(matches!(err, TableError::Width { .. }), "{err}");
    }

    #[test]
    fn push_row_rejects_type_mismatch() {
        let mut t = Table::new(vec![Column::new("a", Ty::Int)]);
        let err = t.push_row(vec![Value::Text("x".into())]).unwrap_err();
        assert!(matches!(err, TableError::Type { .. }), "{err}");
        t.push_row(vec![Value::Null]).unwrap();
        assert_eq!(t.rows.len(), 1);
    }
}
