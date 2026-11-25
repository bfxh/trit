//! 各架构 ELF 扫描共用的 xref 报告构造（从 mod.rs 拆出，控制单文件行数预算）。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::value::Ternary;

use crate::elf;

use super::{ImportXref, RawSite, XrefReport, finalize, to_sites};

pub(crate) fn build_elf_report(
    arch: &str,
    symbols: &[elf::DynamicSymbol],
    jump_slots: &[(u64, usize)],
    sites: &BTreeMap<usize, BTreeSet<RawSite>>,
    complete: bool,
    scanned: usize,
) -> XrefReport {
    let mut rows = Vec::new();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for (got, symbol) in jump_slots {
        let Some(symbol_info) = symbols.iter().find(|entry| entry.index == *symbol) else {
            continue;
        };
        // 只保留"外部导入"：本文件已定义的符号（如 libc 自己的 malloc/free）虽有 JUMP_SLOT，
        // 那是内部 PLT 调用，不是导入。
        if symbol_info.defined || !matches!(symbol_info.kind, 0 | 2 | 10) {
            continue;
        }
        if !seen.insert(*symbol) {
            continue;
        }
        let mut row = ImportXref {
            import: symbol_info.name.clone(),
            slot: *got,
            sites: sites.get(symbol).map(to_sites).unwrap_or_default(),
            status: Ternary::Unknown,
            note: String::new(),
        };
        finalize(&mut row, complete);
        rows.push(row);
    }
    for symbol in symbols
        .iter()
        .filter(|entry| !entry.defined && matches!(entry.kind, 0 | 2 | 10))
    {
        if !seen.contains(&symbol.index) {
            rows.push(ImportXref {
                import: symbol.name.clone(),
                slot: 0,
                sites: Vec::new(),
                // 没有槽位就没有证据："没有重定位"不等于"没有调用点"。
                status: Ternary::Unknown,
                note: "没有 GOT 槽位（可能是动态解析或弱符号）".to_string(),
            });
        }
    }

    let mut report = XrefReport {
        arch: arch.to_string(),
        scanned_bytes: scanned,
        complete,
        imports: rows,
        notes: Vec::new(),
    };
    if !complete {
        report
            .notes
            .push("有可执行节读不出或超过扫描上限，交叉引用不完整".to_string());
    }
    report
}
