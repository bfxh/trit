//! 函数级归属与能力共现。
//!
//! 交叉引用回答"这个导入有没有被调用"；本模块回答"**在哪个函数里被调用**"，
//! 并进一步问"同一函数里是否同时出现了一组能力"（注入三件套、装载+网络、凭据+外发）。
//!
//! 边界来源分三档：
//! - PE x86-64 的 `.pdata`（`RUNTIME_FUNCTION`）——精确；
//! - ELF 的 `STT_FUNC` 符号（要求 `st_size > 0`）——精确；
//! - 启发式（入口 + 直接调用目标）——**只用于展示，绝不用于下 `False` 结论**。
//!
//! 共现判定的三值规则：边界不精确、交叉引用不完整、或有调用点归不到函数时，
//! 一律 `Unknown`——因为"没看到"不能区分"真的没有"和"没看见"。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::value::Ternary;

use crate::elf;
use crate::imports;
use crate::pe;
use crate::xref::XrefReport;

/// 函数边界来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncSource {
    /// PE `.pdata`。
    Pdata,
    /// ELF `STT_FUNC`。
    Symtab,
    /// 启发式（入口 + 直接调用目标）。
    Heuristic,
}

impl FuncSource {
    /// 稳定短名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdata => "pdata",
            Self::Symtab => "symtab",
            Self::Heuristic => "heuristic",
        }
    }
}

/// 一个函数区间。
#[derive(Debug, Clone)]
pub struct Function {
    /// 起始地址（PE 为 RVA，ELF 为虚拟地址）。
    pub start: u64,
    /// 结束地址（不含）。
    pub end: u64,
    /// 名字（只有 `symtab` 有）。
    pub name: Option<String>,
    /// 边界来源。
    pub source: FuncSource,
}

/// 一个函数里观察到的导入调用。
#[derive(Debug, Clone)]
pub struct FunctionCalls {
    /// 函数区间。
    pub function: Function,
    /// 调用到的导入（去重、升序）。
    pub imports: Vec<String>,
    /// 对应的能力桶 id（去重、升序）。
    pub buckets: Vec<&'static str>,
}

/// 函数级报告。
#[derive(Debug, Clone)]
pub struct FunctionReport {
    /// 至少调用了一个导入的函数。
    pub functions: Vec<FunctionCalls>,
    /// 识别出的函数总数。
    pub total_functions: usize,
    /// 边界是否精确。
    pub exact: bool,
    /// 成功归属的调用点数。
    pub attributed_sites: usize,
    /// 归不到任何函数的调用点数。
    pub unattributed_sites: usize,
    /// 其中 `call` 类别的数量。
    pub unattributed_calls: usize,
    /// 其中 `jmp`（跳板/尾跳）类别的数量。
    pub unattributed_jumps: usize,
    /// 这些未归属调用点涉及的能力桶（只有可能影响该组的桶才会拖成 `Unknown`）。
    pub unattributed_buckets: Vec<&'static str>,
    /// 说明。
    pub notes: Vec<String>,
}

impl FunctionReport {
    /// 是否存在"同一函数内同时出现全部 `required` 能力桶"。
    ///
    /// 三值规则：
    /// - 找到 → `True`（正证据不会被别处的不确定性抹掉）；
    /// - 没找到，但边界不精确/交叉引用不完整/有调用点归不到函数 → `Unknown`
    ///   （"没看见"不等于"没有"）；
    /// - 没找到且证据完整 → `False`。
    pub fn cooccurrence(&self, required: &[&str], xref_complete: bool) -> Ternary {
        if !xref_complete || !self.exact {
            return Ternary::Unknown;
        }
        let found = self.functions.iter().any(|function| {
            required
                .iter()
                .all(|need| function.buckets.iter().any(|bucket| bucket == need))
        });
        if found {
            return Ternary::True;
        }
        // 未归属的调用点只在本组相关能力上才构成"可能漏看"。
        if required.iter().any(|need| {
            self.unattributed_buckets
                .iter()
                .any(|bucket| bucket == need)
        }) {
            Ternary::Unknown
        } else {
            Ternary::False
        }
    }
}

/// PE 的函数级报告。
pub fn build_pe(bytes: &[u8], info: &pe::PeInfo, xref: &XrefReport) -> FunctionReport {
    let exec = pe_exec_sections(bytes, info);
    let mut notes = Vec::new();
    let (mut ranges, exact) = match &info.runtime_functions {
        Some(functions) if !functions.is_empty() => (
            functions
                .iter()
                .map(|function| Function {
                    start: function.begin as u64,
                    end: function.end as u64,
                    name: None,
                    source: FuncSource::Pdata,
                })
                .collect(),
            true,
        ),
        _ => {
            notes.push("没有 .pdata：函数边界退化为启发式，只用于展示".to_string());
            (
                heuristic_ranges(&exec, info.entry_rva as u64, arch_of_pe(info.machine)),
                false,
            )
        }
    };
    normalize(&mut ranges);
    attribute(xref, &ranges, exact, notes)
}

