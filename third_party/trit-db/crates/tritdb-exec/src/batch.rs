//! Batch (vectorized) three-valued predicates and as-of joins.
//!
//! These are the M10 primitives that connect [`tritdb_tritvec`] to the
//! executor. The row-at-a-time `eval` path is unchanged: the batch path is
//! opt-in and is validated against the scalar truth tables (predicates) and a
//! brute-force scan (as-of joins).

use std::fmt;

use crate::ann::B4;
use crate::{Row, Value};
use tritdb_tritvec::{pack, unpack, Radix, Trit, TritVecError};
pub use tritdb_tritvec::{TritVec, K3};

/// Errors from batch evaluation and as-of joins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchError {
    /// A predicate atom refers to a column that was not supplied.
    AtomOutOfRange {
        /// Requested atom index.
        index: usize,
        /// Number of supplied atom columns.
        atoms: usize,
    },
    /// Two atom vectors have different lengths.
    LengthMismatch {
        /// Left length.
        expected: usize,
        /// Right length.
        actual: usize,
    },
    /// An `AND`/`OR` term list was empty.
    EmptyTerms,
    /// The right side of an as-of join is not sorted by time.
    UnorderedRight {
        /// Index of the out-of-order row.
        index: usize,
        /// Previous timestamp.
        previous: i64,
        /// Current timestamp.
        current: i64,
    },
    /// A vector operation failed for another reason.
    Vector(TritVecError),
}

impl fmt::Display for BatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchError::AtomOutOfRange { index, atoms } => {
                write!(
                    formatter,
                    "predicate atom {index} is outside {atoms} columns"
                )
            }
            BatchError::LengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "batch length mismatch: expected {expected}, got {actual}"
                )
            }
            BatchError::EmptyTerms => formatter.write_str("AND/OR needs at least one term"),
            BatchError::UnorderedRight {
                index,
                previous,
                current,
            } => write!(
                formatter,
                "as-of right side is not sorted at row {index}: {previous} then {current}"
            ),
            BatchError::Vector(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for BatchError {}

/// A three-valued predicate over atom columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriPredicate {
    /// Column `index` of the atom set.
    Atom(usize),
    /// `NOT`.
    Not(Box<TriPredicate>),
    /// `AND` (an empty list is rejected).
    And(Vec<TriPredicate>),
    /// `OR` (an empty list is rejected).
    Or(Vec<TriPredicate>),
}

/// Evaluates `predicate` element-wise over the atom columns.
pub fn eval_predicate(predicate: &TriPredicate, atoms: &[TritVec]) -> Result<TritVec, BatchError> {
    match predicate {
        TriPredicate::Atom(index) => atoms
            .get(*index)
            .cloned()
            .ok_or(BatchError::AtomOutOfRange {
                index: *index,
                atoms: atoms.len(),
            }),
        TriPredicate::Not(inner) => Ok(eval_predicate(inner, atoms)?.neg()),
        TriPredicate::And(terms) => {
            let mut terms = terms.iter();
            let Some(first) = terms.next() else {
                return Err(BatchError::EmptyTerms);
            };
            let mut accumulator = eval_predicate(first, atoms)?;
            for term in terms {
                let right = eval_predicate(term, atoms)?;
                accumulator = accumulator.meet(&right).map_err(map_vector_error)?;
            }
            Ok(accumulator)
        }
        TriPredicate::Or(terms) => {
            let mut terms = terms.iter();
            let Some(first) = terms.next() else {
                return Err(BatchError::EmptyTerms);
            };
            let mut accumulator = eval_predicate(first, atoms)?;
            for term in terms {
                let right = eval_predicate(term, atoms)?;
                accumulator = accumulator.join(&right).map_err(map_vector_error)?;
            }
            Ok(accumulator)
        }
    }
}

/// A packed K3 column materialized from row values.
///
/// This is the optional vectorized read path for `LOGIC K3` / `LOGIC BELNAP`
/// columns: storage stays row-based, but a scan can materialize one column as
/// `r3`-packed trits and feed it to [`eval_predicate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct K3Column {
    packed: TritVec,
}

