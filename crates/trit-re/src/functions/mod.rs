//! 函数级归属与能力共现。
//!
//! 交叉引用回答"这个导入有没有被调用"；本模块回答"**在哪个函数里被调用**"，
//! 并进一步问"同一函数里是否同时出现了一组能力"（注入三件套、装载+网络、凭据+外发）。
//!
//! 边界来源分三档：
//! - PE x86-64 的 `.pdata`（`RUNTIME_FUNCTION`）——精确；
//! - ELF 的 `STT_FUNC` 符号（要求 `st_size > 0`）——精确；
//! - 最小 CFG（入口 + 直接控制流目标，N2 第一层）——**补隙合并**：有精确源时
//!   只填充精确区间未覆盖的可执行隙（绝不切分精确区间）；无精确源时覆盖全部
//!   可执行节。`Cfg` 边界的 `exact` 恒为 false，共现判定照旧要求精确源——
//!   绝不用于下 `False` 结论。
//!
//! 共现判定的三值规则：边界不精确、交叉引用不完整、或有调用点归不到函数时，
//! 一律 `Unknown`——因为"没看到"不能区分"真的没有"和"没看见"。

mod cfg;
mod x86len;

use std::collections::{BTreeMap, BTreeSet};

use trit_core::value::Ternary;

use crate::elf;
use crate::imports;
use crate::pe;
use crate::xref::XrefReport;
use cfg::cfg_ranges;

/// 函数边界来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncSource {
    /// PE `.pdata`。
    Pdata,
    /// PE COFF 符号表（PE32 常见；与 ELF `STT_FUNC` 同等权威）。
    CoffSymbols,
    /// ELF `STT_FUNC`。
    Symtab,
    /// ELF `.eh_frame`（FDE 区间）。
    EhFrame,
    /// ELF `.debug_frame`（DWARF CFI 区间）。
    DebugFrame,
    /// 最小 CFG 补隙（入口 + 直接控制流目标；N2 第一层，字节模式级——
    /// 不是指令级遍历，证据等级与旧启发式相同：不参与 `False` 结论）。
    Cfg,
}

impl FuncSource {
    /// 稳定短名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdata => "pdata",
            Self::CoffSymbols => "coff-symtab",
            Self::Symtab => "symtab",
            Self::EhFrame => "eh_frame",
            Self::DebugFrame => "debug_frame",
            Self::Cfg => "cfg",
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

/// 未归属调用点的**逐条裁定**。
///
/// 裁定是**证据分类**，不是归属成功——方向保守：任何裁定都不会把调用点
/// 洗成"已归属"，能力桶的 `Unknown` 拖累逻辑不变。裁定回答的是
/// "这个调用点为什么归不到函数"，让人能直接看边界质量问题出在哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteRuling {
    /// 疑**边界裁剪**：距最近函数边界 ≤ [`BOUNDARY_CLIP_BYTES`]——
    /// 对齐填充/边界源截断（`.pdata` 只记录函数体，尾部的 unwind/填充码
    /// 会落在区间外）里的真实代码。
    BoundaryClip,
    /// 疑**边界漏检**：在可执行节内、距任何函数边界 > [`BOUNDARY_CLIP_BYTES`]
    /// ——某个函数的起点没有被任何边界源识别。
    BoundaryMiss,
    /// **不在可执行节**：字节模式匹配的误报候选（数据段里恰好长得像指令）。
    NonExec,
}

impl SiteRuling {
    /// 稳定短名（进台账）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BoundaryClip => "boundary-clip",
            Self::BoundaryMiss => "boundary-miss",
            Self::NonExec => "non-exec",
        }
    }
}

/// 边界裁剪的判定半径：x64 最长指令 15 字节，函数间对齐填充 ≤15 字节。
pub const BOUNDARY_CLIP_BYTES: u64 = 16;

/// 单个未归属调用点的裁定明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuledSite {
    /// 调用点地址（PE 为 RVA，ELF 为虚拟地址）。
    pub addr: u64,
    /// 调用点类别。
    pub kind: crate::xref::SiteKind,
    /// 裁定。
    pub ruling: SiteRuling,
    /// 到最近函数边界的距离（不在任何函数区间内时的间隔字节数）。
    pub distance: u64,
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
    /// 未归属调用点的**逐条裁定**（超过 [`MAX_RULED_SITES`] 条截断，计数仍完整）。
    pub ruled_sites: Vec<RuledSite>,
    /// 三类裁定的总数（与截断无关）。
    pub ruling_clip: usize,
    /// 疑边界漏检数。
    pub ruling_miss: usize,
    /// 非执行节误报候选数。
    pub ruling_nonexec: usize,
    /// 说明。
    pub notes: Vec<String>,
}