/// ELF 的函数级报告。
pub fn build_elf(bytes: &[u8], info: &elf::ElfInfo, xref: &XrefReport) -> FunctionReport {
    let exec = elf_exec_sections(bytes, info);
    let mut notes = Vec::new();
    let symbols = info.function_symbols(bytes);
    let exec_bytes: u64 = exec.iter().map(|(_, data)| data.len() as u64).sum();
    let covered: u64 = symbols.iter().map(|symbol| symbol.size).sum();
    // 带大小的符号不等于"边界完整"：裁剪过的二进制只剩几个导出符号，
    // 覆盖率不够时必须退回启发式，否则会把绝大多数调用点算成"未归属"。
    let exact = coverage_is_complete(covered, exec_bytes);
    let (mut ranges, exact) = if exact {
        (
            symbols
                .iter()
                .map(|symbol| Function {
                    start: symbol.start,
                    end: symbol.start.saturating_add(symbol.size),
                    name: Some(symbol.name.clone()),
                    source: FuncSource::Symtab,
                })
                .collect(),
            true,
        )
    } else {
        if symbols.is_empty() {
            notes.push("没有带大小的 STT_FUNC：函数边界退化为启发式，只用于展示".to_string());
        } else {
            notes.push(format!(
                "STT_FUNC 只覆盖可执行字节的 {}%：边界退化为启发式，只用于展示",
                covered.saturating_mul(100) / exec_bytes.max(1)
            ));
        }
        (
            heuristic_ranges(&exec, info.entry, arch_of_elf(info.machine)),
            false,
        )
    };
    normalize(&mut ranges);
    attribute(xref, &ranges, exact, notes)
}

/// 符号覆盖的可执行字节是否足够撑起"精确边界"。
fn coverage_is_complete(covered: u64, exec_bytes: u64) -> bool {
    exec_bytes > 0 && covered.saturating_mul(10) >= exec_bytes.saturating_mul(9)
}

fn pe_exec_sections<'a>(bytes: &'a [u8], info: &pe::PeInfo) -> Vec<(u64, &'a [u8])> {
    info.sections
        .iter()
        .filter(|section| section.executable())
        .filter_map(|section| {
            let start = section.raw_ptr as usize;
            let end = start.checked_add(section.raw_size as usize)?;
            bytes
                .get(start..end)
                .map(|data| (section.virtual_address as u64, data))
        })
        .collect()
}

fn elf_exec_sections<'a>(bytes: &'a [u8], info: &elf::ElfInfo) -> Vec<(u64, &'a [u8])> {
    info.sections
        .iter()
        .filter(|section| section.flags & 0x4 != 0 && section.size > 0)
        .filter_map(|section| {
            let start = section.offset as usize;
            let end = start.checked_add(section.size as usize)?;
            bytes.get(start..end).map(|data| (section.addr, data))
        })
        .collect()
}

/// 启发式边界：入口 + 直接 `E8`/`E9` 的目标；区间截到下一个起点或所在节末尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arch {
    X86,
    Arm64,
}

fn arch_of_pe(machine: u16) -> Arch {
    if machine == 0xaa64 {
        Arch::Arm64
    } else {
        Arch::X86
    }
}

fn arch_of_elf(machine: u16) -> Arch {
    if machine == 183 {
        Arch::Arm64
    } else {
        Arch::X86
    }
}

