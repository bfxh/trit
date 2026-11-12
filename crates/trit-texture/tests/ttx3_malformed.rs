//! TTX3 恶意/构造输入 + Strength 多瓦片的集成测试（从 tests/ttx3.rs 拆出，控制行数预算）。

use trit_texture::ttx3::{
    HEADER_BYTES, LargeTexture, LevelPolicy, TILE_DATA_BYTES, TILE_ENTRY_BYTES, TILE_HEIGHT,
    TILE_WIDTH, Ttx3Archive,
};
use trit_texture::{TRIT_BLOCK_BYTES, TextureError, decode_rgba, encode_rgba};

fn gradient_rgba(width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            out[at] = (x % 251) as u8;
            out[at + 1] = (y % 241) as u8;
            out[at + 2] = ((x + y) % 253) as u8;
            out[at + 3] = 255;
        }
    }
    out
}

/// 2×2 瓦片的最小测试档（512×640），混搭政策。
fn sample() -> (LargeTexture, Vec<Vec<u8>>) {
    let mut texture = LargeTexture::new(512, 640, &[LevelPolicy::Capacity, LevelPolicy::Strength])
        .expect("尺寸合法");
    let sources: Vec<Vec<u8>> = vec![gradient_rgba(512, 640), gradient_rgba(256, 320)];
    for (level, rgba) in sources.iter().enumerate() {
        texture.push_level_rgba(level, rgba).expect("编码");
    }
    (texture, sources)
}

#[test]
fn strength_level_with_many_tiles_round_trips_and_keeps_redundancy() {
    // 4 个瓦片的 Strength 级：每片的两份副本都要指向它自己的数据。
    let (width, height) = (TILE_WIDTH * 2, TILE_HEIGHT * 2);
    let rgba = gradient_rgba(width, height);
    let mut texture = LargeTexture::new(width, height, &[LevelPolicy::Strength]).expect("尺寸合法");
    texture.push_level_rgba(0, &rgba).expect("编码");
    let bytes = texture.to_bytes();
    let archive = Ttx3Archive::parse(&bytes).expect("解析");
    assert_eq!(archive.verify().unwrap(), 4);

    let reference =
        decode_rgba(&encode_rgba(&rgba, width, height).expect("参照")).expect("参照解码");
    for ty in 0..2 {
        for tx in 0..2 {
            let (off1, off2, len, _) = archive.tile_entry(0, tx, ty).unwrap();
            assert_eq!(off2, off1 + u64::from(len), "tile=({tx},{ty})");
            for (copy, offset) in [(1, off1), (2, off2)] {
                let mut damaged = bytes.clone();
                damaged[offset as usize] ^= 0xFF;
                let tile = Ttx3Archive::parse(&damaged)
                    .expect("解析")
                    .decode_tile(0, tx, ty)
                    .unwrap_or_else(|e| panic!("tile=({tx},{ty}) copy{copy} 损坏应能恢复：{e}"));
                for row in 0..TILE_HEIGHT {
                    let src_at = ((ty * TILE_HEIGHT + row) * width + tx * TILE_WIDTH) * 4;
                    let got_at = row * TILE_WIDTH * 4;
                    assert_eq!(
                        &tile[got_at..got_at + TILE_WIDTH * 4],
                        &reference[src_at..src_at + TILE_WIDTH * 4],
                        "tile=({tx},{ty}) copy{copy} row={row}"
                    );
                }
            }
        }
    }
}

/// 级 0 的瓦片目录起点（级目录第 0 项的 `dir_offset` 字段）。
fn level0_dir_offset(bytes: &[u8]) -> usize {
    let at = HEADER_BYTES + 12;
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize
}

/// 解析并全量校验；任何 panic 都会让测试失败。
fn verify_bytes(bytes: &[u8]) -> Result<usize, TextureError> {
    Ttx3Archive::parse(bytes)?.verify()
}

#[test]
fn hostile_level_dir_offset_is_truncation_error() {
    let (texture, _) = sample();
    let mut bytes = texture.to_bytes();
    let at = HEADER_BYTES + 12;
    for offset in [
        u64::MAX,
        u64::MAX - TILE_ENTRY_BYTES as u64,
        bytes.len() as u64,
    ] {
        bytes[at..at + 8].copy_from_slice(&offset.to_le_bytes());
        assert!(
            matches!(
                verify_bytes(&bytes),
                Err(TextureError::Ttx3Truncated { .. })
            ),
            "dir_offset={offset}"
        );
    }
}

#[test]
fn hostile_tile_offset_is_truncation_error() {
    let (texture, _) = sample();
    let mut bytes = texture.to_bytes();
    let entry = level0_dir_offset(&bytes);
    for offset in [u64::MAX, u64::MAX - 5, bytes.len() as u64] {
        bytes[entry..entry + 8].copy_from_slice(&offset.to_le_bytes());
        assert!(
            matches!(
                verify_bytes(&bytes),
                Err(TextureError::Ttx3Truncated { .. })
            ),
            "off1={offset}"
        );
    }
}

#[test]
fn tile_len_other_than_tile_data_bytes_is_rejected() {
    let (texture, _) = sample();
    let pristine = texture.to_bytes();
    let entry = level0_dir_offset(&pristine);
    for len in [
        0,
        1,
        TRIT_BLOCK_BYTES,
        TILE_DATA_BYTES - 1,
        TILE_DATA_BYTES + 1,
    ] {
        let mut bytes = pristine.clone();
        bytes[entry + 16..entry + 20].copy_from_slice(&(len as u32).to_le_bytes());
        assert!(
            matches!(
                verify_bytes(&bytes),
                Err(TextureError::Ttx3Tile {
                    level: 0,
                    tx: 0,
                    ty: 0,
                    ..
                })
            ),
            "len={len}"
        );
    }
}
