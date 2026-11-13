//! 5 trits / byte 打包：**实现已下沉到 [`trit_core::pack`]**（可发布底层，
//! 2026-11-14 起，`trit-export` 与本 crate 共用同一份），本模块保持既有 API 与
//! [`CodecError`] 错误类型不变：常量与类型直接 re-export，返回错误的函数做
//! `PackError -> CodecError` 的一一映射。线格式契约文档随实现走，见
//! `trit_core::pack` 的模块文档。

pub use trit_core::pack::{
    COMBINATIONS, Packed5, Packed5Vec, TRITS_PER_BYTE, dual_rail_bytes, pack,
};

use trit_core::pack::PackError;
use trit_core::value::Ternary;

use crate::CodecError;

/// `PackError -> CodecError` 的一一映射（变体一一对应，语义不变）。
fn into_codec(error: PackError) -> CodecError {
    match error {
        PackError::InvalidByte(byte) => CodecError::InvalidByte(byte),
        PackError::NonCanonicalPadding(byte) => CodecError::NonCanonicalPadding(byte),
        PackError::WrongByteCount { expected, actual } => {
            CodecError::WrongByteCount { expected, actual }
        }
        PackError::Length {
            expected,
            available,
        } => CodecError::Length {
            expected,
            available,
        },
    }
}

/// 解码前 `len` 个 trit；宽松路径（契约见 `trit_core::pack::unpack`）。
pub fn unpack(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    trit_core::pack::unpack(bytes, len).map_err(into_codec)
}

/// 严格解码：canonical 字节数精确且尾部填充为 `False`（契约见
/// `trit_core::pack::unpack_canonical`）。
pub fn unpack_canonical(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    trit_core::pack::unpack_canonical(bytes, len).map_err(into_codec)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 薄壳只测映射本身；实现的行为测试在 `trit-core`（`crates/trit-core/tests/pack.rs`）。

    #[test]
    fn facade_maps_every_pack_error_variant() {
        assert_eq!(unpack(&[255], 1).unwrap_err(), CodecError::InvalidByte(255),);
        assert_eq!(
            unpack_canonical(&[], 2).unwrap_err(),
            CodecError::WrongByteCount {
                expected: 1,
                actual: 0
            },
        );
        assert_eq!(
            unpack(&[0], 6).unwrap_err(),
            CodecError::Length {
                expected: 6,
                available: 5
            },
        );
        assert_eq!(
            unpack_canonical(&[81], 4).unwrap_err(),
            CodecError::NonCanonicalPadding(81),
        );
    }

    #[test]
    fn facade_round_trips_like_the_implementation() {
        let values = [
            Ternary::True,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
            Ternary::True,
        ];
        let bytes = pack(&values);
        assert_eq!(unpack(&bytes, 5).unwrap(), values.to_vec());
        assert_eq!(unpack_canonical(&bytes, 5).unwrap(), values.to_vec());
        assert_eq!(Packed5::from_digits(values).as_byte(), bytes[0]);
        let packed = Packed5Vec::from_trits(&values);
        assert_eq!(packed.as_bytes(), bytes.as_slice());
        assert_eq!(packed.unpack(), values.to_vec());
        assert_eq!(dual_rail_bytes(9), 3);
    }
}
