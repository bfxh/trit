use tritdb_exec::batch::{B4Column, K3Column, LukasiewiczColumn, NatColumn};
use tritdb_exec::{Column, DataType, ExecError, Schema, Table, Value};

use crate::error::SqlError;
use crate::stats::{ColumnStats, TableStats};

/// Prefix for table values in the key/value store.
pub const TABLE_KEY_PREFIX: &str = "sql/table/";
/// Prefix for `ANALYZE` statistics in the key/value store.
pub const STATS_KEY_PREFIX: &str = "sql/stats/";

const TABLE_MAGIC: [u8; 4] = *b"TRSQ";
const TABLE_VERSION: u8 = 6;
const PACKED_MAGIC: [u8; 4] = *b"K3PK";
const PACKED_VERSION: u8 = 3;
const PACKED_CHUNK: usize = 4096;
const STATS_MAGIC: [u8; 4] = *b"TRSS";
const STATS_VERSION: u8 = 0;
const MAX_COLUMNS: usize = 1024;
/// Upper bound on rows decoded from one table value (memory-DoS guard).
const MAX_ROWS: u64 = 1 << 20;

/// Builds the key for a table.
pub fn table_key(name: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(TABLE_KEY_PREFIX.len() + name.len());
    key.extend_from_slice(TABLE_KEY_PREFIX.as_bytes());
    key.extend_from_slice(name.as_bytes());
    key
}

/// Builds the key for table statistics.
pub fn stats_key(name: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(STATS_KEY_PREFIX.len() + name.len());
    key.extend_from_slice(STATS_KEY_PREFIX.as_bytes());
    key.extend_from_slice(name.as_bytes());
    key
}

