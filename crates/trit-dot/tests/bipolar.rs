//! #590 根治：长度跟着 `PackedBipolar` 走，调用方无法再谎报 `len`。

use trit_dot::{
    DotError, PackedBipolar, bipolar_dot, bipolar_dot_portable, pack_bipolar, scalar_dot,
};

fn bipolar_values(len: usize, seed: u64) -> (Vec<i32>, Vec<i32>) {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let a = (0..len)
        .map(|_| if next() & 1 == 0 { 1 } else { -1 })
        .collect();
    let b = (0..len)
        .map(|_| if next() & 1 == 0 { 1 } else { -1 })
        .collect();
    (a, b)
}

#[test]
fn packed_len_is_intrinsic() {
    let (a, b) = bipolar_values(65, 7);
    let pa = pack_bipolar(&a);
    let pb = pack_bipolar(&b);
    assert_eq!(pa.len(), 65);
    assert_eq!(pa.words().len(), 2);
    assert_eq!(bipolar_dot(&pa, &pb).expect("同长度"), scalar_dot(&a, &b));
}

#[test]
fn dot_matches_scalar_across_boundaries() {
    for len in [1usize, 64, 65, 129] {
        let (a, b) = bipolar_values(len, 7 + len as u64);
        let pa = pack_bipolar(&a);
        let pb = pack_bipolar(&b);
        let expected = scalar_dot(&a, &b);
        assert_eq!(bipolar_dot(&pa, &pb).expect("同长度"), expected);
        assert_eq!(bipolar_dot_portable(&pa, &pb).expect("同长度"), expected);
    }
}

#[test]
fn from_words_requires_exact_word_count() {
    // 65 trit 需要 2 个字；1 个字不够。
    assert!(matches!(
        PackedBipolar::from_words(vec![u64::MAX; 1], 65),
        Err(DotError::LengthMismatch { left: 1, right: 2 })
    ));
    // 64 trit 只需要 1 个字；2 个字多余。
    assert!(matches!(
        PackedBipolar::from_words(vec![u64::MAX; 2], 64),
        Err(DotError::LengthMismatch { left: 2, right: 1 })
    ));
}

#[test]
fn different_lengths_are_rejected() {
    let a = PackedBipolar::from_words(vec![u64::MAX; 2], 65).unwrap();
    let b = PackedBipolar::from_words(vec![u64::MAX; 2], 128).unwrap();
    assert_eq!(
        bipolar_dot(&a, &b),
        Err(DotError::LengthMismatch {
            left: 65,
            right: 128
        })
    );
    assert_eq!(
        bipolar_dot_portable(&a, &b),
        Err(DotError::LengthMismatch {
            left: 65,
            right: 128
        })
    );
}
