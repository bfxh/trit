//! `eh_frame` checked 求和的回归（从 tests.rs 拆出，控制单文件行数预算）。

use super::super::{ElfSection, read_eh_frame};

fn eh_frame_fixture() -> Vec<u8> {
    let mut bytes = vec![0u8; 0x40];
    let put32 = |bytes: &mut [u8], off: usize, value: u32| {
        bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
    };
    put32(&mut bytes, 0, 13);
    put32(&mut bytes, 4, 0);
    bytes[8] = 1;
    bytes[9] = b'z';
    bytes[10] = b'R';
    bytes[11] = 0;
    bytes[12] = 1;
    bytes[13] = 0x78;
    bytes[14] = 30;
    bytes[15] = 1;
    bytes[16] = 0x1b;
    put32(&mut bytes, 17, 12);
    put32(&mut bytes, 21, 21);
    put32(&mut bytes, 25, 0xe7);
    put32(&mut bytes, 29, 0x40);
    bytes
}

fn eh_section(addr: u64, size: u64) -> ElfSection {
    ElfSection {
        name: ".eh_frame".into(),
        sh_type: 1,
        flags: 2,
        addr,
        offset: 0,
        size,
        link: 0,
        entsize: 0,
        entropy: None,
    }
}

#[test]
fn eh_frame_overflowing_section_address_is_skipped_not_panic() {
    let bytes = eh_frame_fixture();
    // 极端 1：节地址近上限，field_addr = addr + 25 回绕 → 跳过该 FDE。
    let ranges = read_eh_frame(&bytes, &[eh_section(u64::MAX - 0x10, 0x30)], true);
    assert!(ranges.is_empty());
    // 极端 2：节地址 = MAX，下标 0 即溢出。
    let ranges = read_eh_frame(&bytes, &[eh_section(u64::MAX, 0x30)], true);
    assert!(ranges.is_empty());
    // 极端 3：零长度节体直接不出区间。
    let ranges = read_eh_frame(&bytes, &[eh_section(0x1000, 0)], true);
    assert!(ranges.is_empty());
}

#[test]
fn eh_frame_legal_fde_range_is_unchanged() {
    let bytes = eh_frame_fixture();
    let ranges = read_eh_frame(&bytes, &[eh_section(0x1000, 0x30)], true);
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start, 0x1100);
    assert_eq!(ranges[0].size, 0x40);
}