fn put_u32(out: &mut Vec<u8>, value: usize) -> Result<(), SqlError> {
    let value = u32::try_from(value)
        .map_err(|_| SqlError::Unsupported("table field exceeds u32".to_string()))?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_text(out: &mut Vec<u8>, value: &str) -> Result<(), SqlError> {
    put_u32(out, value.len())?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn type_tag(data_type: DataType) -> u8 {
    match data_type {
        DataType::Int => 0,
        DataType::Text => 1,
        DataType::Bool => 2,
        DataType::Logic => 3,
        DataType::Belnap => 4,
        DataType::Lukasiewicz => 5,
        DataType::Nat => 6,
    }
}

fn type_from_tag(tag: u8) -> Option<DataType> {
    match tag {
        0 => Some(DataType::Int),
        1 => Some(DataType::Text),
        2 => Some(DataType::Bool),
        3 => Some(DataType::Logic),
        4 => Some(DataType::Belnap),
        5 => Some(DataType::Lukasiewicz),
        6 => Some(DataType::Nat),
        _ => None,
    }
}

/// Encodes a table into the value stored under the `sql/table/` key prefix.
pub fn encode_table(table: &Table) -> Result<Vec<u8>, SqlError> {
    encode_table_versioned(table, TABLE_VERSION)
}

/// Encodes a table with an explicit format version (v3/v4 stay readable).
fn encode_table_versioned(table: &Table, version: u8) -> Result<Vec<u8>, SqlError> {
    if table.schema.columns.len() > MAX_COLUMNS {
        return Err(SqlError::Unsupported(format!(
            "table {} has more than {MAX_COLUMNS} columns",
            table.name
        )));
    }
    if table.rows.len() as u64 > MAX_ROWS {
        // Writing more rows than `decode_table` accepts would leave a table
        // that can never be read back.
        return Err(SqlError::Unsupported(format!(
            "table {} has more than {MAX_ROWS} rows",
            table.name
        )));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&TABLE_MAGIC);
    out.push(version);
    put_u32(&mut out, table.schema.columns.len())?;
    for column in &table.schema.columns {
        out.push(type_tag(column.data_type));
        out.push(u8::from(column.not_null));
        out.push(u8::from(column.unique));
        put_text(&mut out, column.check.as_deref().unwrap_or(""))?;
        put_text(&mut out, &column.name)?;
    }
    put_u32(&mut out, table.schema.unique_sets.len())?;
    for set in &table.schema.unique_sets {
        put_u32(&mut out, set.len())?;
        for &index in set {
            put_u32(&mut out, index)?;
        }
    }
    put_u32(&mut out, table.schema.checks.len())?;
    for check in &table.schema.checks {
        put_text(&mut out, check)?;
    }
    out.extend_from_slice(&(table.rows.len() as u64).to_le_bytes());
    for row in &table.rows {
        if row.len() != table.schema.columns.len() {
            return Err(SqlError::Exec(ExecError::RowShape {
                expected: table.schema.columns.len(),
                actual: row.len(),
            }));
        }
        for (value, column) in row.iter().zip(&table.schema.columns) {
            // v5 packs K3/Belnap; v6 also packs Ł/ℕ.
            let skipped = if version >= 6 {
                is_packed(column.data_type)
            } else {
                version >= 5 && matches!(column.data_type, DataType::Logic | DataType::Belnap)
            };
            if skipped {
                continue;
            }
            encode_value(&mut out, value, column.data_type, &table.name, &column.name)?;
        }
    }
    if version >= 4 {
        out.extend_from_slice(&PACKED_MAGIC);
        if version >= 6 {
            // v6: chunked segments for every annotation type.
            out.push(3);
            let packed = packed_columns(table, version);
            put_u32(&mut out, packed.len())?;
            for (index, kind, bytes, nulls) in packed {
                put_u32(&mut out, index)?;
                out.push(kind);
                put_u32(&mut out, table.rows.len())?;
                let chunks: Vec<&[u8]> = bytes.chunks(PACKED_CHUNK).collect();
                put_u32(&mut out, chunks.len())?;
                for chunk in chunks {
                    put_u32(&mut out, chunk.len())?;
                    out.extend_from_slice(chunk);
                }
                put_u32(&mut out, nulls.len())?;
                out.extend_from_slice(&nulls);
            }
        } else if version == 5 {
            // v5: single-block segments (K3 r3 + Belnap two-trit).
            out.push(2);
            let packed = packed_columns(table, version);
            put_u32(&mut out, packed.len())?;
            for (index, kind, bytes, nulls) in packed {
                put_u32(&mut out, index)?;
                out.push(kind);
                put_u32(&mut out, table.rows.len())?;
                put_u32(&mut out, bytes.len())?;
                put_u32(&mut out, nulls.len())?;
                out.extend_from_slice(&bytes);
                out.extend_from_slice(&nulls);
            }
        } else {
            // v4: row values stay authoritative; the segment is redundant.
            out.push(1);
            let packed: Vec<(usize, K3Column)> = table
                .schema
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| column.data_type == DataType::Logic)
                .filter_map(|(index, _)| table.k3_column(index).map(|column| (index, column)))
                .collect();
            put_u32(&mut out, packed.len())?;
            for (index, column) in packed {
                put_u32(&mut out, index)?;
                put_u32(&mut out, column.len())?;
                let bytes = column.packed().bytes();
                put_u32(&mut out, bytes.len())?;
                out.extend_from_slice(bytes);
            }
        }
    }
    Ok(out)
}

/// Annotation columns packed into the v5 segment.
fn is_packed(data_type: DataType) -> bool {
    matches!(
        data_type,
        DataType::Logic | DataType::Belnap | DataType::Lukasiewicz | DataType::Nat
    )
}

/// Packed annotation columns for v5: `(index, kind, packed, nulls)`.
fn packed_columns(table: &Table, version: u8) -> Vec<(usize, u8, Vec<u8>, Vec<u8>)> {
    table
        .schema
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| match column.data_type {
            DataType::Logic => table
                .k3_column(index)
                .map(|column| (index, 0, column.packed().bytes().to_vec(), Vec::new())),
            DataType::Belnap => {
                let column = B4Column::from_rows(&table.rows, index)?;
                let (packed, nulls) = if version >= 6 {
                    (column.packed().to_vec(), column.nulls().to_vec())
                } else {
                    column.to_parts_2trit()
                };
                Some((index, 1, packed, nulls))
            }
            DataType::Lukasiewicz if version >= 6 => {
                LukasiewiczColumn::from_rows(&table.rows, index)
                    .map(|column| (index, 2, column.packed().to_vec(), column.nulls().to_vec()))
            }
            DataType::Nat if version >= 6 => NatColumn::from_rows(&table.rows, index)
                .map(|column| (index, 3, column.packed().to_vec(), column.nulls().to_vec())),
            _ => None,
        })
        .collect()
}