impl K3Column {
    /// Materializes column `index` from `rows`.
    ///
    /// Returns `None` when the column is missing from any row; an empty row
    /// set materializes as an empty column.
    pub fn from_rows(rows: &[Row], index: usize) -> Option<Self> {
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            values.push(k3_of_value(row.get(index)?));
        }
        Some(Self {
            packed: TritVec::new(&values),
        })
    }

    /// Number of rows.
    pub const fn len(&self) -> usize {
        self.packed.len()
    }

    /// True when the column holds no rows.
    pub const fn is_empty(&self) -> bool {
        self.packed.is_empty()
    }

    /// One element.
    pub fn get(&self, row: usize) -> Option<K3> {
        self.packed.get(row)
    }

    /// The packed representation.
    pub const fn packed(&self) -> &TritVec {
        &self.packed
    }

    /// Rebuilds a column from packed `r3` bytes, validating the length.
    pub fn from_parts(packed: &[u8], len: usize) -> Option<Self> {
        if packed.len() != Radix::R3.packed_len(len) {
            return None;
        }
        let trits = unpack(Radix::R3, packed, len).ok()?;
        let values: Vec<K3> = trits.into_iter().map(K3::from_trit).collect();
        Some(Self {
            packed: TritVec::new(&values),
        })
    }

    /// Row values (`Value::Bool` / `Value::Null`) for storage round-trips.
    pub fn to_values(&self) -> Vec<Value> {
        self.packed
            .to_vec()
            .into_iter()
            .map(|value| match value {
                K3::True => Value::Bool(true),
                K3::False => Value::Bool(false),
                K3::Unknown => Value::Null,
            })
            .collect()
    }

    /// Rows whose value is `TRUE` (SQL `WHERE` keeps only `TRUE`).
    pub fn filter_true(&self) -> Vec<usize> {
        filter_indices(&self.packed)
    }
}

/// Maps a value to K3: `BOTH`, `NEITHER` and `NULL` are `UNKNOWN`.
pub fn k3_of_value(value: &Value) -> K3 {
    match value {
        Value::Bool(true) | Value::Logic(B4::True) => K3::True,
        Value::Bool(false) | Value::Logic(B4::False) => K3::False,
        _ => K3::Unknown,
    }
}

/// A packed Belnap column: two bits per value plus a NULL bitmap.
///
/// `BOTH`/`NEITHER` need four states; two bits per value pack four values into
/// one byte. The NULL bitmap is one bit per row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct B4Column {
    packed: Vec<u8>,
    nulls: Vec<u8>,
    len: usize,
}

impl B4Column {
    /// Materializes column `index` from `rows`.
    pub fn from_rows(rows: &[Row], index: usize) -> Option<Self> {
        let mut packed = vec![0u8; rows.len().div_ceil(4)];
        let mut nulls = vec![0u8; rows.len().div_ceil(8)];
        for (row_index, row) in rows.iter().enumerate() {
            let value = row.get(index)?;
            let code = match value {
                Value::Null => {
                    nulls[row_index / 8] |= 1 << (row_index % 8);
                    0
                }
                Value::Logic(four) => four.code(),
                Value::Bool(true) => B4::True.code(),
                Value::Bool(false) => B4::False.code(),
                _ => return None,
            };
            packed[row_index / 4] |= code << (2 * (row_index % 4));
        }
        Some(Self {
            packed,
            nulls,
            len: rows.len(),
        })
    }

    /// Rebuilds a column from stored parts, validating their lengths.
    pub fn from_parts(packed: Vec<u8>, nulls: Vec<u8>, len: usize) -> Option<Self> {
        if packed.len() != len.div_ceil(4) || nulls.len() != len.div_ceil(8) {
            return None;
        }
        Some(Self { packed, nulls, len })
    }

    /// Encodes this column in the v5 layout (two trits per value, `r3`).
    pub fn to_parts_2trit(&self) -> (Vec<u8>, Vec<u8>) {
        let mut trits = Vec::with_capacity(self.len * 2);
        for row in 0..self.len {
            let code = (self.packed[row / 4] >> (2 * (row % 4))) & 0b11;
            trits.push(Trit::from_u8(code & 1).expect("0/1"));
            trits.push(Trit::from_u8((code >> 1) & 1).expect("0/1"));
        }
        let packed = pack(Radix::R3, &trits).expect("trits pack");
        (packed, self.nulls.clone())
    }

