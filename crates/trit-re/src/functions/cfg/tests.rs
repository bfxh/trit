//! `cfg` 的单元测试：区间收割、精确区间优先与溢出边界。

use super::*;
use crate::functions::{FuncSource, Function};
use std::collections::BTreeSet;

fn exact(start: u64, end: u64) -> Function {
    Function {
        start,
        end,
        name: None,
        source: FuncSource::Pdata,
    }
}

fn section(bytes: &[u8], base: u64) -> Vec<(u64, &[u8])> {
    vec![(base, bytes)]
}

fn e8(from_rel: u64, to: u64) -> [u8; 5] {
    // rel32 = to - (from + 5)
    let rel = (to as i64 - (from_rel as i64 + 5)) as i32;
    let mut out = [0xE8, 0, 0, 0, 0];
    out[1..].copy_from_slice(&rel.to_le_bytes());
    out
}

#[test]
fn no_exact_source_covers_whole_exec_like_old_heuristic() {
    // 入口 0x1000 + E8 目标 0x1030 → 两条 Cfg 区间平铺到节末
    let mut bytes = vec![0x90u8; 0x40];
    bytes[0x10..0x15].copy_from_slice(&e8(0x10, 0x30));
    let ranges = cfg_ranges(&section(&bytes, 0x1000), &[], 0x1000, super::Arch::X86, &[]);
    let starts: Vec<u64> = ranges.iter().map(|r| r.start).collect();
    assert_eq!(starts, vec![0x1000, 0x1030], "入口 + E8 目标都成起点");
    assert!(
        ranges.iter().all(|r| r.source == FuncSource::Cfg),
        "无精确源全 Cfg"
    );
    assert_eq!(ranges.last().unwrap().end, 0x1040, "末区间到节末");
}

#[test]
fn exact_ranges_are_never_split_by_cfg_candidates() {
    // 0x1060 落在精确区间内丢弃；0x1210 在隙内照收，两条都要成立。
    let mut bytes = vec![0x90u8; 0x300];
    bytes[0x50..0x55].copy_from_slice(&e8(0x50, 0x60)); // 目标 0x1060 在精确区间内
    bytes[0x200..0x205].copy_from_slice(&e8(0x200, 0x210)); // 目标 0x1210 在隙内
    let exacts = vec![exact(0x1000, 0x1100)];
    let ranges = cfg_ranges(
        &section(&bytes, 0x1000),
        &exacts,
        0x1000,
        super::Arch::X86,
        &[],
    );
    assert!(
        ranges.iter().all(|r| r.start >= 0x1100),
        "不得进入精确区间内部"
    );
    assert_eq!(
        ranges.iter().map(|r| r.start).collect::<Vec<_>>(),
        vec![0x1210]
    );
}

#[test]
fn cfg_fills_gaps_between_exact_ranges() {
    // 精确区间 [0x1000,0x1100)：E8 目标 0x1210 在隙外扫描到 → 补隙。
    let mut bytes = vec![0x90u8; 0x300];
    bytes[0x200..0x205].copy_from_slice(&e8(0x200, 0x210));
    let exacts = vec![exact(0x1000, 0x1100)];
    let ranges = cfg_ranges(
        &section(&bytes, 0x1000),
        &exacts,
        0x1000,
        super::Arch::X86,
        &[],
    );
    assert_eq!(ranges.len(), 1, "只补一条");
    assert_eq!(ranges[0].start, 0x1210, "E8 目标成补隙起点");
    assert_eq!(ranges[0].end, 0x1300, "平铺到节末");
    assert_eq!(ranges[0].source, FuncSource::Cfg);
}

#[test]
fn entry_inside_gap_becomes_start_entry_inside_exact_is_dropped() {
    // 入口在精确区间内不立起点；在隙内立起点。
    let bytes = vec![0x90u8; 0x40];
    let exacts = vec![exact(0x1010, 0x1020)];
    let ranges = cfg_ranges(
        &section(&bytes, 0x1000),
        &exacts,
        0x1014,
        super::Arch::X86,
        &[],
    );
    assert!(
        !ranges.iter().any(|r| r.start == 0x1014),
        "区间内入口不立起点"
    );
    let exacts2 = vec![exact(0x1010, 0x1020)];
    let ranges2 = cfg_ranges(
        &section(&bytes, 0x1000),
        &exacts2,
        0x1030,
        super::Arch::X86,
        &[],
    );
    assert!(ranges2.iter().any(|r| r.start == 0x1030), "隙内入口立起点");
}

