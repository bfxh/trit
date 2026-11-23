use super::*;
use crate::elf::ElfSection;

fn section(addr: u64, name: &str) -> ElfSection {
    ElfSection {
        name: name.into(),
        sh_type: 1,
        flags: 6,
        addr,
        offset: 0,
        size: 0,
        link: 0,
        entsize: 0,
        entropy: None,
    }
}

#[test]
fn pc_thunk_and_add_give_the_got_base() {
    // 0x1000: call 0x1010（thunk）；0x1005: add $0x2000,%ebx ⇒ base = 0x1005 + 0x2000。
    let text = section(0x1000, ".text");
    let mut data = vec![0u8; 0x14];
    data[0] = 0xE8;
    data[1..5].copy_from_slice(&0x0Bu32.to_le_bytes());
    data[5] = 0x81;
    data[6] = 0xC3;
    data[7..11].copy_from_slice(&0x2000u32.to_le_bytes());
    data[0x10..0x14].copy_from_slice(&[0x8B, 0x1C, 0x24, 0xC3]);
    let exec = vec![(&text, data.as_slice())];
    assert_eq!(
        find_got_bases(&exec),
        BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))])
    );
}

#[test]
fn pc_thunk_ax_uses_the_short_add_encoding() {
    // `call __x86.get_pc_thunk.ax` 之后是 eax 的短编码 `05 imm32`（没有 81 C0）。
    let text = section(0x1000, ".text");
    let mut data = vec![0u8; 0x14];
    data[0] = 0xE8;
    data[1..5].copy_from_slice(&0x0Bu32.to_le_bytes());
    data[5] = 0x05;
    data[6..10].copy_from_slice(&0x2000u32.to_le_bytes());
    data[0x10..0x14].copy_from_slice(&[0x8B, 0x04, 0x24, 0xC3]);
    let exec = vec![(&text, data.as_slice())];
    assert_eq!(
        find_got_bases(&exec),
        BTreeMap::from([(0u8, BTreeSet::from([0x3005u64]))])
    );
}

#[test]
fn eax_relative_got_access_is_recognised() {
    // 基址在 EAX（get_pc_thunk.ax），槽位 0x3010 = 0x3005 + 0xB。
    let bases = BTreeMap::from([(0u8, BTreeSet::from([0x3005u64]))]);
    let got = BTreeMap::from([(0x3010u64, 1usize)]);
    // mov ecx, [eax+disp32]（8B 88）与 call *disp32(%eax)（FF 90）。
    let mut data = vec![0x8B, 0x88];
    data.extend_from_slice(&0x0Bu32.to_le_bytes());
    assert_eq!(taken_operand(&bases, &got, &data, 0), Some(0x3010));
    let mut data = vec![0xFF, 0x90];
    data.extend_from_slice(&0x0Bu32.to_le_bytes());
    assert_eq!(
        got_operand(&bases, &got, &data, 0),
        Some((0x3010, SiteKind::Call))
    );
    // 反例：EBX 的基址表不该被 EAX 的位移命中。
    let bases_ebx = BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))]);
    let mut data = vec![0x8B, 0x88];
    data.extend_from_slice(&0x0Bu32.to_le_bytes());
    assert_eq!(taken_operand(&bases_ebx, &got, &data, 0), None);
}

#[test]
fn resolve_base_only_accepts_known_slots() {
    let bases = BTreeMap::from([(3u8, BTreeSet::from([0x3FCCu64]))]);
    let got = BTreeMap::from([(0x3FD8u64, 1usize)]);
    assert_eq!(resolve_base(&bases, &got, 3, 0xC), Some(0x3FD8));
    assert_eq!(resolve_base(&bases, &got, 3, 0x10), None);
    assert_eq!(resolve_base(&bases, &got, 0, 0xC), None);
    assert_eq!(resolve_base(&BTreeMap::new(), &got, 3, 0xC), None);
}