    /// Decodes the v5 layout (two trits per value, `r3`) and re-packs as 2-bit.
    pub fn from_parts_2trit(packed: Vec<u8>, nulls: Vec<u8>, len: usize) -> Option<Self> {
        if packed.len() != Radix::R3.packed_len(len * 2) || nulls.len() != len.div_ceil(8) {
            return None;
        }
        let trits = unpack(Radix::R3, &packed, len * 2).ok()?;
        let mut repacked = vec![0u8; len.div_ceil(4)];
        for row in 0..len {
            let low = trits[row * 2].get() & 1;
            let high = trits[row * 2 + 1].get() & 1;
            let code = low | (high << 1);
            repacked[row / 4] |= code << (2 * (row % 4));
        }
        Some(Self {
            packed: repacked,
            nulls,
            len,
        })
    }

    /// Number of rows.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the column holds no rows.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True when `row` is `NULL`.
    pub fn is_null(&self, row: usize) -> bool {
        row < self.len && (self.nulls[row / 8] >> (row % 8)) & 1 == 1
    }

    /// One element; `None` for `NULL` or an out-of-range row.
    pub fn get(&self, row: usize) -> Option<B4> {
        if row >= self.len || self.is_null(row) {
            return None;
        }
        B4::from_code((self.packed[row / 4] >> (2 * (row % 4))) & 0b11)
    }

    /// Packed values (four per byte).
    pub fn packed(&self) -> &[u8] {
        &self.packed
    }

    /// NULL bitmap (one bit per row).
    pub fn nulls(&self) -> &[u8] {
        &self.nulls
    }

    /// Rebuilds the row values (`Value::Logic` / `Value::Null`).
    pub fn to_values(&self) -> Vec<Value> {
        (0..self.len)
            .map(|row| {
                if self.is_null(row) {
                    return Value::Null;
                }
                B4::from_code((self.packed[row / 4] >> (2 * (row % 4))) & 0b11)
                    .map(Value::Logic)
                    .unwrap_or(Value::Null)
            })
            .collect()
    }
}

/// A packed Łukasiewicz column: 10 bits per value plus a NULL bitmap.
///
/// Values are thousandths (`0..=1000`), so ten bits are enough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LukasiewiczColumn {
    packed: Vec<u8>,
    nulls: Vec<u8>,
    len: usize,
}

impl LukasiewiczColumn {
    /// Materializes column `index` from `rows`.
    pub fn from_rows(rows: &[Row], index: usize) -> Option<Self> {
        let mut packed = vec![0u8; (rows.len() * 10).div_ceil(8)];
        let mut nulls = vec![0u8; rows.len().div_ceil(8)];
        for (row_index, row) in rows.iter().enumerate() {
            match row.get(index)? {
                Value::Null => nulls[row_index / 8] |= 1 << (row_index % 8),
                Value::Lukasiewicz(value) if *value <= 1000 => {
                    write_bits(&mut packed, row_index * 10, 10, u32::from(*value));
                }
                _ => return None,
            }
        }
        Some(Self {
            packed,
            nulls,
            len: rows.len(),
        })
    }

    /// Rebuilds a column from stored parts, validating their lengths.
    pub fn from_parts(packed: Vec<u8>, nulls: Vec<u8>, len: usize) -> Option<Self> {
        if packed.len() != (len * 10).div_ceil(8) || nulls.len() != len.div_ceil(8) {
            return None;
        }
        Some(Self { packed, nulls, len })
    }

    /// Number of rows.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the column holds no rows.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True when `row` is `NULL`.
    pub fn is_null(&self, row: usize) -> bool {
        row < self.len && (self.nulls[row / 8] >> (row % 8)) & 1 == 1
    }

    /// One value in thousandths; `None` for `NULL` or an out-of-range row.
    pub fn get(&self, row: usize) -> Option<u16> {
        if row >= self.len || self.is_null(row) {
            return None;
        }
        Some(read_bits(&self.packed, row * 10, 10) as u16)
    }

    /// Packed bits (ten per value).
    pub fn packed(&self) -> &[u8] {
        &self.packed
    }

    /// NULL bitmap (one bit per row).
    pub fn nulls(&self) -> &[u8] {
        &self.nulls
    }

    /// Rebuilds the row values (`Value::Lukasiewicz` / `Value::Null`).
    pub fn to_values(&self) -> Vec<Value> {
        (0..self.len)
            .map(|row| {
                if self.is_null(row) {
                    Value::Null
                } else {
                    Value::Lukasiewicz(read_bits(&self.packed, row * 10, 10) as u16)
                }
            })
            .collect()
    }
}