#[test]
fn arm64_bl_targets_seed_starts() {
    // BL 0x14000000 | imm26 → 目标 0x1010：4 字节步进，目标在节内。
    let mut bytes = vec![0x00u8; 0x40];
    let word: u32 = 0x9400_0000 | ((0x10 >> 2) & 0x03FF_FFFF);
    bytes[0..4].copy_from_slice(&word.to_le_bytes());
    let ranges = cfg_ranges(&section(&bytes, 0x1000), &[], 0, super::Arch::Arm64, &[]);
    assert!(ranges.iter().any(|r| r.start == 0x1010), "BL 目标成起点");
}

#[test]
fn addr_taken_seeds_indirectly_called_leaf() {
    // lea rax,[rip→0x1040] + call rax + ret；叶子 0x1040 无 E8 指向，取址信号捡回。
    let mut bytes = vec![0x90u8; 0x60];
    let rel = (0x1040i64 - 0x1007) as i32;
    bytes[0..3].copy_from_slice(&[0x48, 0x8D, 0x05]);
    bytes[3..7].copy_from_slice(&rel.to_le_bytes());
    bytes[7..10].copy_from_slice(&[0xFF, 0xD0, 0xC3]); // call rax（间接）+ ret
    bytes[0x40] = 0xFF; // call [rip+0]（IAT 形态）
    bytes[0x41] = 0x15;
    bytes[0x42..0x46].copy_from_slice(&0i32.to_le_bytes());
    bytes[0x46] = 0xC3;
    let ranges = cfg_ranges(&section(&bytes, 0x1000), &[], 0x1000, super::Arch::X86, &[]);
    let starts: Vec<u64> = ranges.iter().map(|r| r.start).collect();
    assert!(starts.contains(&0x1000), "入口照常是起点");
    assert!(starts.contains(&0x1040), "取址目标成起点");
    assert_eq!(starts.len(), 2, "无多余候选");
}

#[test]
fn pointer_table_seeds_start_vtable_functions() {
    // 指针表候选喂给 cfg_ranges → vtable 叶子 A/B 长出起点；不喂种子不可见。
    let mut bytes = vec![0x90u8; 0x80];
    bytes[0x10] = 0xC3;
    bytes[0x30] = 0xC3;
    let exec = vec![(0x1000u64, bytes.as_slice())];
    let seeds = [0x1010u64, 0x1030u64];
    let ranges = cfg_ranges(&exec, &[], 0x1000, super::Arch::X86, &seeds);
    let starts: Vec<u64> = ranges.iter().map(|r| r.start).collect();
    assert!(starts.contains(&0x1010), "叶子 A 成起点");
    assert!(starts.contains(&0x1030), "叶子 B 成起点");
    let plain = cfg_ranges(&exec, &[], 0x1000, super::Arch::X86, &[]);
    assert!(
        !plain.iter().any(|r| r.start == 0x1010 || r.start == 0x1030),
        "无种子不可见"
    );
}

#[test]
fn edges_from_near_u64_max_are_skipped_not_panicking() {
    // 节基址贴近 u64::MAX：E8 在 index 1，`base + index` 溢出必须跳过。
    let mut bytes = vec![0x90u8; 6];
    bytes[1] = 0xE8;
    assert!(harvest_edges(u64::MAX - 3, &bytes, super::Arch::X86).is_empty());
    assert!(harvest_edges(u64::MAX, &bytes, super::Arch::X86).is_empty());
    let mut arm = vec![0x00u8; 8];
    arm[4..8].copy_from_slice(&0x9400_0000u32.to_le_bytes());
    assert!(harvest_edges(u64::MAX - 3, &arm, super::Arch::Arm64).is_empty());
}

#[test]
fn walk_near_max_base_does_not_wrap() {
    // 近上限基址 + nop 流：`cur + len` 不可表示即停链，不回绕成低地址。
    let base = u64::MAX - 15;
    let seeds = BTreeSet::from([base]);
    let out = walk(&section(&[0x90u8; 0x10], base), &seeds);
    assert!(out.visited.contains_key(&base));
    assert!(
        out.visited.keys().all(|&a| a >= base),
        "遍历步进回绕到低地址"
    );
}
