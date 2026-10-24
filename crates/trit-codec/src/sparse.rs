//! 三值稀疏编码（COO 格式）。
//!
//! 三值网络权重里 `Unknown(0)` 占多数时，稠密存整条 `Vec<Ternary>` 浪费空间。
//! COO（coordinate）格式只记录**非 Unknown** 的位置和值：
//!
//! - `(index, value)` 对，`value ∈ {False, True}`；
//! - 未列出的位置一律视为 `Unknown`；
//! - `index` 必须严格递增、落在 `[0, len)` 内、不重复。
//!
//! 压缩比 = `1 - 非零数 / 总长`；全 Unknown 时编码为空列表，解码后是全 Unknown。

use crate::CodecError;
use trit_core::value::Ternary;

/// 一条稀疏记录：`(下标, 值)`。
pub type Entry = (u32, Ternary);

/// 编码：只保留非 Unknown 的位置。
pub fn encode(values: &[Ternary]) -> Vec<Entry> {
    values
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_known())
        .map(|(i, v)| (i as u32, *v))
        .collect()
}

/// 解码：按 `len` 长度还原完整序列，未列出位置填 `Unknown`。
///
/// 校验：下标必须严格递增、不越界、不重复；否则返回结构化错误而不是静默错。
pub fn decode(entries: &[Entry], len: usize) -> Result<Vec<Ternary>, CodecError> {
    let mut out = vec![Ternary::Unknown; len];
    let mut prev: Option<u32> = None;
    for &(index, value) in entries {
        if index as usize >= len {
            return Err(CodecError::SparseIndexOutOfRange { index, len });
        }
        if let Some(p) = prev
            && index <= p
        {
            return Err(CodecError::SparseIndexNotIncreasing {
                prev: p,
                found: index,
            });
        }
        out[index as usize] = value;
        prev = Some(index);
    }
    Ok(out)
}

/// 压缩比：`1 - 非零数 / 总长`；总长为 0 时返回 0。
pub fn sparsity(entries: &[Entry], len: usize) -> f64 {
    if len == 0 {
        0.0
    } else {
        1.0 - entries.len() as f64 / len as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sequence_encodes_to_empty() {
        assert!(encode(&[]).is_empty());
        assert_eq!(decode(&[], 0).unwrap(), Vec::<Ternary>::new());
    }

    #[test]
    fn all_unknown_compresses_to_nothing() {
        let values = vec![Ternary::Unknown; 100];
        let entries = encode(&values);
        assert!(entries.is_empty());
        assert_eq!(decode(&entries, 100).unwrap(), values);
    }

    #[test]
    fn dense_keeps_everything() {
        let values = vec![Ternary::True, Ternary::False, Ternary::True];
        let entries = encode(&values);
        assert_eq!(entries.len(), 3);
        assert_eq!(decode(&entries, 3).unwrap(), values);
    }

    #[test]
    fn sparse_middle_entries_are_preserved() {
        // 位置 1 是 False，位置 3 是 True，其余 Unknown。
        let values = vec![
            Ternary::Unknown,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
            Ternary::Unknown,
        ];
        let entries = encode(&values);
        assert_eq!(entries, vec![(1, Ternary::False), (3, Ternary::True)]);
        assert_eq!(decode(&entries, 5).unwrap(), values);
    }

    #[test]
    fn out_of_range_index_is_rejected() {
        let entries = vec![(5, Ternary::True)];
        assert!(matches!(
            decode(&entries, 3),
            Err(CodecError::SparseIndexOutOfRange { index: 5, len: 3 })
        ));
    }

    #[test]
    fn duplicate_index_is_rejected() {
        let entries = vec![(1, Ternary::True), (1, Ternary::False)];
        assert!(matches!(
            decode(&entries, 5),
            Err(CodecError::SparseIndexNotIncreasing { .. })
        ));
    }

    #[test]
    fn unsorted_index_is_rejected() {
        let entries = vec![(3, Ternary::True), (1, Ternary::False)];
        assert!(matches!(
            decode(&entries, 5),
            Err(CodecError::SparseIndexNotIncreasing { prev: 3, found: 1 })
        ));
    }

    #[test]
    fn sparsity_makes_sense() {
        let entries = vec![(0, Ternary::True), (4, Ternary::False)];
        // 2/5 非零 → 稀疏率 0.6。
        assert!((sparsity(&entries, 5) - 0.6).abs() < 1e-9);
        assert_eq!(sparsity(&entries, 0), 0.0);
    }

    #[test]
    fn round_trip_randomish() {
        let values = vec![
            Ternary::True,
            Ternary::Unknown,
            Ternary::Unknown,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
        ];
        let entries = encode(&values);
        assert_eq!(decode(&entries, values.len()).unwrap(), values);
    }
}
