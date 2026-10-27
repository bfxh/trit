//! TTX3 万级大纹理档案的集成测试（公开 API 面）。

use trit_texture::ttx3::{
    LargeTexture, LevelPolicy, TILE_BLOCKS, TILE_DATA_BYTES, TILE_ENTRY_BYTES, TILE_HEIGHT,
    TILE_TEXELS, TILE_WIDTH, TileRgba, Ttx3Archive, level_storage_bytes, suggest_chain,
};
use trit_texture::{TextureError, decode_rgba, decode_rgba_into, encode_rgba};

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
fn round_trip_mixed_policies() {
    let (texture, sources) = sample();
    let bytes = texture.to_bytes();
    let archive = Ttx3Archive::parse(&bytes).expect("解析");
    assert_eq!(archive.level_count(), 2);
    assert_eq!(archive.level_info(0).unwrap().0, 512);
    assert_eq!(archive.level_info(0).unwrap().2, LevelPolicy::Capacity);
    assert_eq!(archive.level_info(1).unwrap().2, LevelPolicy::Strength);
    // 逐瓦片解码对拍：与 crate 自身的 TT3 编解码逐位一致——
    // TTX3 只做瓦片化与容器，不改块语义；有损性是 TT3 的（调色板量化）。
    for (level, rgba) in sources.iter().enumerate() {
        let reference = decode_rgba(
            &encode_rgba(
                rgba,
                texture.levels[level].width,
                texture.levels[level].height,
            )
            .expect("参照"),
        )
        .expect("参照解码");
        let (width, height, _) = archive.level_info(level).unwrap();
        let (tiles_x, tiles_y) = (width / TILE_WIDTH, height / TILE_HEIGHT);
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile = archive.decode_tile(level, tx, ty).expect("解码");
                for row in 0..TILE_HEIGHT {
                    let src_at = ((ty * TILE_HEIGHT + row) * width + tx * TILE_WIDTH) * 4;
                    let got_at = row * TILE_WIDTH * 4;
                    assert_eq!(
                        &tile[got_at..got_at + TILE_WIDTH * 4],
                        &reference[src_at..src_at + TILE_WIDTH * 4],
                        "level={level} tile=({tx},{ty}) row={row}"
                    );
                }
            }
        }
    }
    // 全量校验：2×2 + 1×1 = 5 瓦片。
    assert_eq!(archive.verify().unwrap(), 5);
}

#[test]
fn strength_recovers_single_copy_corruption() {
    let (texture, sources) = sample();
    let mut bytes = texture.to_bytes();
    let view = Ttx3Archive::parse(&bytes).expect("解析");
    let (_, _, policy) = view.level_info(1).unwrap();
    assert_eq!(policy, LevelPolicy::Strength);
    let (off1, off2, _, _) = view.tile_entry(1, 0, 0).unwrap();
    bytes[off1 as usize + 7] ^= 0xFF; // 打坏 copy1
    let repaired = Ttx3Archive::parse(&bytes).expect("解析");
    // 单副本损坏：Strength 档自动走 copy2 恢复，解码结果与
    // crate 参考路径逐位一致（有损是 TT3 的语义，不是容器的）。
    let tile = repaired.decode_tile(1, 0, 0).expect("应从 copy2 恢复");
    let reference =
        decode_rgba(&encode_rgba(&sources[1], 256, 320).expect("参照")).expect("参照解码");
    assert_eq!(tile, reference);
    let _ = (off2, &repaired);
}

#[test]
fn both_copies_corrupt_is_structured_error() {
    let (texture, _) = sample();
    let mut bytes = texture.to_bytes();
    let view = Ttx3Archive::parse(&bytes).expect("解析");
    let (off1, off2, _, _) = view.tile_entry(1, 0, 0).unwrap();
    bytes[off1 as usize] ^= 0xFF;
    bytes[off2 as usize] ^= 0xFF;
    let broken = Ttx3Archive::parse(&bytes).expect("解析");
    let error = broken.tile_blocks(1, 0, 0).unwrap_err();
    assert!(error.to_string().contains("两份冗余副本都损坏"), "{error}");
}

#[test]
fn capacity_corruption_is_detected_not_guessed() {
    let (texture, _) = sample();
    let mut bytes = texture.to_bytes();
    let view = Ttx3Archive::parse(&bytes).expect("解析");
    let (off1, off2, _, _) = view.tile_entry(0, 0, 0).unwrap();
    assert_eq!(off2, 0, "Capacity 档无第二份");
    bytes[off1 as usize] ^= 0xFF;
    let broken = Ttx3Archive::parse(&bytes).expect("解析");
    let error = broken.decode_tile(0, 0, 0).unwrap_err();
    assert!(error.to_string().contains("校验不符"), "{error}");
}