/// Encodes exact table statistics into the value stored under the
/// `sql/stats/` key prefix.
pub fn encode_stats(stats: &TableStats) -> Result<Vec<u8>, SqlError> {
    if stats.columns.len() > MAX_COLUMNS {
        return Err(SqlError::Unsupported(format!(
            "statistics have more than {MAX_COLUMNS} columns"
        )));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&STATS_MAGIC);
    out.push(STATS_VERSION);
    put_u64(&mut out, stats.rows);
    put_u32(&mut out, stats.columns.len())?;
    for column in &stats.columns {
        put_text(&mut out, &column.name)?;
        put_u64(&mut out, column.nulls);
        put_u64(&mut out, column.distinct);
    }
    Ok(out)
}

fn encode_value(
    out: &mut Vec<u8>,
    value: &Value,
    data_type: DataType,
    table: &str,
    column: &str,
) -> Result<(), SqlError> {
    match value {
        Value::Null => {
            out.push(0);
            Ok(())
        }
        Value::Int(value) if data_type == DataType::Int => {
            out.push(1);
            put_i64(out, *value);
            Ok(())
        }
        Value::Text(value) if data_type == DataType::Text => {
            out.push(2);
            put_text(out, value)
        }
        Value::Bool(value) if matches!(data_type, DataType::Bool | DataType::Logic) => {
            out.push(3);
            out.push(u8::from(*value));
            Ok(())
        }
        Value::Logic(value) if data_type == DataType::Belnap => {
            out.push(4);
            out.push(value.code());
            Ok(())
        }
        Value::Lukasiewicz(value) if data_type == DataType::Lukasiewicz => {
            out.push(5);
            out.extend_from_slice(&value.to_le_bytes());
            Ok(())
        }
        Value::Nat(value) if data_type == DataType::Nat => {
            out.push(6);
            put_u64(out, *value);
            Ok(())
        }
        other => Err(SqlError::CorruptTable {
            table: table.to_string(),
            reason: format!(
                "column {column} expects {}, got {}",
                data_type.name(),
                other.type_name()
            ),
        }),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn u8(&mut self) -> Option<u8> {
        let value = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(value)
    }

    fn u32(&mut self) -> Option<u32> {
        let bytes = self.bytes.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        let bytes = self.bytes.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn text(&mut self, len: usize) -> Option<String> {
        let bytes = self.bytes.get(self.pos..self.pos + len)?;
        self.pos += len;
        std::str::from_utf8(bytes).ok().map(str::to_string)
    }

    fn i64(&mut self) -> Option<i64> {
        let bytes = self.bytes.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(i64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
}

/// Decodes a table value, checking that it belongs to `name`.
fn value_matches(value: &Value, data_type: DataType) -> bool {
    matches!(
        (data_type, value),
        (_, Value::Null)
            | (DataType::Int, Value::Int(_))
            | (DataType::Text, Value::Text(_))
            | (DataType::Bool, Value::Bool(_))
            | (DataType::Logic, Value::Bool(_))
            | (DataType::Belnap, Value::Logic(_))
            | (DataType::Lukasiewicz, Value::Lukasiewicz(_))
            | (DataType::Nat, Value::Nat(_))
    )
}

pub fn decode_table(name: &str, bytes: &[u8]) -> Result<Table, SqlError> {
    let corrupt = |reason: &str| SqlError::CorruptTable {
        table: name.to_string(),
        reason: reason.to_string(),
    };
    let mut reader = Reader::new(bytes);
    if reader.bytes.get(0..4) != Some(TABLE_MAGIC.as_slice()) {
        return Err(corrupt("bad magic"));
    }
    reader.pos = 4;
    let version = reader.u8().ok_or_else(|| corrupt("missing version"))?;
    if version > TABLE_VERSION {
        return Err(corrupt("unsupported version"));
    }
    let column_count = reader
        .u32()
        .ok_or_else(|| corrupt("missing column count"))? as usize;
    if column_count == 0 || column_count > MAX_COLUMNS {
        return Err(corrupt("invalid column count"));
    }
    let mut columns = Vec::with_capacity(column_count);
    for _ in 0..column_count {
        let type_tag = reader.u8().ok_or_else(|| corrupt("missing column type"))?;
        let data_type = type_from_tag(type_tag).ok_or_else(|| corrupt("invalid column type"))?;
        let not_null = reader.u8().ok_or_else(|| corrupt("missing null flag"))? != 0;
        let unique = if version >= 1 {
            reader.u8().ok_or_else(|| corrupt("missing unique flag"))? != 0
        } else {
            false
        };
        let check = if version >= 3 {
            let len = reader
                .u32()
                .ok_or_else(|| corrupt("missing check length"))? as usize;
            let text = reader
                .text(len)
                .ok_or_else(|| corrupt("invalid check text"))?;
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        } else {
            None
        };
        let name_len = reader
            .u32()
            .ok_or_else(|| corrupt("missing column name length"))? as usize;
        let column_name = reader
            .text(name_len)
            .ok_or_else(|| corrupt("invalid column name"))?;
        columns.push(Column {
            name: column_name,
            data_type,
            not_null,
            unique,
            check,
        });
    }
    let unique_sets = if version >= 2 {
        let set_count = reader
            .u32()
            .ok_or_else(|| corrupt("missing unique set count"))? as usize;
        if set_count > MAX_COLUMNS {
            return Err(corrupt("too many unique sets"));
        }
        let mut sets = Vec::with_capacity(set_count);
        for _ in 0..set_count {
            let count = reader
                .u32()
                .ok_or_else(|| corrupt("missing unique column count"))?
                as usize;
            if count > MAX_COLUMNS {
                return Err(corrupt("too many unique columns"));
            }
            let mut indices = Vec::with_capacity(count);
            for _ in 0..count {
                let index = reader
                    .u32()
                    .ok_or_else(|| corrupt("missing unique column index"))?
                    as usize;
                if index >= column_count {
                    return Err(corrupt("unique column index out of range"));
                }
                indices.push(index);
            }
            sets.push(indices);
        }
        sets
    } else {
        Vec::new()
    };
    let checks = if version >= 3 {
        let count = reader.u32().ok_or_else(|| corrupt("missing check count"))? as usize;
        if count > MAX_COLUMNS {
            return Err(corrupt("too many checks"));
        }
        let mut checks = Vec::with_capacity(count);
        for _ in 0..count {
            let len = reader
                .u32()
                .ok_or_else(|| corrupt("missing check length"))? as usize;
            checks.push(
                reader
                    .text(len)
                    .ok_or_else(|| corrupt("invalid check text"))?,
            );
        }
        checks
    } else {
        Vec::new()
    };
    let row_count = reader.u64().ok_or_else(|| corrupt("missing row count"))?;
    if row_count > MAX_ROWS {
        return Err(corrupt("too many rows"));
    }
    let mut rows = Vec::new();
    for _ in 0..row_count {
        let mut row = Vec::with_capacity(column_count);
        for column in &columns {
            if version >= 5 && is_packed(column.data_type) {
                // v5: the value comes from the packed segment below.
                row.push(Value::Null);
                continue;
            }
            let tag = reader.u8().ok_or_else(|| corrupt("missing value tag"))?;
            let value = match tag {
                0 => Value::Null,
                1 => Value::Int(reader.i64().ok_or_else(|| corrupt("truncated int"))?),
                2 => {
                    let len = reader.u32().ok_or_else(|| corrupt("missing text length"))? as usize;
                    Value::Text(reader.text(len).ok_or_else(|| corrupt("invalid text"))?)
                }
                3 => Value::Bool(reader.u8().ok_or_else(|| corrupt("missing bool"))? != 0),
                4 => {
                    let code = reader.u8().ok_or_else(|| corrupt("missing belnap code"))?;
                    Value::Logic(
                        tritdb_exec::ann::B4::from_code(code)
                            .ok_or_else(|| corrupt("invalid belnap code"))?,
                    )
                }
                5 => {
                    let low = reader
                        .u8()
                        .ok_or_else(|| corrupt("missing lukasiewicz value"))?;
                    let high = reader
                        .u8()
                        .ok_or_else(|| corrupt("missing lukasiewicz value"))?;
                    Value::Lukasiewicz(u16::from_le_bytes([low, high]))
                }
                6 => Value::Nat(reader.u64().ok_or_else(|| corrupt("missing nat value"))?),
                _ => return Err(corrupt("invalid value tag")),
            };
            if !value_matches(&value, column.data_type) {
                return Err(corrupt("value type does not match column"));
            }
            if value.is_null() && column.not_null {
                return Err(corrupt("NULL in NOT NULL column"));
            }
            row.push(value);
        }
        rows.push(row);
    }
    if version >= 4 {
        let magic = reader
            .bytes
            .get(reader.pos..reader.pos + 4)
            .ok_or_else(|| corrupt("missing K3 segment magic"))?;
        if magic != PACKED_MAGIC.as_slice() {
            return Err(corrupt("bad K3 segment magic"));
        }
        reader.pos += 4;
        let segment_version = reader
            .u8()
            .ok_or_else(|| corrupt("missing K3 segment version"))?;
        if segment_version == 0 || segment_version > PACKED_VERSION {
            return Err(corrupt("unsupported K3 segment version"));
        }
        let segment_count = reader
            .u32()
            .ok_or_else(|| corrupt("missing K3 segment count"))?
            as usize;
        for _ in 0..segment_count {
            let column_index = reader
                .u32()
                .ok_or_else(|| corrupt("missing K3 column index"))?
                as usize;
            let kind = if segment_version >= 2 {
                reader
                    .u8()
                    .ok_or_else(|| corrupt("missing K3 column kind"))?
            } else {
                0
            };
            let row_count = reader
                .u32()
                .ok_or_else(|| corrupt("missing K3 row count"))?
                as usize;
            let (segment, nulls) = if segment_version >= 3 {
                let chunk_count = reader
                    .u32()
                    .ok_or_else(|| corrupt("missing K3 chunk count"))?
                    as usize;
                let mut bytes = Vec::new();
                for _ in 0..chunk_count {
                    let len = reader
                        .u32()
                        .ok_or_else(|| corrupt("missing K3 chunk length"))?
                        as usize;
                    let chunk = reader
                        .bytes
                        .get(reader.pos..reader.pos + len)
                        .ok_or_else(|| corrupt("truncated K3 chunk"))?;
                    reader.pos += len;
                    bytes.extend_from_slice(chunk);
                }
                let null_len = reader
                    .u32()
                    .ok_or_else(|| corrupt("missing K3 null length"))?
                    as usize;
                let nulls = reader
                    .bytes
                    .get(reader.pos..reader.pos + null_len)
                    .ok_or_else(|| corrupt("truncated K3 null bitmap"))?
                    .to_vec();
                reader.pos += null_len;
                (bytes, nulls)
            } else {
                let packed_len = reader
                    .u32()
                    .ok_or_else(|| corrupt("missing K3 packed length"))?
                    as usize;
                let null_len = if segment_version >= 2 {
                    reader
                        .u32()
                        .ok_or_else(|| corrupt("missing K3 null length"))?
                        as usize
                } else {
                    0
                };
                let segment = reader
                    .bytes
                    .get(reader.pos..reader.pos + packed_len)
                    .ok_or_else(|| corrupt("truncated K3 segment"))?
                    .to_vec();
                reader.pos += packed_len;
                let nulls = reader
                    .bytes
                    .get(reader.pos..reader.pos + null_len)
                    .ok_or_else(|| corrupt("truncated K3 null bitmap"))?
                    .to_vec();
                reader.pos += null_len;
                (segment, nulls)
            };
            let column = columns
                .get(column_index)
                .ok_or_else(|| corrupt("K3 segment column index out of range"))?;
            if row_count != rows.len() {
                return Err(corrupt("K3 segment row count does not match the table"));
            }
            if segment_version >= 2 {
                let values = match kind {
                    0 if column.data_type == DataType::Logic => {
                        K3Column::from_parts(&segment, row_count)
                            .ok_or_else(|| corrupt("K3 segment column"))?
                            .to_values()
                    }
                    1 if column.data_type == DataType::Belnap => if segment_version >= 3 {
                        B4Column::from_parts(segment, nulls, row_count)
                    } else {
                        B4Column::from_parts_2trit(segment, nulls, row_count)
                    }
                    .ok_or_else(|| corrupt("Belnap segment column"))?
                    .to_values(),
                    2 if column.data_type == DataType::Lukasiewicz => {
                        LukasiewiczColumn::from_parts(segment, nulls, row_count)
                            .ok_or_else(|| corrupt("Lukasiewicz segment column"))?
                            .to_values()
                    }
                    3 if column.data_type == DataType::Nat => {
                        NatColumn::from_parts(segment, nulls, row_count)
                            .ok_or_else(|| corrupt("Nat segment column"))?
                            .to_values()
                    }
                    _ => return Err(corrupt("K3 segment kind does not match the column type")),
                };
                if column.not_null && values.iter().any(|value| value.is_null()) {
                    return Err(corrupt("NULL in NOT NULL column"));
                }
                for (row, value) in rows.iter_mut().zip(values) {
                    row[column_index] = value;
                }
            } else {
                if column.data_type != DataType::Logic {
                    return Err(corrupt("K3 segment on a non-LOGIC K3 column"));
                }
                let expected = K3Column::from_rows(&rows, column_index)
                    .ok_or_else(|| corrupt("K3 segment column"))?;
                if expected.packed().bytes() != segment.as_slice() {
                    return Err(corrupt("K3 segment does not match the row data"));
                }
            }
        }
    }
    if reader.pos != bytes.len() {
        return Err(corrupt("trailing bytes"));
    }
    let mut schema = Schema::new(columns);
    schema.unique_sets = unique_sets;
    schema.checks = checks;
    Ok(Table {
        name: name.to_string(),
        schema,
        rows,
    })
}

/// Decodes table statistics, checking that they still match the table schema
/// and row count.
pub fn decode_stats(table: &Table, bytes: &[u8]) -> Result<TableStats, SqlError> {
    let corrupt = |reason: &str| SqlError::CorruptStats {
        table: table.name.clone(),
        reason: reason.to_string(),
    };
    let mut reader = Reader::new(bytes);
    if reader.bytes.get(0..4) != Some(STATS_MAGIC.as_slice()) {
        return Err(corrupt("bad magic"));
    }
    reader.pos = 4;
    let version = reader.u8().ok_or_else(|| corrupt("missing version"))?;
    if version != STATS_VERSION {
        return Err(corrupt("unsupported version"));
    }
    let rows = reader.u64().ok_or_else(|| corrupt("missing row count"))?;
    let column_count = reader
        .u32()
        .ok_or_else(|| corrupt("missing column count"))? as usize;
    if column_count == 0 || column_count > MAX_COLUMNS {
        return Err(corrupt("invalid column count"));
    }
    if column_count != table.schema.len() {
        return Err(corrupt("column count does not match table"));
    }
    let mut columns = Vec::with_capacity(column_count);
    for schema_column in &table.schema.columns {
        let name_len = reader
            .u32()
            .ok_or_else(|| corrupt("missing column name length"))? as usize;
        let name = reader
            .text(name_len)
            .ok_or_else(|| corrupt("invalid column name"))?;
        if name != schema_column.name {
            return Err(corrupt("column name does not match table"));
        }
        let nulls = reader.u64().ok_or_else(|| corrupt("missing null count"))?;
        let distinct = reader
            .u64()
            .ok_or_else(|| corrupt("missing distinct count"))?;
        if nulls > rows {
            return Err(corrupt("null count exceeds row count"));
        }
        if distinct > rows - nulls {
            return Err(corrupt("distinct count exceeds non-null row count"));
        }
        columns.push(ColumnStats {
            name,
            nulls,
            distinct,
        });
    }
    if reader.pos != bytes.len() {
        return Err(corrupt("trailing bytes"));
    }
    if rows != table.rows.len() as u64 {
        return Err(corrupt("row count does not match table"));
    }
    Ok(TableStats { rows, columns })
}

#[cfg(test)]
mod tests {
    use crate::stats::analyze_table;

    use super::*;

    #[test]
    fn encode_rejects_more_rows_than_decode_accepts() {
        // Writing past MAX_ROWS used to succeed and produce a table value that
        // decode_table then rejected, making the table unreadable.
        let table = Table {
            name: "too_many".to_string(),
            schema: Schema::new(vec![Column::new("x", DataType::Int)]),
            rows: (0..MAX_ROWS as usize + 1)
                .map(|index| vec![Value::Int(index as i64)])
                .collect(),
        };
        let error = encode_table(&table).expect_err("the row cap must reject this table");
        assert!(
            matches!(error, SqlError::Unsupported(ref message) if message.contains("rows")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn table_v6_round_trips_k3_packed_segments() {
        let table = Table {
            name: "k3".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("flag", DataType::Logic),
            ]),
            rows: vec![
                vec![Value::Int(1), Value::Bool(true)],
                vec![Value::Int(2), Value::Bool(false)],
                vec![Value::Int(3), Value::Null],
            ],
        };
        let bytes = encode_table(&table).expect("encode");
        assert!(
            bytes.windows(4).any(|window| window == b"K3PK"),
            "v4 must carry a packed segment"
        );
        let decoded = decode_table("k3", &bytes).expect("decode");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v4_without_k3_columns_still_decodes() {
        let table = Table {
            name: "plain".to_string(),
            schema: Schema::new(vec![Column::new("id", DataType::Int)]),
            rows: vec![vec![Value::Int(7)]],
        };
        let bytes = encode_table(&table).expect("encode");
        assert!(bytes.windows(4).any(|window| window == b"K3PK"));
        let decoded = decode_table("plain", &bytes).expect("decode");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v6_round_trips_belnap_segments() {
        let table = Table {
            name: "ann".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("flag", DataType::Logic),
                Column::new("mood", DataType::Belnap),
            ]),
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Bool(true),
                    Value::Logic(tritdb_exec::ann::B4::Both),
                ],
                vec![
                    Value::Int(2),
                    Value::Bool(false),
                    Value::Logic(tritdb_exec::ann::B4::Neither),
                ],
                vec![Value::Int(3), Value::Null, Value::Null],
            ],
        };
        let bytes = encode_table(&table).expect("encode");
        let decoded = decode_table("ann", &bytes).expect("decode");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v6_is_smaller_than_v5() {
        let rows: Vec<Vec<Value>> = (0..100)
            .map(|index| {
                vec![
                    Value::Int(index),
                    Value::Bool(index % 2 == 0),
                    Value::Lukasiewicz((index % 1001) as u16),
                    Value::Nat(index as u64),
                ]
            })
            .collect();
        let table = Table {
            name: "sized".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("flag", DataType::Logic),
                Column::new("p", DataType::Lukasiewicz),
                Column::new("n", DataType::Nat),
            ]),
            rows,
        };
        let v6 = encode_table_versioned(&table, 6).expect("v6");
        let v5 = encode_table_versioned(&table, 5).expect("v5");
        assert!(v6.len() < v5.len(), "v6={} v5={}", v6.len(), v5.len());
    }

    #[test]
    fn table_v6_round_trips_lukasiewicz_and_nat_segments() {
        let table = Table {
            name: "packed".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("p", DataType::Lukasiewicz),
                Column::new("n", DataType::Nat),
            ]),
            rows: vec![
                vec![Value::Int(1), Value::Lukasiewicz(0), Value::Nat(0)],
                vec![
                    Value::Int(2),
                    Value::Lukasiewicz(1000),
                    Value::Nat(u64::MAX),
                ],
                vec![Value::Int(3), Value::Null, Value::Null],
            ],
        };
        let bytes = encode_table(&table).expect("encode");
        let decoded = decode_table("packed", &bytes).expect("decode");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v6_chunks_long_columns() {
        let rows: Vec<Vec<Value>> = (0..10_000)
            .map(|index| vec![Value::Nat(index as u64)])
            .collect();
        let table = Table {
            name: "chunked".to_string(),
            schema: Schema::new(vec![Column::new("n", DataType::Nat)]),
            rows,
        };
        // 10_000 values * 8 bytes = 80_000 bytes > PACKED_CHUNK, so the
        // segment is split into several chunks.
        let bytes = encode_table(&table).expect("encode");
        let decoded = decode_table("chunked", &bytes).expect("decode");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v5_data_still_decodes() {
        let table = Table {
            name: "legacy".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("flag", DataType::Logic),
                Column::new("mood", DataType::Belnap),
            ]),
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Bool(true),
                    Value::Logic(tritdb_exec::ann::B4::Both),
                ],
                vec![Value::Int(2), Value::Null, Value::Null],
            ],
        };
        let bytes = encode_table_versioned(&table, 5).expect("v5");
        let decoded = decode_table("legacy", &bytes).expect("decode v5");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v4_data_still_decodes() {
        let table = Table {
            name: "k3".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column::new("flag", DataType::Logic),
            ]),
            rows: vec![
                vec![Value::Int(1), Value::Bool(true)],
                vec![Value::Int(2), Value::Null],
            ],
        };
        let bytes = encode_table_versioned(&table, 4).expect("v4");
        let decoded = decode_table("k3", &bytes).expect("decode v4");
        assert_eq!(decoded.rows, table.rows);
    }

    #[test]
    fn table_v4_rejects_a_tampered_k3_segment() {
        let table = Table {
            name: "k3".to_string(),
            schema: Schema::new(vec![Column::new("flag", DataType::Logic)]),
            rows: vec![vec![Value::Bool(true)], vec![Value::Bool(false)]],
        };
        let mut bytes = encode_table(&table).expect("encode");
        let segment = bytes
            .windows(4)
            .position(|window| window == b"K3PK")
            .expect("segment");
        // magic(4) + version(1) + count(4) + column(4) + kind(1) = row_count offset
        let row_count = segment + 4 + 1 + 4 + 4 + 1;
        bytes[row_count] = 3;
        assert!(matches!(
            decode_table("k3", &bytes),
            Err(SqlError::CorruptTable { .. })
        ));
    }

    #[test]
    fn table_v3_without_segments_still_decodes() {
        let table = Table {
            name: "k3".to_string(),
            schema: Schema::new(vec![Column::new("flag", DataType::Logic)]),
            rows: vec![vec![Value::Bool(true)], vec![Value::Null]],
        };
        let bytes = encode_table_versioned(&table, 3).expect("encode v3");
        assert!(!bytes.windows(4).any(|window| window == b"K3PK"));
        let decoded = decode_table("k3", &bytes).expect("v3 fallback");
        assert_eq!(decoded.rows, table.rows);
    }

    fn table() -> Table {
        Table {
            name: "t".to_string(),
            schema: Schema::new(vec![
                Column::new("id", DataType::Int),
                Column {
                    name: "name".to_string(),
                    data_type: DataType::Text,
                    not_null: true,
                    unique: true,
                    check: Some("name IS NOT NULL".to_string()),
                },
                Column::new("ok", DataType::Bool),
            ]),
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Text("a".to_string()),
                    Value::Bool(true),
                ],
                vec![Value::Null, Value::Text("b".to_string()), Value::Null],
            ],
        }
    }

    #[test]
    fn table_roundtrip() {
        let table = table();
        let bytes = encode_table(&table).expect("encode");
        let decoded = decode_table("t", &bytes).expect("decode");
        assert_eq!(decoded, table);
    }

    #[test]
    fn rejects_truncated_table() {
        let bytes = encode_table(&table()).expect("encode");
        for end in 0..bytes.len() {
            assert!(decode_table("t", &bytes[..end]).is_err(), "prefix {end}");
        }
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = encode_table(&table()).expect("encode");
        bytes.push(0);
        assert!(decode_table("t", &bytes).is_err());
    }

    #[test]
    fn rejects_type_mismatch() {
        let mut table = table();
        table.rows[0][0] = Value::Text("x".to_string());
        assert!(encode_table(&table).is_err());
    }

    #[test]
    fn stats_roundtrip_and_validate_table() {
        let table = table();
        let stats = analyze_table(&table);
        let bytes = encode_stats(&stats).expect("encode stats");
        let decoded = decode_stats(&table, &bytes).expect("decode stats");
        assert_eq!(decoded, stats);

        for end in 0..bytes.len() {
            assert!(
                decode_stats(&table, &bytes[..end]).is_err(),
                "stats prefix {end}"
            );
        }

        let mut shorter = table;
        shorter.rows.pop();
        assert!(decode_stats(&shorter, &bytes).is_err());
    }
}
