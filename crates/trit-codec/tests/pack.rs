//! `trit_codec::pack` 薄壳的公开 API 行为：常量、宽松/严格解码、错误映射与类型语义。
//!
//! 实现的行为测试在 `trit-core`（`crates/trit-core/src/pack.rs` 内嵌测试，变异锚点
//! 同在那里）；本文件测的是薄壳对调用方的**承诺**——`CodecError` 语义与 re-export
//! 的类型在迁移（实现下沉 `trit_core::pack`）之后不变。

use trit_codec::CodecError;
use trit_codec::pack::{
    COMBINATIONS, Packed5, Packed5Vec, TRITS_PER_BYTE, dual_rail_bytes, pack, unpack,
    unpack_canonical,
};
use trit_core::Ternary;

fn sample() -> Vec<Ternary> {
    vec![
        Ternary::True,
        Ternary::False,
        Ternary::Unknown,
        Ternary::True,
        Ternary::True,
        Ternary::False,
        Ternary::Unknown,
    ]
}

#[test]
fn constants_unchanged() {
    assert_eq!(TRITS_PER_BYTE, 5);
    assert_eq!(COMBINATIONS, 243);
    assert_eq!(dual_rail_bytes(9), 3);
    assert_eq!(dual_rail_bytes(1), 1);
}

#[test]
fn pack_is_canonical_low_digit_first() {
    // TTTTT = 2 + 6 + 18 + 54 + 162 = 242：最后一个合法字节。
    assert_eq!(pack(&[Ternary::True; 5]), vec![242]);
    // 低位在前：第 0 个 trit 权重 3^0。
    assert_eq!(
        pack(&[Ternary::Unknown, Ternary::False, Ternary::False]),
        vec![1]
    );
    // 长度规则：len 个 trit 恰好 ceil(len/5) 字节。
    let values = sample();
    assert_eq!(pack(&values).len(), values.len().div_ceil(5));
}

#[test]
fn unpack_is_lenient_about_extra_bytes_and_padding() {
    let values = sample();
    let bytes = pack(&values);
    // 多出的字节被丢弃。
    let mut padded = bytes.clone();
    padded.push(42);
    assert_eq!(unpack(&padded, values.len()).unwrap(), values);
    // 非规范 padding 不被宽松路径拒绝（字节 81 的尾部槽位是 Unknown）。
    assert_eq!(unpack(&[81], 1).unwrap(), vec![Ternary::False]);
}

#[test]
fn unpack_rejects_invalid_bytes() {
    for byte in [243u8, 250, 255] {
        assert_eq!(
            unpack(&[byte], 1).unwrap_err(),
            CodecError::InvalidByte(byte),
        );
    }
}

#[test]
fn unpack_rejects_overlong_request() {
    assert_eq!(
        unpack(&[0], 6).unwrap_err(),
        CodecError::Length {
            expected: 6,
            available: 5
        },
    );
}

#[test]
fn unpack_canonical_requires_exact_byte_count() {
    let values = sample();
    let bytes = pack(&values);
    assert_eq!(
        unpack_canonical(&bytes[..bytes.len() - 1], values.len()).unwrap_err(),
        CodecError::WrongByteCount {
            expected: bytes.len(),
            actual: bytes.len() - 1,
        },
    );
    assert_eq!(
        unpack_canonical(&[], 2).unwrap_err(),
        CodecError::WrongByteCount {
            expected: 1,
            actual: 0
        },
    );
}

#[test]
fn unpack_canonical_rejects_non_canonical_padding() {
    // 字节 81 = trit[4] 是 Unknown ⇒ len=4 时尾部槽位非 False。
    assert_eq!(
        unpack_canonical(&[81], 4).unwrap_err(),
        CodecError::NonCanonicalPadding(81),
    );
}

#[test]
fn unpack_canonical_round_trips() {
    let values = sample();
    let bytes = pack(&values);
    assert_eq!(unpack_canonical(&bytes, values.len()).unwrap(), values);
}

#[test]
fn packed5_construction_and_access() {
    assert_eq!(Packed5::try_new(242).unwrap().as_byte(), 242);
    assert!(Packed5::try_new(243).is_err());
    let values = [
        Ternary::True,
        Ternary::False,
        Ternary::Unknown,
        Ternary::True,
        Ternary::True,
    ];
    let packed = Packed5::from_digits(values);
    assert_eq!(packed.unpack(), values);
    assert_eq!(packed.digit(0), Ternary::True);
    assert_eq!(packed.digit(1), Ternary::False);
    assert_eq!(packed.digit(2), Ternary::Unknown);
}

#[test]
fn packed5vec_preserves_len_and_canonical_bytes() {
    let values = sample();
    let packed = Packed5Vec::from_trits(&values);
    assert_eq!(packed.len(), values.len());
    assert!(!packed.is_empty());
    assert_eq!(packed.as_bytes(), pack(&values).as_slice());
    assert_eq!(packed.get(0), Some(Ternary::True));
    assert_eq!(packed.get(6), Some(Ternary::Unknown));
    assert_eq!(packed.get(7), None);
    assert_eq!(packed.unpack(), values);
    // try_from_bytes 复用严格校验：非规范输入被拒。
    assert!(Packed5Vec::try_from_bytes(vec![81], 4).is_err());
    assert!(Packed5Vec::try_from_bytes(pack(&values), values.len()).is_ok());
}

#[test]
fn empty_input_edge_cases() {
    assert_eq!(pack(&[]), Vec::<u8>::new());
    assert_eq!(unpack(&[], 0).unwrap(), Vec::<Ternary>::new());
    assert_eq!(unpack_canonical(&[], 0).unwrap(), Vec::<Ternary>::new());
    let empty = Packed5Vec::from_trits(&[]);
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
}

#[test]
fn ternary_mapping_is_explicit() {
    // 契约第 1 条：False=0、Unknown=1、True=2（不要依赖 serde 表示）。
    assert_eq!(pack(&[Ternary::False]), vec![0]);
    assert_eq!(pack(&[Ternary::Unknown]), vec![1]);
    assert_eq!(pack(&[Ternary::True]), vec![2]);
}