fn heuristic_ranges(exec: &[(u64, &[u8])], entry: u64, arch: Arch) -> Vec<Function> {
    let bounds: Vec<(u64, u64)> = exec
        .iter()
        .map(|(base, data)| (*base, base.saturating_add(data.len() as u64)))
        .collect();
    let mut starts = BTreeSet::new();
    if entry != 0 && in_exec(entry, &bounds) {
        starts.insert(entry);
    }
    for (base, data) in exec {
        match arch {
            Arch::X86 => {
                let mut index = 0usize;
                while index + 5 <= data.len() {
                    if data[index] == 0xE8 || data[index] == 0xE9 {
                        let rel = i32::from_le_bytes([
                            data[index + 1],
                            data[index + 2],
                            data[index + 3],
                            data[index + 4],
                        ]);
                        let target = base
                            .wrapping_add(index as u64 + 5)
                            .wrapping_add(rel as i64 as u64);
                        if in_exec(target, &bounds) {
                            starts.insert(target);
                        }
                    }
                    index += 1;
                }
            }
            Arch::Arm64 => {
                let mut index = 0usize;
                while index + 4 <= data.len() {
                    let word = u32::from_le_bytes([
                        data[index],
                        data[index + 1],
                        data[index + 2],
                        data[index + 3],
                    ]);
                    let opcode = word & 0xFC00_0000;
                    if opcode == 0x9400_0000 || opcode == 0x1400_0000 {
                        let imm = sign_extend_28(((word & 0x03FF_FFFF) as i64) << 2);
                        let pc = base + index as u64;
                        let target = pc.wrapping_add(imm as u64);
                        if in_exec(target, &bounds) {
                            starts.insert(target);
                        }
                    }
                    index += 4;
                }
            }
        }
    }
    let mut ranges = Vec::new();
    for start in starts {
        let Some(&(_, end)) = bounds
            .iter()
            .find(|(low, high)| start >= *low && start < *high)
        else {
            continue;
        };
        ranges.push(Function {
            start,
            end,
            name: None,
            source: FuncSource::Heuristic,
        });
    }
    for index in 0..ranges.len().saturating_sub(1) {
        let next = ranges[index + 1].start;
        if next < ranges[index].end {
            ranges[index].end = next;
        }
    }
    ranges
}

fn sign_extend_28(value: i64) -> i64 {
    (value << 36) >> 36
}

fn in_exec(addr: u64, bounds: &[(u64, u64)]) -> bool {
    bounds
        .iter()
        .any(|(low, high)| addr >= *low && addr < *high)
}

fn normalize(ranges: &mut Vec<Function>) {
    ranges.sort_by_key(|function| (function.start, function.end));
    ranges.dedup_by(|left, right| left.start == right.start && left.end == right.end);
    for index in 0..ranges.len().saturating_sub(1) {
        let next = ranges[index + 1].start;
        if next < ranges[index].end {
            ranges[index].end = next;
        }
    }
}