/// 逐条裁定的保留上限（报告体积护栏；计数不受影响）。
pub const MAX_RULED_SITES: usize = 64;

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
        // PE32 没有 `.pdata`；COFF 符号表（mingw 系默认带）是与 ELF `STT_FUNC`
        // 同等权威的边界源——函数符号的 Value 按 RVA 解释，落在可执行节内
        // 才收（pe/coff.rs 已自校验）。区间终点 = 下一个函数起点 / 所在节末尾。
        _ => match &info.coff_functions {
            Some(functions) if !functions.is_empty() => {
                notes.push(format!(
                    "没有 .pdata：函数边界取自 COFF 符号表（{} 个函数符号）",
                    functions.len()
                ));
                (coff_symbol_ranges(functions, &info.sections), true)
            }
            _ => {
                notes.push(
                    "没有 .pdata 也没有可用的 COFF 符号表：函数边界来自最小 CFG（字节模式级，不参与 False 结论）"
                        .to_string(),
                );
                (
                    cfg_ranges(
                        &exec,
                        &[],
                        info.entry_rva as u64,
                        arch_of_pe(info.machine),
                        &[],
                    ),
                    false,
                )
            }
        },
    };
    normalize(&mut ranges);
    // N2 补隙：精确边界源未覆盖的可执行隙由 CFG 填充（候选起点 = 入口 +
    // 直接控制流目标；绝不切分精确区间——切分是 fn_* 回归的头号来源）。
    // v4：数据节指针表收割的候选起点一并进入（回调/虚表方法的间接入口）。
    if exact {
        let exec_bounds: Vec<(u64, u64)> = exec
            .iter()
            .map(|(base, data)| (*base, base.saturating_add(data.len() as u64)))
            .collect();
        let pointer_seeds = pe_pointer_seeds(bytes, info, &exec_bounds);
        let gap_ranges = cfg_ranges(
            &exec,
            &ranges,
            info.entry_rva as u64,
            arch_of_pe(info.machine),
            &pointer_seeds,
        );
        if !gap_ranges.is_empty() {
            notes.push(format!(
                "CFG 补隙：{} 个区间（.pdata/符号表未覆盖的可执行隙；指针表种子 {}）",
                gap_ranges.len(),
                pointer_seeds.len()
            ));
            ranges.extend(gap_ranges);
        }
    }
    normalize(&mut ranges);
    attribute(xref, &ranges, &exec, exact, notes)
}

/// ELF 的函数级报告。
pub fn build_elf(bytes: &[u8], info: &elf::ElfInfo, xref: &XrefReport) -> FunctionReport {
    let exec = elf_exec_sections(bytes, info);
    let mut notes = Vec::new();
    let symbols = info.function_symbols(bytes);
    let exec_bytes: u64 = exec.iter().map(|(_, data)| data.len() as u64).sum();
    let covered: u64 = symbols.iter().map(|symbol| symbol.size).sum();
    // 带大小的符号不等于"边界完整"：裁剪过的二进制只剩几个导出符号，
    // 覆盖率不够时先看 `.eh_frame`（FDE 是链接器写下的真实函数区间），再退回启发式。
    let symtab_exact = coverage_is_complete(covered, exec_bytes);
    let eh_frame = if symtab_exact {
        Vec::new()
    } else {
        info.eh_frame_ranges(bytes)
    };
    let debug_frame = if symtab_exact {
        Vec::new()
    } else {
        info.debug_frame_ranges(bytes)
    };
    let chosen = if symtab_exact {
        None
    } else {
        choose_frame_source(&eh_frame, &debug_frame, xref)
    };
    let (mut ranges, exact) = if symtab_exact {
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
    } else if let Some((source, chosen, other_sites)) = chosen {
        let label = match source {
            FuncSource::EhFrame => ".eh_frame",
            FuncSource::DebugFrame => ".debug_frame",
            _ => "?",
        };
        notes.push(format!(
            "边界来自 {label}（{} 个 FDE，覆盖 {} 个调用点）{}",
            chosen.len(),
            site_coverage(chosen, xref),
            match other_sites {
                Some(sites) => format!("；另一来源覆盖 {sites} 个调用点"),
                None => String::new(),
            }
        ));
        (
            chosen
                .iter()
                .map(|symbol| Function {
                    start: symbol.start,
                    end: symbol.start.saturating_add(symbol.size),
                    name: None,
                    source,
                })
                .collect(),
            true,
        )
    } else {
        if symbols.is_empty() {
            notes.push(
                "没有带大小的 STT_FUNC：函数边界来自最小 CFG（字节模式级，不参与 False 结论）"
                    .to_string(),
            );
        } else {
            notes.push(format!(
                "STT_FUNC 只覆盖可执行字节的 {}%：边界来自最小 CFG 补隙（字节模式级，不参与 False 结论）",
                covered.saturating_mul(100) / exec_bytes.max(1)
            ));
        }
        (
            cfg_ranges(&exec, &[], info.entry, arch_of_elf(info.machine), &[]),
            false,
        )
    };
    normalize(&mut ranges);
    // N2 补隙（ELF）：符号表/CFI 未覆盖的可执行隙由 CFG 填充（不切分精确区间）。
    if exact {
        let gap_ranges = cfg_ranges(&exec, &ranges, info.entry, arch_of_elf(info.machine), &[]);
        if !gap_ranges.is_empty() {
            notes.push(format!(
                "CFG 补隙：{} 个区间（符号表/CFI 未覆盖的可执行隙）",
                gap_ranges.len()
            ));
            ranges.extend(gap_ranges);
        }
    }
    normalize(&mut ranges);
    attribute(xref, &ranges, &exec, exact, notes)
}