#[test]
fn taken_operand_recognises_read_only_forms() {
    // 槽位 0x3010，基址 0x3005 ⇒ EBX 相对位移 0xB。
    let got = BTreeMap::from([(0x3010u64, 1usize)]);
    let bases = BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))]);
    let mut cases: Vec<(u8, u8, [u8; 4], bool)> = vec![
        (0x8B, 0x83, 0x0Bu32.to_le_bytes(), true), // mov reg, [ebx+disp]
        (0x8D, 0x83, 0x0Bu32.to_le_bytes(), true), // lea reg, [ebx+disp]
        (0x39, 0x83, 0x0Bu32.to_le_bytes(), true), // cmp [ebx+disp], reg
        (0x3B, 0x83, 0x0Bu32.to_le_bytes(), true), // cmp reg, [ebx+disp]
        (0x85, 0x83, 0x0Bu32.to_le_bytes(), true), // test [ebx+disp], reg
        (0xFF, 0xB3, 0x0Bu32.to_le_bytes(), true), // push [ebx+disp]
        (0x8B, 0x83, 0x10u32.to_le_bytes(), false), // 不是槽位
        (0x8B, 0x84, 0x0Bu32.to_le_bytes(), false), // mov 但不是 [ebx+disp32]
    ];
    for (first, second, disp, expected) in cases.drain(..) {
        let mut data = vec![first, second];
        data.extend_from_slice(&disp);
        let found = taken_operand(&bases, &got, &data, 0);
        assert_eq!(found.is_some(), expected, "{first:#x} {second:#x}");
    }
    // 绝对寻址：mov/cmp/test reg, [abs32]。
    for (first, second) in [(0x8B, 0x05), (0x3B, 0x05), (0x85, 0x05)] {
        let mut data = vec![first, second];
        data.extend_from_slice(&0x3010u32.to_le_bytes());
        assert_eq!(
            taken_operand(&bases, &got, &data, 0),
            Some(0x3010),
            "{first:#x} {second:#x}"
        );
    }
    // `A1` 是 `mov eax, moffs32`：地址紧跟操作码。
    let mut data = vec![0xA1];
    data.extend_from_slice(&0x3010u32.to_le_bytes());
    assert_eq!(taken_operand(&bases, &got, &data, 0), Some(0x3010));
}

#[test]
fn byte_compare_and_immediate_forms_count_as_taken() {
    // 槽位 0x3010 = 基址 0x3005 + 0xB。
    let got = BTreeMap::from([(0x3010u64, 1usize)]);
    let bases = BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))]);

    // cmp r/m8|32, imm：`80/81/83 BB disp32 imm…`（EBX 相对）。
    for opcode in [0x80u8, 0x81, 0x83] {
        let mut data = vec![opcode, 0xBB];
        data.extend_from_slice(&0x0Bu32.to_le_bytes());
        data.extend_from_slice(&[0, 0, 0, 0]); // 立即数（不参与判定）
        assert_eq!(
            taken_operand(&bases, &got, &data, 0),
            Some(0x3010),
            "cmp ebx+disp, {opcode:#x}"
        );
    }
    // 同一组指令的绝对形式 `disp32`。
    for opcode in [0x80u8, 0x81, 0x83] {
        let mut data = vec![opcode, 0x3D];
        data.extend_from_slice(&0x3010u32.to_le_bytes());
        data.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(
            taken_operand(&bases, &got, &data, 0),
            Some(0x3010),
            "cmp abs, {opcode:#x}"
        );
    }
    // mov r32, imm32：B8..BF，立即数在 +1。
    for register in 0xB8u8..=0xBF {
        let mut data = vec![register];
        data.extend_from_slice(&0x3010u32.to_le_bytes());
        assert_eq!(
            taken_operand(&bases, &got, &data, 0),
            Some(0x3010),
            "mov {register:#x}, imm32"
        );
    }
    // 反例：位移/立即数不是槽位。
    let mut data = vec![0x83, 0xBB];
    data.extend_from_slice(&0x10u32.to_le_bytes());
    data.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(taken_operand(&bases, &got, &data, 0), None);
    let mut data = vec![0xB8];
    data.extend_from_slice(&0x3014u32.to_le_bytes());
    assert_eq!(taken_operand(&bases, &got, &data, 0), None);
}

