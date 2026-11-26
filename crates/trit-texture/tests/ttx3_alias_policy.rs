//! TTX3 跨级/跨瓦片载荷别名与 LevelPolicy-副本数对账回归（第 40 轮 FIX-T，F1/F2）。
//!
//! 覆盖：跨级别名拒绝、跨瓦片第二副本别名拒绝、首尾相接放行、零长度条目、
//! `u32::MAX` 偏移/长度、单级链、`level_count=0`、物理副本数 0/1/2、
//! Capacity 挂非零 `off2`、政策字节双向翻转、未知政策字节。每条断言具体错误类别。

use trit_texture::TextureError;
use trit_texture::ttx3::{
    HEADER_BYTES, LEVEL_ENTRY_BYTES, LargeTexture, LevelPolicy, TILE_DATA_BYTES, TILE_ENTRY_BYTES,
    TILE_HEIGHT, TILE_WIDTH, Ttx3Archive,
};

fn solid_rgba(width: usize, height: usize, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    for px in out.chunks_exact_mut(4) {
        px[0] = rgb[0];
        px[1] = rgb[1];
        px[2] = rgb[2];
        px[3] = 255;
    }
    out
}

/// 单级单瓦片干净档（256×320）。
fn one_level(policy: LevelPolicy) -> Vec<u8> {
    let mut lt = LargeTexture::new(TILE_WIDTH, TILE_HEIGHT, &[policy]).expect("尺寸合法");
    lt.push_level_rgba(0, &solid_rgba(TILE_WIDTH, TILE_HEIGHT, [40, 90, 160]))
        .expect("编码");
    lt.to_bytes()
}

/// 两级档：L0 512×640（2×2 瓦片），L1 256×320（1 瓦片），同政策。
fn two_level(policy: LevelPolicy) -> Vec<u8> {
    let mut lt =
        LargeTexture::new(TILE_WIDTH * 2, TILE_HEIGHT * 2, &[policy, policy]).expect("尺寸合法");
    lt.push_level_rgba(
        0,
        &solid_rgba(TILE_WIDTH * 2, TILE_HEIGHT * 2, [220, 30, 30]),
    )
    .expect("编码");
    lt.push_level_rgba(1, &solid_rgba(TILE_WIDTH, TILE_HEIGHT, [30, 200, 30]))
        .expect("编码");
    lt.to_bytes()
}

/// 两级横向双瓦片档（512×320），用于跨瓦片第二副本别名。
fn two_tile_strength() -> Vec<u8> {
    let mut lt =
        LargeTexture::new(TILE_WIDTH * 2, TILE_HEIGHT, &[LevelPolicy::Strength]).expect("尺寸合法");
    lt.push_level_rgba(0, &solid_rgba(TILE_WIDTH * 2, TILE_HEIGHT, [40, 90, 160]))
        .expect("编码");
    lt.to_bytes()
}

fn level_dir(bytes: &[u8], level: usize) -> usize {
    let at = HEADER_BYTES + level * LEVEL_ENTRY_BYTES;
    u64::from_le_bytes(bytes[at + 12..at + 20].try_into().unwrap()) as usize
}

