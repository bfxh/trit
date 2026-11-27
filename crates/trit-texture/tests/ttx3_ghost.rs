//! TTX3 幽灵级：零尺寸、谎报级数与跨级递增都必须在 parse 阶段拒绝。

use trit_texture::TextureError;
use trit_texture::ttx3::{
    HEADER_BYTES, LEVEL_ENTRY_BYTES, LargeTexture, LevelPolicy, TILE_HEIGHT, TILE_WIDTH,
    Ttx3Archive,
};

fn two_level() -> Vec<u8> {
    LargeTexture::new(512, 640, &[LevelPolicy::Capacity, LevelPolicy::Strength])
        .expect("尺寸合法")
        .to_bytes()
}

fn level_entry(index: usize) -> usize {
    HEADER_BYTES + index * LEVEL_ENTRY_BYTES
}

#[test]
fn zero_sized_ghost_level_is_rejected_by_parse() {
    let mut bytes = two_level();
    let at = level_entry(1);
    bytes[at..at + 4].fill(0);
    bytes[at + 4..at + 8].fill(0);
    assert!(matches!(
        Ttx3Archive::parse(&bytes),
        Err(TextureError::Ttx3Dims { .. })
    ));
}

#[test]
fn non_descending_levels_are_rejected_by_parse() {
    for (width, height) in [(512, 640), (1024, 640)] {
        let mut bytes = two_level();
        let at = level_entry(1);
        bytes[at..at + 4].copy_from_slice(&(width as u32).to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&(height as u32).to_le_bytes());
        assert!(
            matches!(
                Ttx3Archive::parse(&bytes),
                Err(TextureError::Ttx3Level { .. })
            ),
            "第二级 {width}×{height} 应拒绝"
        );
    }
}

#[test]
fn lying_level_count_is_rejected_by_parse() {
    let mut bytes = LargeTexture::new(TILE_WIDTH, TILE_HEIGHT, &[LevelPolicy::Capacity])
        .expect("尺寸合法")
        .to_bytes();
    bytes[8] = 2;
    assert!(
        Ttx3Archive::parse(&bytes).is_err(),
        "单级档案把 level_count 改成 2，多出来的条目是残留字节，必须拒绝"
    );
}

#[test]
fn equal_width_shrinking_height_chain_is_still_valid() {
    // 非方形减半链：宽到瓦片下限后保持不变、高继续减半，不能误判为“未递减”。
    let bytes = LargeTexture::new(256, 640, &[LevelPolicy::Capacity, LevelPolicy::Strength])
        .expect("尺寸合法")
        .to_bytes();
    let archive = Ttx3Archive::parse(&bytes).expect("合法减半链");
    assert_eq!(archive.level_count(), 2);
    assert_eq!(
        archive.level_info(1).expect("第二级"),
        (256, 320, LevelPolicy::Strength)
    );
}