/// A packed ℕ column: eight bytes per value plus a NULL bitmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NatColumn {
    packed: Vec<u8>,
    nulls: Vec<u8>,
    len: usize,
}

impl NatColumn {
    /// Materializes column `index` from `rows`.
    pub fn from_rows(rows: &[Row], index: usize) -> Option<Self> {
        let mut packed = vec![0u8; rows.len() * 8];
        let mut nulls = vec![0u8; rows.len().div_ceil(8)];
        for (row_index, row) in rows.iter().enumerate() {
            match row.get(index)? {
                Value::Null => nulls[row_index / 8] |= 1 << (row_index % 8),
                Value::Nat(value) => {
                    packed[row_index * 8..row_index * 8 + 8].copy_from_slice(&value.to_le_bytes());
                }
                _ => return None,
            }
        }
        Some(Self {
            packed,
            nulls,
            len: rows.len(),
        })
    }

    /// Rebuilds a column from stored parts, validating their lengths.
    pub fn from_parts(packed: Vec<u8>, nulls: Vec<u8>, len: usize) -> Option<Self> {
        if packed.len() != len * 8 || nulls.len() != len.div_ceil(8) {
            return None;
        }
        Some(Self { packed, nulls, len })
    }

    /// Number of rows.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the column holds no rows.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True when `row` is `NULL`.
    pub fn is_null(&self, row: usize) -> bool {
        row < self.len && (self.nulls[row / 8] >> (row % 8)) & 1 == 1
    }

    /// One value; `None` for `NULL` or an out-of-range row.
    pub fn get(&self, row: usize) -> Option<u64> {
        if row >= self.len || self.is_null(row) {
            return None;
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.packed[row * 8..row * 8 + 8]);
        Some(u64::from_le_bytes(bytes))
    }

    /// Packed values (eight bytes each).
    pub fn packed(&self) -> &[u8] {
        &self.packed
    }

    /// NULL bitmap (one bit per row).
    pub fn nulls(&self) -> &[u8] {
        &self.nulls
    }

    /// Rebuilds the row values (`Value::Nat` / `Value::Null`).
    pub fn to_values(&self) -> Vec<Value> {
        (0..self.len)
            .map(|row| {
                if self.is_null(row) {
                    Value::Null
                } else {
                    let mut bytes = [0u8; 8];
                    bytes.copy_from_slice(&self.packed[row * 8..row * 8 + 8]);
                    Value::Nat(u64::from_le_bytes(bytes))
                }
            })
            .collect()
    }
}

/// Writes `bits` low bits of `value` at `bit_offset` (little-endian bit order).
fn write_bits(bytes: &mut [u8], bit_offset: usize, bits: u32, value: u32) {
    for index in 0..bits {
        let bit = bit_offset + index as usize;
        if (value >> index) & 1 == 1 {
            bytes[bit / 8] |= 1 << (bit % 8);
        }
    }
}

/// Reads `bits` low bits at `bit_offset` (little-endian bit order).
fn read_bits(bytes: &[u8], bit_offset: usize, bits: u32) -> u32 {
    let mut value = 0u32;
    for index in 0..bits {
        let bit = bit_offset + index as usize;
        let bit_value = (bytes[bit / 8] >> (bit % 8)) & 1;
        value |= u32::from(bit_value) << index;
    }
    value
}

/// Indices where `truth` is `TRUE`.
///
/// This is SQL `WHERE` semantics: `FALSE` and `UNKNOWN` rows are dropped.
pub fn filter_indices(truth: &TritVec) -> Vec<usize> {
    truth
        .to_vec()
        .into_iter()
        .enumerate()
        .filter(|(_, value)| *value == K3::True)
        .map(|(index, _)| index)
        .collect()
}

/// For each left timestamp, the index of the last right row with `right <= left`.
///
/// `right` must be non-decreasing; the result has one entry per left row.
/// Equal timestamps resolve to the last matching right row (q's `aj`).
pub fn asof_join(left: &[i64], right: &[i64]) -> Result<Vec<Option<usize>>, BatchError> {
    for index in 1..right.len() {
        if right[index] < right[index - 1] {
            return Err(BatchError::UnorderedRight {
                index,
                previous: right[index - 1],
                current: right[index],
            });
        }
    }
    let mut out = Vec::with_capacity(left.len());
    for timestamp in left {
        let count = right.partition_point(|value| *value <= *timestamp);
        out.push(if count == 0 { None } else { Some(count - 1) });
    }
    Ok(out)
}

