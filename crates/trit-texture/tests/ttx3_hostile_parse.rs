//! TTX3 解析期敌对偏移拒绝（第 40 轮 CI 修复轮增补）。
//!
//! 目标：`off1 = u64::MAX` 必须在 `parse` 阶段就判 `Ttx3Truncated`，
//! 不允许靠算术回绕或后续解码兜底——后者在 release 下会静默接受。

use trit_texture::TextureError;
use trit_texture::ttx3::{
    HEADER_BYTES, LargeTexture, LevelPolicy, TILE_HEIGHT, TILE_WIDTH, Ttx3Archive,
};

fn solid_rgba(width: usize, height: usize, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    for i in (0..out.len()).step_by(4) {
        out[i] = rgb[0];
        out[i + 1] = rgb[1];
        out[i + 2] = rgb[2];
        out[i + 3] = 255;
    }
    out
}

fn one_level(policy: LevelPolicy) -> Vec<u8> {
    let mut lt = LargeTexture::new(TILE_WIDTH, TILE_HEIGHT, &[policy]).expect("尺寸合法");
    lt.push_level_rgba(0, &solid_rgba(TILE_WIDTH, TILE_HEIGHT, [40, 90, 160]))
        .expect("编码");
    lt.to_bytes()
}

fn first_tile_entry(bytes: &[u8]) -> usize {
    // 级 0 的 `dir_offset` 字段即首条目起点。
    let at = HEADER_BYTES + 12;
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize
}

#[test]
fn hostile_u64_max_offset_is_rejected_at_parse() {
    let mut bytes = one_level(LevelPolicy::Capacity);
    let entry = first_tile_entry(&bytes);
    bytes[entry..entry + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Truncated { .. }),
        "u64::MAX 偏移应在解析期判越界，不得靠回绕或后续解码兜底，实际 {error:?}"
    );
}