/// 边界候选覆盖了多少个交叉引用调用点。
fn site_coverage(symbols: &[elf::FunctionSymbol], xref: &XrefReport) -> usize {
    if symbols.is_empty() {
        return 0;
    }
    let mut ranges: Vec<Function> = symbols
        .iter()
        .map(|symbol| Function {
            start: symbol.start,
            end: symbol.start.saturating_add(symbol.size),
            name: None,
            source: FuncSource::EhFrame,
        })
        .collect();
    ranges.sort_by_key(|function| function.start);
    xref.imports
        .iter()
        .flat_map(|row| row.sites.iter())
        .filter(|site| locate(&ranges, site.addr).is_some())
        .count()
}

fn frame_bytes(symbols: &[elf::FunctionSymbol]) -> u64 {
    symbols.iter().map(|symbol| symbol.size).sum()
}

/// 同时有 `.eh_frame` 与 `.debug_frame` 时，按实际覆盖的调用点选择；
/// 平手再比字节数，最后偏向 `.eh_frame`（标准异步表）。
fn choose_frame_source<'a>(
    eh_frame: &'a [elf::FunctionSymbol],
    debug_frame: &'a [elf::FunctionSymbol],
    xref: &XrefReport,
) -> Option<(FuncSource, &'a [elf::FunctionSymbol], Option<usize>)> {
    match (eh_frame.is_empty(), debug_frame.is_empty()) {
        (true, true) => None,
        (false, true) => Some((FuncSource::EhFrame, eh_frame, None)),
        (true, false) => Some((FuncSource::DebugFrame, debug_frame, None)),
        (false, false) => {
            let eh_sites = site_coverage(eh_frame, xref);
            let debug_sites = site_coverage(debug_frame, xref);
            let pick_debug = debug_sites > eh_sites
                || (debug_sites == eh_sites && frame_bytes(debug_frame) > frame_bytes(eh_frame));
            if pick_debug {
                Some((FuncSource::DebugFrame, debug_frame, Some(eh_sites)))
            } else {
                Some((FuncSource::EhFrame, eh_frame, Some(debug_sites)))
            }
        }
    }
}

/// 符号覆盖的可执行字节是否足够撑起"精确边界"。
fn coverage_is_complete(covered: u64, exec_bytes: u64) -> bool {
    exec_bytes > 0 && covered.saturating_mul(10) >= exec_bytes.saturating_mul(9)
}

