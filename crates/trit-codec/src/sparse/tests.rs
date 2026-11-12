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

#[test]
fn decode_len_cap_is_checked_before_allocation() {
    // usize::MAX 必须在分配前被硬顶拦下，而不是去 reserve 一个天文数字。
    assert!(matches!(
        decode(&[], usize::MAX),
        Err(CodecError::SparseLenTooLarge {
            len: usize::MAX,
            max: MAX_SPARSE_DECODE_LEN
        })
    ));
}

#[test]
fn check_len_boundary() {
    assert!(check_len(0).is_ok());
    assert!(check_len(MAX_SPARSE_DECODE_LEN).is_ok());
    assert!(matches!(
        check_len(MAX_SPARSE_DECODE_LEN + 1),
        Err(CodecError::SparseLenTooLarge { .. })
    ));
    assert!(matches!(
        check_len(usize::MAX),
        Err(CodecError::SparseLenTooLarge { .. })
    ));
}

#[test]
fn decode_within_cap_still_fills_unknown() {
    // 上限内的正常长度仍逐位还原（硬顶不误伤合法输入）。
    let out = decode(&[(2, Ternary::True)], 4).unwrap();
    assert_eq!(
        out,
        vec![
            Ternary::Unknown,
            Ternary::Unknown,
            Ternary::True,
            Ternary::Unknown
        ]
    );
}