fn tile_entry_at(bytes: &[u8], level: usize, tile: usize) -> usize {
    level_dir(bytes, level) + tile * TILE_ENTRY_BYTES
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn write_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn write_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn cross_level_alias_is_rejected() {
    // L1 瓦片 0 条目 off1/crc 指回 L0 瓦片 0 的合法载荷：跨级别名必须结构化拒绝。
    let mut bytes = two_level(LevelPolicy::Capacity);
    let l0_entry = tile_entry_at(&bytes, 0, 0);
    let l1_entry = tile_entry_at(&bytes, 1, 0);
    let l0_off1 = read_u64(&bytes, l0_entry);
    let l0_crc = read_u32(&bytes, l0_entry + 20);
    write_u64(&mut bytes, l1_entry, l0_off1);
    write_u32(&mut bytes, l1_entry + 20, l0_crc);

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "跨级别名应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn cross_tile_copy2_alias_is_rejected() {
    // Strength tile0 的 off2 指到 tile1 的副本 1：跨瓦片别名重叠必须拒绝。
    let mut bytes = two_tile_strength();
    let t0 = tile_entry_at(&bytes, 0, 0);
    let t1 = tile_entry_at(&bytes, 0, 1);
    let t1_off1 = read_u64(&bytes, t1);
    write_u64(&mut bytes, t0 + 8, t1_off1);

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "跨瓦片第二副本别名应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn adjacent_intervals_are_accepted() {
    // 首尾相接（上一区间终点 == 下一区间起点）合法：干净多瓦片档照常通过。
    let two_level = two_level(LevelPolicy::Capacity);
    let archive = Ttx3Archive::parse(&two_level).expect("两级 Capacity 应解析");
    assert_eq!(archive.verify().unwrap(), 5);

    let strength = one_level(LevelPolicy::Strength);
    let archive = Ttx3Archive::parse(&strength).expect("单级 Strength 应解析");
    assert_eq!(archive.verify().unwrap(), 1);
}

#[test]
fn zero_length_entry_is_rejected() {
    let mut bytes = one_level(LevelPolicy::Capacity);
    let entry = tile_entry_at(&bytes, 0, 0);
    write_u32(&mut bytes, entry + 16, 0);

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(
            error,
            TextureError::Ttx3Tile {
                level: 0,
                tx: 0,
                ty: 0,
                ..
            }
        ),
        "零长度条目应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn u32_max_offset_and_len_are_rejected() {
    let mut offset = one_level(LevelPolicy::Capacity);
    let entry = tile_entry_at(&offset, 0, 0);
    write_u64(&mut offset, entry, u64::from(u32::MAX));
    let error = Ttx3Archive::parse(&offset).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Truncated { .. }),
        "u32::MAX 偏移越界应拒绝为 Ttx3Truncated，实际 {error:?}"
    );

    let mut len = one_level(LevelPolicy::Capacity);
    let entry = tile_entry_at(&len, 0, 0);
    write_u32(&mut len, entry + 16, u32::MAX);
    let error = Ttx3Archive::parse(&len).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "u32::MAX 长度应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn single_level_chain_is_accepted() {
    for policy in [LevelPolicy::Capacity, LevelPolicy::Strength] {
        let bytes = one_level(policy);
        let archive =
            Ttx3Archive::parse(&bytes).unwrap_or_else(|e| panic!("{policy:?} 单级链应解析：{e}"));
        assert_eq!(archive.verify().unwrap(), 1, "{policy:?}");
    }
}

#[test]
fn level_count_zero_is_rejected() {
    let mut bytes = one_level(LevelPolicy::Capacity);
    bytes[8] = 0;
    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Level { .. }),
        "level_count=0 应拒绝为 Ttx3Level，实际 {error:?}"
    );
}

#[test]
fn capacity_nonzero_off2_is_rejected() {
    // 物理副本数：Capacity 声明 1 份，挂非零 off2 = 账务/布局不一致。
    let mut bytes = one_level(LevelPolicy::Capacity);
    let entry = tile_entry_at(&bytes, 0, 0);
    let off1 = read_u64(&bytes, entry);
    write_u64(&mut bytes, entry + 8, off1);

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "Capacity 挂非零 off2 应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn strength_zero_off2_is_rejected() {
    // 物理副本数：Strength 声明 2 份，off2=0 实际只有 1 份。
    let mut bytes = one_level(LevelPolicy::Strength);
    let entry = tile_entry_at(&bytes, 0, 0);
    write_u64(&mut bytes, entry + 8, 0);

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "Strength 缺第二副本偏移应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn policy_flip_capacity_to_strength_is_rejected() {
    // 干净 Capacity 档案政策字节翻成 Strength：磁盘仍单副本，档位/布局脱节。
    let mut bytes = one_level(LevelPolicy::Capacity);
    bytes[HEADER_BYTES + 8] = 1;

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "Cap→Str 翻转应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn policy_flip_strength_to_capacity_is_rejected() {
    // 干净 Strength 档案政策字节翻成 Capacity：磁盘仍留第二副本区。
    let mut bytes = one_level(LevelPolicy::Strength);
    bytes[HEADER_BYTES + 8] = 0;

    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Tile { .. }),
        "Str→Cap 翻转应拒绝为 Ttx3Tile，实际 {error:?}"
    );
}

#[test]
fn unknown_policy_byte_is_rejected() {
    let mut bytes = one_level(LevelPolicy::Capacity);
    bytes[HEADER_BYTES + 8] = 2;
    let error = Ttx3Archive::parse(&bytes).unwrap_err();
    assert!(
        matches!(error, TextureError::Ttx3Level { level: 2, .. }),
        "未知政策字节应拒绝为 Ttx3Level，实际 {error:?}"
    );
}

#[test]
fn tile_data_bytes_constant_is_stable() {
    assert_eq!(TILE_DATA_BYTES, 21_760);
}