/// COFF 函数符号 → 函数区间：起点 = 符号 RVA（升序去重后），终点 =
/// 下一个函数起点与所在节末尾的较小者；最后一个函数到所在节末尾。
/// 符号不落在任何节里（pe/coff.rs 已过滤，这里再兜底）就跳过。
fn coff_symbol_ranges(functions: &[pe::CoffFunction], sections: &[pe::Section]) -> Vec<Function> {
    let section_end = |rva: u32| -> u64 {
        sections
            .iter()
            .find(|section| {
                rva >= section.virtual_address
                    && rva < section.virtual_address.saturating_add(section.raw_size)
            })
            .map(|section| {
                section.virtual_address as u64 + section.virtual_size.max(section.raw_size) as u64
            })
            .unwrap_or(u64::MAX)
    };
    let mut ranges: Vec<Function> = Vec::new();
    for (index, symbol) in functions.iter().enumerate() {
        let start = symbol.rva as u64;
        let next_start = functions
            .get(index + 1)
            .map(|next| next.rva as u64)
            .unwrap_or(u64::MAX);
        let end = next_start.min(section_end(symbol.rva));
        if end <= start {
            continue;
        }
        ranges.push(Function {
            start,
            end,
            name: symbol.name.clone(),
            source: FuncSource::CoffSymbols,
        });
    }
    ranges
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

/// v4：**数据节指针表收割**——非可执行节里指向可执行域的指针串。
///
/// 间接调用的函数（回调/虚表方法）在数据节里被引用：vtable 是一串连续
/// 的函数指针。收割规则（全机械）：
/// - PE32+ 按 u64、PE32 按 u32 读（对齐步进）；
/// - 值 − `image_base` 必须落在可执行域内（NULL 槽位跳过、非可执行域归零连击）；
/// - **连续 ≥2 个**可执行域指针（表的形状）全收，孤立指针仅在 16 字节
///   对齐时收（x64 MSVC 函数 16 对齐；不对齐的孤立值多半是整数巧合）。
///
/// 返回候选起点 RVA——只当起点、不进块遍历（收割侧已过滤，零覆盖污染）。
/// ELF 不在此列：PIE 下数据节指针要先过重定位，诚实留白。
pub(crate) fn pe_pointer_seeds(
    bytes: &[u8],
    info: &pe::PeInfo,
    exec_bounds: &[(u64, u64)],
) -> Vec<u64> {
    let stride = if info.plus { 8usize } else { 4usize };
    let mut seeds = Vec::new();
    for section in &info.sections {
        if section.executable() {
            continue;
        }
        let start = section.raw_ptr as usize;
        let Some(end) = start.checked_add(section.raw_size as usize) else {
            continue;
        };
        let Some(data) = bytes.get(start..end) else {
            continue;
        };
        let mut run = 0usize;
        for off in (0..data.len()).step_by(stride) {
            let Some(chunk) = data.get(off..off + stride) else {
                break;
            };
            let value = if info.plus {
                u64::from_le_bytes(chunk.try_into().expect("步进与宽度一致"))
            } else {
                u32::from_le_bytes(chunk.try_into().expect("步进与宽度一致")) as u64
            };
            let Some(rva) = value.checked_sub(info.image_base) else {
                run = 0;
                continue;
            };
            if !exec_bounds
                .iter()
                .any(|(low, high)| rva >= *low && rva < *high)
            {
                run = 0;
                continue;
            }
            run += 1;
            if run >= 2 || rva % 16 == 0 {
                seeds.push(rva);
            }
        }
    }
    seeds
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
    exec: &[(u64, &[u8])],
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
    let mut ruled_sites: Vec<RuledSite> = Vec::new();
    let mut ruling_clip = 0usize;
    let mut ruling_miss = 0usize;
    let mut ruling_nonexec = 0usize;
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
                    // 逐条裁定：为什么归不到函数。裁定不改变保守方向
                    // （不洗成"已归属"），只给可复算的证据分类。
                    let in_exec = exec.iter().any(|(start, data)| {
                        site.addr >= *start && site.addr < start.wrapping_add(data.len() as u64)
                    });
                    let distance = nearest_boundary_distance(ranges, site.addr);
                    let (ruling, counter) = if !in_exec {
                        (SiteRuling::NonExec, &mut ruling_nonexec)
                    } else if distance <= BOUNDARY_CLIP_BYTES {
                        (SiteRuling::BoundaryClip, &mut ruling_clip)
                    } else {
                        (SiteRuling::BoundaryMiss, &mut ruling_miss)
                    };
                    *counter += 1;
                    if ruled_sites.len() < MAX_RULED_SITES {
                        ruled_sites.push(RuledSite {
                            addr: site.addr,
                            kind: site.kind,
                            ruling,
                            distance,
                        });
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
            "{unattributed} 个调用点不在任何精确函数区间内（call {unattributed_calls} / jmp {unattributed_jumps}；跳板或缺少边界信息的函数（PE 叶函数 / ELF 缺 FDE）都会落在这里），示例 {}",
            samples.join(", ")
        ));
        notes.push(format!(
            "逐条裁定：疑边界裁剪（≤{BOUNDARY_CLIP_BYTES}B）{ruling_clip}、疑边界漏检（exec 内远距）{ruling_miss}、非执行节（误报候选）{ruling_nonexec}；明细见 ruled_sites（≤{MAX_RULED_SITES} 条）"
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
        ruled_sites,
        ruling_clip,
        ruling_miss,
        ruling_nonexec,
        notes,
    }
}

/// 到最近函数边界的距离：地址在补集里时，与所有 `[start, end)` 区间的
/// 最小间隔。区间内不会调用本函数（locate 已筛过）。
fn nearest_boundary_distance(ranges: &[Function], addr: u64) -> u64 {
    ranges
        .iter()
        .map(|function| {
            if addr < function.start {
                function.start - addr
            } else {
                // addr >= end：间隔 = addr 越过区间末尾的字节数。
                addr - function.end + 1
            }
        })
        .min()
        .unwrap_or(u64::MAX)
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
mod tests;
