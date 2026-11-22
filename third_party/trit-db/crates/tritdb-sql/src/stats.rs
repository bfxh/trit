use std::collections::HashSet;

use tritdb_exec::{Table, Value};

/// Statistics for one column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnStats {
    /// Column name.
    pub name: String,
    /// Number of `NULL` values.
    pub nulls: u64,
    /// Number of distinct non-`NULL` values.
    pub distinct: u64,
}

/// Exact table statistics produced by `ANALYZE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableStats {
    /// Row count.
    pub rows: u64,
    /// Column statistics in schema order.
    pub columns: Vec<ColumnStats>,
}

/// Computes exact table statistics by scanning the in-memory table.
///
/// `NULL` values are counted separately and excluded from the distinct count.
pub fn analyze_table(table: &Table) -> TableStats {
    let mut nulls = vec![0u64; table.schema.len()];
    let mut distinct = (0..table.schema.len())
        .map(|_| HashSet::new())
        .collect::<Vec<HashSet<Value>>>();
    for row in &table.rows {
        for (index, value) in row.iter().enumerate() {
            if value.is_null() {
                nulls[index] += 1;
            } else {
                distinct[index].insert(value.clone());
            }
        }
    }

    let columns = table
        .schema
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| ColumnStats {
            name: column.name.clone(),
            nulls: nulls[index],
            distinct: distinct[index].len() as u64,
        })
        .collect();
    TableStats {
        rows: table.rows.len() as u64,
        columns,
    }
}

#[cfg(test)]
mod tests {
    use tritdb_exec::{Column, DataType, Schema, Value};

    use super::*;

    #[test]
    fn counts_nulls_and_distinct_values() {
        let table = Table {
            name: "t".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("name", DataType::Text),
            ]),
            rows: vec![
                vec![Value::Int(1), Value::Text("a".to_string())],
                vec![Value::Int(1), Value::Null],
                vec![Value::Null, Value::Text("a".to_string())],
                vec![Value::Int(2), Value::Text("b".to_string())],
            ],
        };
        let stats = analyze_table(&table);
        assert_eq!(stats.rows, 4);
        assert_eq!(
            stats.columns,
            [
                ColumnStats {
                    name: "id".to_string(),
                    nulls: 1,
                    distinct: 2,
                },
                ColumnStats {
                    name: "name".to_string(),
                    nulls: 1,
                    distinct: 2,
                },
            ]
        );
    }
}