#[test]
fn immediate_reference_reads_each_encoding() {
    let immediate = 0x08048160u32.to_le_bytes();
    // `push imm32` 与 `mov r32, imm32`（B8..BF）：立即数紧跟操作码。
    for opcode in [0x68u8, 0xB8, 0xBC, 0xBF] {
        let mut data = vec![opcode; 5];
        data[1..5].copy_from_slice(&immediate);
        assert_eq!(
            immediate_reference(&data, 0),
            Some(0x08048160),
            "{opcode:#x}"
        );
    }
    // `C7 /0` 的三种内存形式：立即数在 modrm + 寻址之后。
    let cases: Vec<([u8; 11], usize)> = vec![
        ([0xC7, 0x45, 0xF4, 0, 0, 0, 0, 0, 0, 0, 0], 3), // [ebp+disp8]
        ([0xC7, 0x85, 0, 0, 0, 0, 0, 0, 0, 0, 0], 6),    // [ebp+disp32]
        ([0xC7, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0], 6),    // [abs32]
    ];
    for (mut data, at) in cases {
        data[at..at + 4].copy_from_slice(&immediate);
        assert_eq!(
            immediate_reference(&data, 0),
            Some(0x08048160),
            "C7 at {at}"
        );
    }
    // 不认识的编码与越界。
    assert_eq!(immediate_reference(&[0x90; 6], 0), None);
    assert_eq!(immediate_reference(&[0xC7, 0x83, 0, 0, 0, 0, 0], 0), None);
    assert_eq!(immediate_reference(&[0xB8, 0, 0], 0), None);
}

#[test]
fn pic_stub_without_base_marks_the_scan_incomplete() {
    let text = section(0x1000, ".text");
    let plt = section(0x2000, ".plt");
    let mut text_data = vec![0u8; 8];
    text_data[0] = 0xE8;
    text_data[1..5].copy_from_slice(&0x0u32.to_le_bytes());
    let mut plt_data = vec![0u8; 8];
    plt_data[0] = 0xFF;
    plt_data[1] = 0xA3;
    let exec = vec![(&text, text_data.as_slice()), (&plt, plt_data.as_slice())];
    let symbols = vec![elf::DynamicSymbol {
        index: 1,
        name: "fopen".into(),
        defined: false,
        kind: 2,
    }];
    let jump_slots = vec![(0x3000u64, 1usize)];
    let report = scan_elf("x86", &symbols, &jump_slots, &exec, true, 16);
    assert!(!report.complete);
    assert_eq!(report.imports.len(), 1);
    assert_eq!(report.imports[0].status, trit_core::value::Ternary::Unknown);
}

#[test]
fn pic_stub_with_resolved_base_finds_the_caller() {
    // 0x1000: call thunk（0x1010）；0x1005: add $0x2000,%ebx ⇒ base 0x3005；槽位 0x3005+0xB = 0x3010。
    // 0x100B: call 0x1020（PLT 桩）；0x1020: jmp *0xB(%ebx)。
    let text = section(0x1000, ".text");
    let plt = section(0x1020, ".plt");
    let mut text_data = vec![0u8; 0x10];
    text_data[0] = 0xE8;
    text_data[1..5].copy_from_slice(&0x0Bu32.to_le_bytes());
    text_data[5] = 0x81;
    text_data[6] = 0xC3;
    text_data[7..11].copy_from_slice(&0x2000u32.to_le_bytes());
    text_data[0x0B] = 0xE8;
    text_data[0x0C..0x10].copy_from_slice(&0x10u32.to_le_bytes());
    text_data.extend_from_slice(&[0x8B, 0x1C, 0x24, 0xC3]);
    let mut plt_data = vec![0u8; 8];
    plt_data[0] = 0xFF;
    plt_data[1] = 0xA3;
    plt_data[2..6].copy_from_slice(&0xBu32.to_le_bytes());
    let exec = vec![(&text, text_data.as_slice()), (&plt, plt_data.as_slice())];
    let symbols = vec![elf::DynamicSymbol {
        index: 1,
        name: "fopen".into(),
        defined: false,
        kind: 2,
    }];
    let jump_slots = vec![(0x3010u64, 1usize)];
    let report = scan_elf("x86", &symbols, &jump_slots, &exec, true, 24);
    assert!(report.complete);
    assert_eq!(report.imports.len(), 1);
    assert_eq!(report.imports[0].status, trit_core::value::Ternary::True);
    assert_eq!(report.imports[0].sites.len(), 1);
    assert_eq!(report.imports[0].sites[0].addr, 0x100B);
}
