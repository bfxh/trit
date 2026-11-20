//! TTX3 各级宽高在 `parse` 时校验：`decode_level` 的 `width*height*4` 不再吃未校验的 u32。
//!
//! `LargeTexture::new` 本来有 `validate_dims`，但 `Ttx3Archive::parse` 直接读文件头，
//! 以前绕过它；这里用一个超边长的级目录条目钉住。

use trit_texture::ttx3::{
    HEADER_BYTES, LEVEL_ENTRY_BYTES, MAGIC, MAX_SIDE, TILE_HEIGHT, TILE_WIDTH, Ttx3Archive,
};

#[test]
fn parse_rejects_oversize_level_dims() {
    let mut b = vec![0u8; HEADER_BYTES + LEVEL_ENTRY_BYTES];
    b[0..4].copy_from_slice(&MAGIC);
    b[8] = 1; // level_count
    b[12..20].copy_from_slice(&(HEADER_BYTES as u64).to_le_bytes());
    let at = HEADER_BYTES;
    b[at..at + 4].copy_from_slice(&((MAX_SIDE + TILE_WIDTH) as u32).to_le_bytes());
    b[at + 4..at + 8].copy_from_slice(&(TILE_HEIGHT as u32).to_le_bytes());
    let err = Ttx3Archive::parse(&b).expect_err("超边长必须 Err");
    assert!(err.to_string().contains("边长"), "实得: {err}");
}
