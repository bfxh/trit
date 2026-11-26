//! ARM64 地址算术溢出回归（切片 FIX-R / trit-re）：节基址近 `u64::MAX`、
//! 节尾最后一条指令、`cur + 4` 跨出节尾、空节、`size = 0`、`u64::MAX` 偏移。
//!
//! 样本全部在 Rust 内联用 byte vec 构造（基于 `support::elf_arm64_sample`
//! 的已知可用 dynsym/rela 三件套，仅改 .text 节头地址与入口），不依赖外部
//! 文件；断言落到具体的 `xref.complete` 与注记类别，不只判 `is_err()`。

mod support;

use trit_core::value::Ternary;
use trit_re::analyze;

/// `.text` 节头在合成 ELF 里的偏移（shoff=0x500，第 1 个节，64 字节/节）。
const TEXT_SHDR: usize = 0x540;
/// 节头里 `sh_addr` 字段的偏移。
const SH_ADDR: usize = 16;

/// 在已知可用的 ARM64 合成样本上，把 `.text` 节地址改成 `base`、入口改成 `entry`。
fn arm64_sample_with_text(base: u64, entry: u64) -> Vec<u8> {
    let mut file = support::elf_arm64_sample();
    support::put_u64(&mut file, TEXT_SHDR + SH_ADDR, base);
    support::put_u64(&mut file, 24, entry);
    file
}

#[test]
fn near_max_section_base_downgrades_xref_complete() {
    // F1：节基址近 u64::MAX，「基址 + 下标*4」求和溢出。
    // 溢出指令跳过，扫描必须降级为不完整；不得回绕出假调用点、不得判 False。
    let base = u64::MAX - 8;
    let bytes = arm64_sample_with_text(base, 0);
    let triage = analyze(&bytes);
    let xref = triage.xref.expect("ARM64 ELF 应有 xref 报告");
    assert!(!xref.complete, "近上限基址溢出时扫描必须标记不完整");
    assert!(
        xref.notes.iter().any(|note| note.contains("不完整")),
        "降级扫描应带不完整注记，实际：{:?}",
        xref.notes
    );
    for row in &xref.imports {
        assert_ne!(
            row.status,
            Ternary::False,
            "不完整扫描不得把导入判成无调用点"
        );
    }
}

#[test]
fn walk_at_near_max_base_does_not_wrap_addresses() {
    // F2：CFG 遍历在节尾步进；近上限基址下 `cur + 4` 不得回绕成低地址。
    let base = u64::MAX - 15;
    let bytes = arm64_sample_with_text(base, base);
    let triage = analyze(&bytes);
    let functions = triage.functions.expect("应有函数报告");
    for function in &functions.functions {
        assert!(
            function.function.start >= base,
            "步进回绕：函数起点 0x{:x} 落回低地址（节基址 0x{base:x}）",
            function.function.start
        );
    }
}

#[test]
fn last_instruction_falls_off_section_tail() {
    // 节尾最后一条指令之后 `cur + 4` 恰好跨出节尾：正常基址下停链，
    // 不 panic、不误标溢出。
    let base = 0x100_000;
    let bytes = arm64_sample_with_text(base, base);
    let triage = analyze(&bytes);
    let xref = triage.xref.expect("应有 xref 报告");
    assert!(xref.complete, "正常基址不应触发溢出降级");
    let functions = triage.functions.expect("应有函数报告");
    for function in &functions.functions {
        assert!(
            function.function.start >= base && function.function.start < base + 0x200,
            "函数起点越出节区间：0x{:x}",
            function.function.start
        );
    }
}

#[test]
fn empty_exec_section_does_not_panic_or_mark_overflow() {
    // 空节 / `size = 0`：可执行节被 `size > 0` 过滤，不进入指令扫描；
    // 不 panic，也不冒充地址溢出。
    let mut file = support::elf_arm64_sample();
    support::put_u64(&mut file, TEXT_SHDR + 32, 0); // .text sh_size = 0
    let triage = analyze(&file);
    let Some(xref) = triage.xref else {
        return;
    };
    assert!(xref.complete, "空节不是地址溢出");
    assert!(
        xref.notes.iter().all(|note| !note.contains("不完整")),
        "空节不应记录不完整注记，实际：{:?}",
        xref.notes
    );
}

#[test]
fn u64_max_section_address_marks_incomplete() {
    // 节地址 = `u64::MAX`：下标偏移求和溢出，`complete` 必须降级。
    let bytes = arm64_sample_with_text(u64::MAX, 0);
    let triage = analyze(&bytes);
    let xref = triage.xref.expect("应有 xref 报告");
    assert!(!xref.complete, "节地址 = u64::MAX 时扫描必须标记不完整");
    assert!(xref.notes.iter().any(|note| note.contains("不完整")));
}

#[test]
fn normal_arm64_sample_stays_complete() {
    // 对照：合法合成样本不被误降级。
    let bytes = support::elf_arm64_sample();
    let triage = analyze(&bytes);
    let xref = triage.xref.expect("应有 xref 报告");
    assert!(xref.complete, "合法样本扫描应保持完整");
}