fn map_vector_error(error: TritVecError) -> BatchError {
    match error {
        TritVecError::LengthMismatch { expected, actual } => {
            BatchError::LengthMismatch { expected, actual }
        }
        other => BatchError::Vector(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tritdb_testkit::Rng;

    fn random_k3(rng: &mut Rng) -> K3 {
        match rng.below(3) {
            0 => K3::False,
            1 => K3::Unknown,
            _ => K3::True,
        }
    }

    fn random_atoms(rng: &mut Rng, rows: usize, atoms: usize) -> Vec<TritVec> {
        (0..atoms)
            .map(|_| {
                let values: Vec<K3> = (0..rows).map(|_| random_k3(rng)).collect();
                TritVec::new(&values)
            })
            .collect()
    }

    fn random_predicate(rng: &mut Rng, atoms: usize, depth: usize) -> TriPredicate {
        if depth == 0 || rng.chance(40) {
            return TriPredicate::Atom(rng.below(atoms as u32) as usize);
        }
        match rng.below(3) {
            0 => TriPredicate::Not(Box::new(random_predicate(rng, atoms, depth - 1))),
            1 => TriPredicate::And(vec![
                random_predicate(rng, atoms, depth - 1),
                random_predicate(rng, atoms, depth - 1),
            ]),
            _ => TriPredicate::Or(vec![
                random_predicate(rng, atoms, depth - 1),
                random_predicate(rng, atoms, depth - 1),
            ]),
        }
    }

    fn scalar_eval(predicate: &TriPredicate, row: &[K3]) -> K3 {
        match predicate {
            TriPredicate::Atom(index) => row[*index],
            TriPredicate::Not(inner) => scalar_eval(inner, row).neg(),
            TriPredicate::And(terms) => terms.iter().fold(K3::True, |accumulator, term| {
                accumulator.meet(scalar_eval(term, row))
            }),
            TriPredicate::Or(terms) => terms.iter().fold(K3::False, |accumulator, term| {
                accumulator.join(scalar_eval(term, row))
            }),
        }
    }

    #[test]
    fn batch_predicates_match_the_scalar_truth_tables() {
        let mut rng = Rng::new(0xB47C_0001);
        for _ in 0..300 {
            let rows = (rng.below(60) + 1) as usize;
            let atom_count = (rng.below(3) + 1) as usize;
            let atoms = random_atoms(&mut rng, rows, atom_count);
            let unpacked: Vec<Vec<K3>> = atoms.iter().map(TritVec::to_vec).collect();
            let predicate = random_predicate(&mut rng, atom_count, 3);
            let batch = eval_predicate(&predicate, &atoms)
                .expect("batch evaluation")
                .to_vec();
            assert_eq!(batch.len(), rows);
            for (row_index, actual) in batch.iter().enumerate() {
                let scalar_row: Vec<K3> = unpacked.iter().map(|column| column[row_index]).collect();
                assert_eq!(
                    *actual,
                    scalar_eval(&predicate, &scalar_row),
                    "row {row_index} of {predicate:?}"
                );
            }
        }
    }

    #[test]
    fn filter_keeps_only_true() {
        let truth = TritVec::new(&[K3::True, K3::False, K3::Unknown, K3::True]);
        assert_eq!(filter_indices(&truth), vec![0, 3]);
        assert!(filter_indices(&TritVec::new(&[])).is_empty());
    }

    #[test]
    fn batch_rejects_bad_inputs() {
        let short = TritVec::new(&[K3::True]);
        let long = TritVec::new(&[K3::True, K3::False]);
        assert_eq!(
            eval_predicate(&TriPredicate::Atom(3), std::slice::from_ref(&short)),
            Err(BatchError::AtomOutOfRange { index: 3, atoms: 1 })
        );
        assert_eq!(
            eval_predicate(
                &TriPredicate::And(vec![TriPredicate::Atom(0), TriPredicate::Atom(1)]),
                &[short, long],
            ),
            Err(BatchError::LengthMismatch {
                expected: 1,
                actual: 2
            })
        );
        assert_eq!(
            eval_predicate(&TriPredicate::And(vec![]), &[]),
            Err(BatchError::EmptyTerms)
        );
        assert_eq!(
            eval_predicate(&TriPredicate::Or(vec![]), &[]),
            Err(BatchError::EmptyTerms)
        );
    }

    fn asof_bruteforce(left: &[i64], right: &[i64]) -> Vec<Option<usize>> {
        left.iter()
            .map(|timestamp| {
                right
                    .iter()
                    .enumerate()
                    .filter(|(_, value)| *value <= timestamp)
                    .map(|(index, _)| index)
                    .next_back()
            })
            .collect()
    }

    #[test]
    fn asof_matches_bruteforce() {
        let mut rng = Rng::new(0xA50F_0002);
        for _ in 0..200 {
            let right_len = rng.below(20) as usize;
            let mut right: Vec<i64> = (0..right_len).map(|_| rng.below(100) as i64).collect();
            right.sort_unstable();
            let left: Vec<i64> = (0..rng.below(20) as usize)
                .map(|_| rng.below(100) as i64)
                .collect();
            assert_eq!(
                asof_join(&left, &right).unwrap(),
                asof_bruteforce(&left, &right),
                "left={left:?} right={right:?}"
            );
        }
    }

    #[test]
    fn asof_handles_edges() {
        assert_eq!(asof_join(&[], &[]).unwrap(), Vec::<Option<usize>>::new());
        assert_eq!(asof_join(&[1], &[]).unwrap(), vec![None]);
        assert_eq!(asof_join(&[5], &[1, 3, 5, 9]).unwrap(), vec![Some(2)]);
        // Equal timestamps: the last match wins.
        assert_eq!(asof_join(&[5], &[5, 5, 5]).unwrap(), vec![Some(2)]);
        // Unsorted right side is rejected.
        assert_eq!(
            asof_join(&[1], &[3, 2]),
            Err(BatchError::UnorderedRight {
                index: 1,
                previous: 3,
                current: 2
            })
        );
    }

    #[test]
    fn b4_column_packs_four_values_per_byte() {
        let rows: Vec<Row> = vec![
            vec![Value::Logic(B4::Both)],
            vec![Value::Logic(B4::Neither)],
            vec![Value::Null],
            vec![Value::Logic(B4::True)],
            vec![Value::Logic(B4::False)],
        ];
        let column = B4Column::from_rows(&rows, 0).expect("column");
        assert_eq!(column.packed().len(), 2);
        assert_eq!(column.get(0), Some(B4::Both));
        assert_eq!(column.get(1), Some(B4::Neither));
        assert_eq!(column.get(4), Some(B4::False));

        // v5 legacy layout: two trits per value, derived from the B4 codes.
        let legacy_trits: Vec<Trit> = [B4::Both, B4::True, B4::False, B4::Neither]
            .into_iter()
            .flat_map(|value| {
                let code = value.code();
                [
                    Trit::from_u8(code & 1).expect("0/1"),
                    Trit::from_u8((code >> 1) & 1).expect("0/1"),
                ]
            })
            .collect();
        let legacy_packed = pack(Radix::R3, &legacy_trits).expect("legacy");
        let legacy = B4Column::from_parts_2trit(legacy_packed, vec![0], 4).expect("legacy column");
        assert_eq!(legacy.get(0), Some(B4::Both));
        assert_eq!(legacy.get(1), Some(B4::True));
        assert_eq!(legacy.get(2), Some(B4::False));
        assert_eq!(legacy.get(3), Some(B4::Neither));
    }

    #[test]
    fn lukasiewicz_column_round_trips() {
        let rows: Vec<Row> = vec![
            vec![Value::Lukasiewicz(0)],
            vec![Value::Lukasiewicz(500)],
            vec![Value::Lukasiewicz(1000)],
            vec![Value::Null],
        ];
        let column = LukasiewiczColumn::from_rows(&rows, 0).expect("column");
        assert_eq!(column.len(), 4);
        assert_eq!(column.get(0), Some(0));
        assert_eq!(column.get(1), Some(500));
        assert_eq!(column.get(2), Some(1000));
        assert_eq!(column.get(3), None);
        assert_eq!(
            column.to_values(),
            vec![
                Value::Lukasiewicz(0),
                Value::Lukasiewicz(500),
                Value::Lukasiewicz(1000),
                Value::Null
            ]
        );
        assert_eq!(
            LukasiewiczColumn::from_parts(column.packed().to_vec(), column.nulls().to_vec(), 4)
                .expect("parts"),
            column
        );
    }

    #[test]
    fn nat_column_round_trips() {
        let rows: Vec<Row> = vec![
            vec![Value::Nat(0)],
            vec![Value::Nat(u64::MAX)],
            vec![Value::Null],
            vec![Value::Nat(42)],
        ];
        let column = NatColumn::from_rows(&rows, 0).expect("column");
        assert_eq!(column.get(0), Some(0));
        assert_eq!(column.get(1), Some(u64::MAX));
        assert_eq!(column.get(2), None);
        assert_eq!(column.get(3), Some(42));
        assert_eq!(
            NatColumn::from_parts(column.packed().to_vec(), column.nulls().to_vec(), 4)
                .expect("parts"),
            column
        );
    }

    #[test]
    fn k3_column_parts_round_trip_values() {
        let rows: Vec<Row> = vec![
            vec![Value::Bool(true)],
            vec![Value::Bool(false)],
            vec![Value::Null],
        ];
        let column = K3Column::from_rows(&rows, 0).expect("column");
        let restored = K3Column::from_parts(column.packed().bytes(), 3).expect("parts");
        assert_eq!(restored, column);
        assert_eq!(
            restored.to_values(),
            vec![Value::Bool(true), Value::Bool(false), Value::Null]
        );
        assert!(K3Column::from_parts(&[], 3).is_none());
    }

    #[test]
    fn b4_column_round_trips_values_and_nulls() {
        let rows: Vec<Row> = vec![
            vec![Value::Logic(B4::True)],
            vec![Value::Logic(B4::False)],
            vec![Value::Logic(B4::Both)],
            vec![Value::Logic(B4::Neither)],
            vec![Value::Null],
            vec![Value::Bool(true)],
            vec![Value::Bool(false)],
        ];
        let column = B4Column::from_rows(&rows, 0).expect("column");
        assert_eq!(column.len(), 7);
        assert_eq!(column.get(0), Some(B4::True));
        assert_eq!(column.get(2), Some(B4::Both));
        assert_eq!(column.get(3), Some(B4::Neither));
        assert!(column.is_null(4));
        assert_eq!(column.get(4), None);
        assert_eq!(column.get(5), Some(B4::True));
        let values = column.to_values();
        assert_eq!(values[0], Value::Logic(B4::True));
        assert_eq!(values[4], Value::Null);
        assert_eq!(
            B4Column::from_parts(column.packed().to_vec(), column.nulls().to_vec(), 7)
                .expect("parts"),
            column
        );
        // Truncated parts are rejected.
        assert!(B4Column::from_parts(vec![], vec![], 7).is_none());
    }

    #[test]
    fn k3_column_round_trips_row_values() {
        let rows: Vec<Row> = vec![
            vec![Value::Bool(true)],
            vec![Value::Bool(false)],
            vec![Value::Null],
            vec![Value::Logic(B4::Both)],
            vec![Value::Logic(B4::True)],
        ];
        let column = K3Column::from_rows(&rows, 0).expect("column");
        assert_eq!(column.len(), 5);
        assert_eq!(column.get(0), Some(K3::True));
        assert_eq!(column.get(1), Some(K3::False));
        assert_eq!(column.get(2), Some(K3::Unknown));
        assert_eq!(column.get(3), Some(K3::Unknown));
        assert_eq!(column.get(4), Some(K3::True));
        assert_eq!(column.filter_true(), vec![0, 4]);
        assert_eq!(K3Column::from_rows(&rows, 1), None);
        let empty = K3Column::from_rows(&[], 0).expect("empty column");
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn asof_scales_to_100k_rows() {
        let mut rng = Rng::new(0xA50F_0003);
        let mut right: Vec<i64> = (0..100_000).map(|_| rng.below(1_000_000) as i64).collect();
        right.sort_unstable();
        let left: Vec<i64> = (0..100_000).map(|_| rng.below(1_000_000) as i64).collect();
        let result = asof_join(&left, &right).expect("asof");
        assert_eq!(result.len(), left.len());
        let prefix = 200;
        assert_eq!(
            &result[..prefix],
            &asof_bruteforce(&left[..prefix], &right)[..]
        );
    }
}