#[test]
fn wanji_account_is_closed_form() {
    // 万级 10240×10240：40×32 = 1280 瓦片。
    assert_eq!(
        level_storage_bytes(10240, 10240, LevelPolicy::Capacity),
        1280 * (TILE_DATA_BYTES as u64 + TILE_ENTRY_BYTES as u64)
    );
    assert_eq!(
        level_storage_bytes(10240, 10240, LevelPolicy::Strength),
        1280 * (TILE_DATA_BYTES as u64 * 2 + TILE_ENTRY_BYTES as u64)
    );
    // 全链账（suggest_chain + 逐级账）：细层大头，粗层冗余小头。
    let mut total = 0u64;
    for (w, h, policy) in suggest_chain(10240, 10240) {
        total += level_storage_bytes(w, h, policy);
    }
    let raw = 10240u64 * 10240 * 4;
    assert!(total < raw / 8, "存档应低于原图 1/8：{total} vs {raw}");
    // 级数与默认混搭：最粗两级 Strength。
    let chain = suggest_chain(10240, 10240);
    assert_eq!(chain.len(), 6);
    assert_eq!(chain[0].2, LevelPolicy::Capacity);
    assert_eq!(chain[4].2, LevelPolicy::Strength);
    assert_eq!(chain[5].2, LevelPolicy::Strength);
}

#[test]
fn large_texture_rejects_bad_dimensions_and_levels() {
    assert!(
        LargeTexture::new(300, 320, &[LevelPolicy::Capacity]).is_err(),
        "宽不是瓦片倍数"
    );
    assert!(LargeTexture::new(0, 320, &[LevelPolicy::Capacity]).is_err());
    assert!(LargeTexture::new(65536, 131072, &[LevelPolicy::Capacity]).is_err());
    assert!(LargeTexture::new(512, 640, &[]).is_err(), "级数不能为空");
    let many = vec![LevelPolicy::Capacity; 19];
    assert!(LargeTexture::new(512, 640, &many).is_err(), "级数超上限");
}

#[test]
fn parse_rejects_foreign_magic_and_flags() {
    let (texture, _) = sample();
    let mut bytes = texture.to_bytes();
    bytes[0..4].copy_from_slice(b"TTX2");
    assert!(matches!(
        Ttx3Archive::parse(&bytes),
        Err(TextureError::Ttx3Magic)
    ));
    let (texture, _) = sample();
    let mut bytes = texture.to_bytes();
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    assert!(matches!(
        Ttx3Archive::parse(&bytes),
        Err(TextureError::Ttx3Flags { flags: 1 })
    ));
}

#[test]
fn decode_tile_bounds_are_structured_errors() {
    let (texture, _) = sample();
    let bytes = texture.to_bytes();
    let archive = Ttx3Archive::parse(&bytes).expect("解析");
    let error = archive.decode_tile(0, 999, 0).unwrap_err();
    assert!(error.to_string().contains("越界"), "{error}");
    let error = archive.decode_tile(7, 0, 0).unwrap_err();
    assert!(error.to_string().contains("级不存在"), "{error}");
}

#[test]
fn suggest_chain_snaps_to_tile_multiples() {
    for (w, h, _) in suggest_chain(10240, 10240) {
        assert_eq!(w % TILE_WIDTH, 0, "w={w}");
        assert_eq!(h % TILE_HEIGHT, 0, "h={h}");
    }
}

#[test]
fn encode_decode_matches_crate_path_bit_exact() {
    // TTX3 的瓦片化不得改变 TT3 块语义：整级解码必须与
    // 「encode_rgba → decode_rgba」逐位一致（TT3 的有损量化
    // 是 crate 语义，不是 TTX3 的）。
    let (texture, sources) = sample();
    for (level, rgba) in sources.iter().enumerate() {
        let decoded = texture.decode_level(level).expect("解码");
        let reference = decode_rgba(
            &encode_rgba(
                rgba,
                texture.levels[level].width,
                texture.levels[level].height,
            )
            .expect("参照"),
        )
        .expect("参照解码");
        assert_eq!(decoded, reference, "level={level}");
    }
    // 瓦片维度常量自洽。
    assert_eq!(TILE_TEXELS, TILE_WIDTH * TILE_HEIGHT);
    assert_eq!(TILE_BLOCKS, TILE_TEXELS / 40);
    assert_eq!(TILE_DATA_BYTES, TILE_BLOCKS * 12);
    let _ =
        decode_rgba_into as fn(&trit_texture::TritTexture, &mut [u8]) -> Result<(), TextureError>;
    let _ = TileRgba::default();
}
