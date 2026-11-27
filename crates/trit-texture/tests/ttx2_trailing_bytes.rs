//! 回归：TTX2 section 尾部残留未对账字节必须在解析期拒绝（TEX / MOD 同口径）。
//!
//! 尾部字节既不进解码流，也不进 CRC 流（`texture_trits` / `model_trits` 只覆盖
//! 已解码级别/层）；`section_len` 放大后外层长度检查放行，section 内部游标却
//! 从不与 `section.len()` 对账，残留字节被静默忽略。

use trit_texture::ttx2::{
    HEADER_BYTES, TritModel, TritModelLayer, TtxDocument, from_ttx2, to_ttx2,
};
use trit_texture::{TRIT_BLOCK_BYTES, TextureError, encode_rgba};

fn sample(width: usize, height: usize) -> Vec<u8> {
    (0..width * height * 4).map(|i| (i % 251) as u8).collect()
}

fn tex_single() -> Vec<u8> {
    let level = encode_rgba(&sample(16, 16), 16, 16).unwrap();
    to_ttx2(&TtxDocument::Texture {
        levels: vec![level],
    })
    .unwrap()
}

fn model_single() -> Vec<u8> {
    let layer = TritModelLayer::new(&[vec![1, -1, 0, 1, -1, 0, 1, 1]], &[1]).unwrap();
    to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap()
}

/// 在 section 尾部追加 `extra` 字节并同步放大 `section_len`（外层长度检查仍放行）。
fn append_trailing(bytes: &[u8], extra: usize) -> Vec<u8> {
    let section_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut section = bytes[HEADER_BYTES..HEADER_BYTES + section_len].to_vec();
    section.extend(std::iter::repeat_n(0xCD, extra));
    let mut out = Vec::new();
    out.extend_from_slice(&bytes[..HEADER_BYTES]);
    out.extend_from_slice(&section);
    out.extend_from_slice(&bytes[HEADER_BYTES + section_len..]);
    out[8..12].copy_from_slice(&(section.len() as u32).to_le_bytes());
    out
}

#[test]
fn texture_section_trailing_bytes_are_rejected() {
    let bad = append_trailing(&tex_single(), TRIT_BLOCK_BYTES);
    assert!(
        matches!(from_ttx2(&bad), Err(TextureError::TtxTruncated { .. })),
        "TEX section 尾部残留块必须在解析期拒绝"
    );
}

#[test]
fn model_section_trailing_bytes_are_rejected() {
    let bad = append_trailing(&model_single(), TRIT_BLOCK_BYTES);
    assert!(
        matches!(from_ttx2(&bad), Err(TextureError::TtxTruncated { .. })),
        "MOD section 尾部残留块必须在解析期拒绝"
    );
}

#[test]
fn clean_sections_still_parse() {
    assert!(from_ttx2(&tex_single()).is_ok());
    assert!(from_ttx2(&model_single()).is_ok());
}
