//! I2_S 契约：映射、块内交错、位序、scale 口径与尾部布局。
//!
//! 规格与判据见 `docs/plans/BITNET-FORMAT-INTEROP-PLAN.md`（J1–J3）。

use trit_core::Ternary;
use trit_export::bitnet::i2s::{
    BLOCK_VALUES, TAIL_BYTES, decode, encode_f32, encode_ternary, pack, quantize_scale,
};

fn patterned(len: usize) -> Vec<Ternary> {
    (0..len)
        .map(|i| match i % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        })
        .collect()
}

#[test]
fn j1_roundtrip_is_bit_exact() {
    let trits = patterned(2 * BLOCK_VALUES);
    let bytes = encode_ternary(&trits, 0.25).unwrap();
    assert_eq!(bytes.len(), trits.len() / 4 + TAIL_BYTES);
    let (back, scale) = decode(&bytes, trits.len()).unwrap();
    assert_eq!(back, trits);
    assert_eq!(scale, 0.25);
}

#[test]
fn j1_unaligned_count_is_rejected() {
    let trits = vec![Ternary::True; BLOCK_VALUES + 1];
    assert!(pack(&trits).is_err());
    assert!(encode_ternary(&trits, 1.0).is_err());
    assert!(decode(&[0u8; 40], BLOCK_VALUES + 1).is_err());
}

#[test]
fn j2_block_interleave_and_bit_order() {
    let mut trits = vec![Ternary::Unknown; BLOCK_VALUES];
    trits[0] = Ternary::False; // c0 = 0
    trits[32] = Ternary::Unknown; // c1 = 1
    trits[64] = Ternary::True; // c2 = 2
    trits[96] = Ternary::False; // c3 = 0
    let packed = pack(&trits).unwrap();
    assert_eq!(packed[0], 0x18, "0<<6 | 1<<4 | 2<<2 | 0");
}

#[test]
fn j3_scale_uses_first_nonzero_abs() {
    assert_eq!(quantize_scale(&[0.0, 0.5, -0.25]), 0.5);
    assert_eq!(quantize_scale(&[0.0, -0.0]), 1e-5);
}

#[test]
fn j3_encode_f32_uses_and_overrides_scale() {
    let mut values = vec![0.0f32; BLOCK_VALUES];
    values[0] = 2.0;
    values[1] = -1.0;
    let (_, scale) = encode_f32(&values, None).unwrap();
    assert_eq!(scale, 2.0);
    let (_, overridden) = encode_f32(&values, Some(0.5)).unwrap();
    assert_eq!(overridden, 0.5);
}

#[test]
fn tail_stores_f32_scale_and_zero_padding() {
    let trits = vec![Ternary::True; BLOCK_VALUES];
    let bytes = encode_ternary(&trits, 2.5).unwrap();
    let tail = &bytes[bytes.len() - TAIL_BYTES..];
    assert_eq!(f32::from_le_bytes(tail[..4].try_into().unwrap()), 2.5);
    assert!(tail[4..].iter().all(|b| *b == 0));
}

#[test]
fn length_mismatch_is_rejected() {
    let trits = vec![Ternary::True; BLOCK_VALUES];
    let mut bytes = encode_ternary(&trits, 1.0).unwrap();
    bytes.pop();
    assert!(decode(&bytes, BLOCK_VALUES).is_err());
}

#[test]
fn invalid_scale_is_rejected() {
    let trits = vec![Ternary::True; BLOCK_VALUES];
    assert!(encode_ternary(&trits, 0.0).is_err());
    assert!(encode_ternary(&trits, f32::NAN).is_err());
    // 尾部 scale 为 0：长度对、scale 非法。
    assert!(decode(&[0u8; 64], BLOCK_VALUES).is_err());
}
