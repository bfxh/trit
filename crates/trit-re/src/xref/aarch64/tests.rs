use super::{SiteKind, adrp_page, branch_target, ldr_imm, references};

fn adrp(pc: u64, target: u64, register: u32) -> u32 {
    let delta = ((target & !0xFFF) as i64 - (pc & !0xFFF) as i64) >> 12;
    let imm = delta as u32 & 0x1F_FFFF;
    let immlo = (imm & 0x3) << 29;
    let immhi = ((imm >> 2) & 0x7_FFFF) << 5;
    0x9000_0000 | immlo | immhi | register
}

fn ldr(register: u32, offset: u64, target: u32) -> u32 {
    assert_eq!(offset % 8, 0);
    0xF940_0000 | (((offset / 8) as u32) << 10) | (register << 5) | target
}

fn add(register: u32, offset: u64, target: u32) -> u32 {
    0x9100_0000 | ((offset as u32) << 10) | (register << 5) | target
}

fn blr(register: u32) -> u32 {
    0xD63F_0000 | (register << 5)
}

fn br(register: u32) -> u32 {
    0xD61F_0000 | (register << 5)
}

fn bl(pc: u64, target: u64) -> u32 {
    let delta = ((target as i64 - pc as i64) >> 2) as u32 & 0x03FF_FFFF;
    0x9400_0000 | delta
}

fn put(data: &mut [u8], index: usize, insn: u32) {
    data[index * 4..index * 4 + 4].copy_from_slice(&insn.to_le_bytes());
}

#[test]
fn decodes_adrp_page() {
    // 0x1000 → 0x2000
    assert_eq!(adrp_page(adrp(0x1000, 0x2000, 9), 0x1000), Some(0x2000));
    // 负方向
    let insn = adrp(0x2000, 0x1000, 3);
    assert_eq!(adrp_page(insn, 0x2000), Some(0x1000));
    assert_eq!(adrp_page(0xDEAD_BEEF, 0x1000), None);
}

#[test]
fn finds_direct_blr_through_slot() {
    let mut data = vec![0u8; 16];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, ldr(9, 0x40, 9));
    put(&mut data, 2, blr(9));
    let (refs, _) = references(&data, 0x1000);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slot, 0x2040);
    assert_eq!(refs[0].site, 0x1008);
    assert_eq!(refs[0].kind, SiteKind::Call);
}

#[test]
fn follows_add_then_ldr() {
    let mut data = vec![0u8; 16];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, add(9, 0x48, 9));
    put(&mut data, 2, ldr(9, 0x0, 9));
    put(&mut data, 3, blr(9));
    let (refs, _) = references(&data, 0x1000);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slot, 0x2048);
    assert_eq!(refs[0].site, 0x100c);
}

#[test]
fn interleaved_instructions_do_not_hide_the_reference() {
    let mut data = vec![0u8; 40];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, 0xD2800020); // movz x0, #1
    put(&mut data, 2, ldr(9, 0x40, 9));
    put(&mut data, 3, 0xD2800041); // movz x1, #2
    put(&mut data, 4, 0xD2800062); // movz x2, #3
    put(&mut data, 5, blr(9));
    let (refs, _) = references(&data, 0x1000);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slot, 0x2040);
    assert_eq!(refs[0].site, 0x1014);
}

fn mov(dst: u32, src: u32) -> u32 {
    0xAA00_03E0 | (src << 16) | dst
}

#[test]
fn mov_alias_carries_the_address() {
    let mut data = vec![0u8; 20];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, mov(10, 9));
    put(&mut data, 2, ldr(10, 0x40, 10));
    put(&mut data, 3, 0xD503201F); // nop
    put(&mut data, 4, blr(10));
    let (refs, _) = references(&data, 0x1000);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slot, 0x2040);
    assert_eq!(refs[0].site, 0x1010);
}

#[test]
fn mov_alias_carries_the_loaded_pointer() {
    let mut data = vec![0u8; 20];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, ldr(9, 0x40, 9));
    put(&mut data, 2, mov(10, 9));
    put(&mut data, 3, 0xD503201F); // nop
    put(&mut data, 4, blr(10));
    let (refs, _) = references(&data, 0x1000);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slot, 0x2040);
}

#[test]
fn clobbered_register_breaks_the_chain() {
    let mut data = vec![0u8; 16];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, 0xD2800009); // movz x9, #0
    put(&mut data, 2, ldr(9, 0x40, 9));
    put(&mut data, 3, blr(9));
    let (refs, _) = references(&data, 0x1000);
    assert!(refs.is_empty());
}

#[test]
fn address_only_reference_is_marked_taken() {
    let mut data = vec![0u8; 24];
    put(&mut data, 0, adrp(0x1000, 0x2000, 9));
    put(&mut data, 1, ldr(9, 0x40, 9));
    let (refs, taken) = references(&data, 0x1000);
    assert!(refs.is_empty());
    assert!(taken.contains(&0x2040));
}

#[test]
fn thunk_is_a_jump_at_adrp() {
    let mut data = vec![0u8; 12];
    put(&mut data, 0, adrp(0x1020, 0x2000, 16));
    put(&mut data, 1, ldr(16, 0x40, 17));
    put(&mut data, 2, br(17));
    let (refs, _) = references(&data, 0x1020);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].kind, SiteKind::Jump);
    assert_eq!(refs[0].site, 0x1020);
    assert_eq!(refs[0].slot, 0x2040);
}

#[test]
fn branch_targets_are_signed() {
    let insn = bl(0x2000, 0x1000);
    let (target, is_call) = branch_target(insn, 0x2000).unwrap();
    assert!(is_call);
    assert_eq!(target, 0x1000);
    assert_eq!(ldr_imm(ldr(9, 0x40, 9), 9), Some((9, 0x40)));
}