fn attribute(
    xref: &XrefReport,
    ranges: &[Function],
    exact: bool,
    notes: Vec<String>,
) -> FunctionReport {
    let mut buckets_of: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for row in &xref.imports {
        let hits = imports::classify(std::slice::from_ref(&row.import));
        buckets_of.insert(row.import.clone(), hits.keys().copied().collect());
    }

    let mut per_function: Vec<(BTreeSet<String>, BTreeSet<&'static str>)> =
        vec![(BTreeSet::new(), BTreeSet::new()); ranges.len()];
    let mut attributed = 0usize;
    let mut unattributed = 0usize;
    let mut unattributed_calls = 0usize;
    let mut unattributed_jumps = 0usize;
    let mut unattributed_samples: Vec<u64> = Vec::new();
    let mut unattributed_buckets: BTreeSet<&'static str> = BTreeSet::new();
    for row in &xref.imports {
        for site in &row.sites {
            match locate(ranges, site.addr) {
                Some(index) => {
                    attributed += 1;
                    per_function[index].0.insert(row.import.clone());
                    if let Some(buckets) = buckets_of.get(&row.import) {
                        for bucket in buckets {
                            per_function[index].1.insert(*bucket);
                        }
                    }
                }
                None => {
                    unattributed += 1;
                    match site.kind {
                        crate::xref::SiteKind::Call => unattributed_calls += 1,
                        crate::xref::SiteKind::Jump => unattributed_jumps += 1,
                    }
                    if unattributed_samples.len() < 8 {
                        unattributed_samples.push(site.addr);
                    }
                    if let Some(buckets) = buckets_of.get(&row.import) {
                        for bucket in buckets {
                            unattributed_buckets.insert(*bucket);
                        }
                    }
                }
            }
        }
    }

    let functions: Vec<FunctionCalls> = ranges
        .iter()
        .cloned()
        .zip(per_function)
        .filter(|(_, (called, _))| !called.is_empty())
        .map(|(function, (called, buckets))| FunctionCalls {
            function,
            imports: called.into_iter().collect(),
            buckets: buckets.into_iter().collect(),
        })
        .collect();

    let mut notes = notes;
    if unattributed > 0 {
        let samples: Vec<String> = unattributed_samples
            .iter()
            .map(|addr| format!("0x{addr:x}"))
            .collect();
        notes.push(format!(
            "{unattributed} 个调用点不在任何精确函数区间内（call {unattributed_calls} / jmp {unattributed_jumps}；跳板与无 .pdata 的叶函数都会落在这里），示例 {}",
            samples.join(", ")
        ));
    }

    FunctionReport {
        functions,
        total_functions: ranges.len(),
        exact,
        attributed_sites: attributed,
        unattributed_sites: unattributed,
        unattributed_calls,
        unattributed_jumps,
        unattributed_buckets: unattributed_buckets.into_iter().collect(),
        notes,
    }
}

fn locate(ranges: &[Function], addr: u64) -> Option<usize> {
    let index = ranges.partition_point(|function| function.start <= addr);
    if index == 0 {
        return None;
    }
    let candidate = &ranges[index - 1];
    (addr < candidate.end).then_some(index - 1)
}

#[cfg(test)]
mod tests {
    use super::{FuncSource, Function, FunctionCalls, FunctionReport, locate};
    use trit_core::value::Ternary;

    fn cooccurrence_case() -> FunctionReport {
        FunctionReport {
            functions: vec![FunctionCalls {
                function: function(0x1000, 0x1100),
                imports: vec!["a!x".into(), "a!y".into()],
                buckets: vec!["a", "b"],
            }],
            total_functions: 1,
            exact: true,
            attributed_sites: 2,
            unattributed_sites: 0,
            unattributed_calls: 0,
            unattributed_jumps: 0,
            unattributed_buckets: vec![],
            notes: vec![],
        }
    }

    fn function(start: u64, end: u64) -> Function {
        Function {
            start,
            end,
            name: None,
            source: FuncSource::Pdata,
        }
    }

    #[test]
    fn exact_coverage_needs_ninety_percent() {
        use super::coverage_is_complete;
        assert!(coverage_is_complete(900, 1000));
        assert!(coverage_is_complete(1000, 1000));
        assert!(!coverage_is_complete(899, 1000));
        assert!(!coverage_is_complete(0, 0));
        assert!(!coverage_is_complete(10, 0));
    }

    #[test]
    fn unattributed_sites_are_split_by_kind() {
        use super::attribute;
        use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};
        let xref = XrefReport {
            arch: "test".into(),
            scanned_bytes: 0,
            complete: true,
            imports: vec![ImportXref {
                import: "a!VirtualAllocEx".into(),
                slot: 1,
                sites: vec![
                    CallSite {
                        addr: 0x100,
                        file_offset: 0,
                        kind: Kind::Call,
                    },
                    CallSite {
                        addr: 0x200,
                        file_offset: 0,
                        kind: Kind::Jump,
                    },
                ],
                status: Ternary::True,
                note: String::new(),
            }],
            notes: vec![],
        };
        let report = attribute(&xref, &[], true, vec![]);
        assert_eq!(report.unattributed_calls, 1);
        assert_eq!(report.unattributed_jumps, 1);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("call 1 / jmp 1"))
        );
    }

    #[test]
    fn locate_respects_half_open_ranges() {
        let ranges = vec![function(0x1000, 0x1100), function(0x1100, 0x1200)];
        assert_eq!(locate(&ranges, 0x1000), Some(0));
        assert_eq!(locate(&ranges, 0x10ff), Some(0));
        assert_eq!(locate(&ranges, 0x1100), Some(1));
        assert_eq!(locate(&ranges, 0x1200), None);
        assert_eq!(locate(&ranges, 0x0fff), None);
    }

    #[test]
    fn cooccurrence_requires_exact_and_fully_attributed() {
        let report = FunctionReport {
            functions: vec![],
            total_functions: 1,
            exact: true,
            attributed_sites: 0,
            unattributed_sites: 0,
            unattributed_calls: 0,
            unattributed_jumps: 0,
            unattributed_buckets: vec![],
            notes: vec![],
        };
        assert_eq!(report.cooccurrence(&["a", "b"], true), Ternary::False);
        assert_eq!(report.cooccurrence(&["a", "b"], false), Ternary::Unknown);
        let loose = FunctionReport {
            exact: false,
            ..report.clone()
        };
        assert_eq!(loose.cooccurrence(&["a", "b"], true), Ternary::Unknown);
        let relevant_gap = FunctionReport {
            unattributed_sites: 1,
            unattributed_buckets: vec!["a"],
            ..report.clone()
        };
        assert_eq!(
            relevant_gap.cooccurrence(&["a", "b"], true),
            Ternary::Unknown
        );
        // 与本组无关的未归属调用点不影响结论。
        let irrelevant_gap = FunctionReport {
            unattributed_sites: 1,
            unattributed_calls: 1,
            unattributed_buckets: vec!["other"],
            ..report.clone()
        };
        assert_eq!(
            irrelevant_gap.cooccurrence(&["a", "b"], true),
            Ternary::False
        );
        let hit = cooccurrence_case();
        assert_eq!(hit.cooccurrence(&["a", "b"], true), Ternary::True);
        // 正证据优先：即使有未归属调用点，找到共现仍然给 True。
        let hit_with_gap = FunctionReport {
            unattributed_sites: 3,
            unattributed_calls: 3,
            unattributed_buckets: vec!["a"],
            ..cooccurrence_case()
        };
        assert_eq!(hit_with_gap.cooccurrence(&["a", "b"], true), Ternary::True);
    }
}
