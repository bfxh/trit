//! From parsed structures to three-valued facts.
//!
//! # The one primitive that keeps this honest
//!
//! Almost every derived fact has the shape "does the file contain X?". With a
//! boolean the answer is `false` both when the answer is genuinely no and when the
//! search could not complete — and those two cases lead to opposite analyst
//! decisions. [`presence`] makes the distinction explicit and is used for every
//! import-derived fact:
//!
//! ```text
//!   seen == true                  -> True      (it is there)
//!   seen == false && complete     -> False     (looked everywhere, absent)
//!   seen == false && !complete    -> Unknown   (did not finish looking)
//! ```
//!
//! The truncated-observation cases are not hypothetical: a file cut off by a
//! failed download, a section table that walks off the end, an import descriptor
//! whose name RVA points into unmapped space. A boolean pipeline reports all of
//! those as "clean".
//!
//! # Calibration
//!
//! The bands in [`calibration`] are where a measurement becomes a trit. They are
//! filled from a measured corpus, and `t3 bench` prints the quantiles that justify
//! them. A band whose width is zero is a legacy hard threshold and is called out as
//! such.

use t3_core::{FactId, Facts, Tri};
use t3_kernel::entropy;

use crate::elf::{self};

mod elf_fill;

use crate::pe::{self, cap, dirs_index, PeInfo};
use crate::region::Region;
use elf_fill::fill_elf;

/// Decision bands and thresholds, all in one place so they can be audited.
pub mod calibration {
    use t3_core::Band;

    /// Files below this size cannot carry a meaningful entropy statistic; their
    /// sections are too small for the estimate to be stable.
    pub const MIN_TRUSTED_SIZE: u64 = 16 * 1024;
    /// Sections smaller than this are excluded from the maximum-entropy statistic.
    /// A 16-byte stub has an entropy of 4 bits by construction and would otherwise
    /// dominate.
    pub const MIN_ENTROPY_BYTES: usize = 4096;

    /// Maximum section entropy, over executable sections.
    ///
    /// Measured on 6 503 executable sections from 10 242 clean Windows system binaries
    /// (`t3 bench --dir C:\Windows\System32`):
    ///
    /// ```text
    ///   p50 6.115   p95 6.487   p99 6.643   p99.5 6.675   p99.9 7.369   max 7.930
    /// ```
    ///
    /// `lo` is the 99.5th percentile and `hi` the 99.9th, so half a percent of clean
    /// files land in the undecided band and a tenth of a percent above it. Those
    /// numbers are the honest cost of the rule, not a claim that the rule is exact.
    pub const SECTION_ENTROPY: Band = Band { lo: 6.68, hi: 7.37 };

    /// Whole-file entropy, same corpus: p99.5 7.162, p99.9 7.541.
    ///
    /// Reported but not used by any hypothesis on PE/ELF inputs: per-section entropy
    /// strictly dominates it there. It exists for inputs whose container was not
    /// recognised, where nothing finer is available.
    pub const FILE_ENTROPY: Band = Band { lo: 7.16, hi: 7.54 };

    /// Overlay entropy.
    ///
    /// Four clean files in the corpus had an overlay at all, and three of those were
    /// high-entropy. The sample is far too small to place a percentile, so this band is
    /// a stated prior rather than a measurement — and it is the one threshold here that
    /// a reader should not treat as calibrated.
    pub const OVERLAY_ENTROPY: Band = Band { lo: 6.95, hi: 7.40 };

    /// Fraction of the file the overlay must occupy to count as dominant,
    /// in percent.
    pub const OVERLAY_DOMINANT_PERCENT: usize = 25;

    /// A section must carry at least this many raw bytes of executable data before
    /// "there are no imports" means anything.
    pub const MIN_CODE_BYTES: usize = 4096;

    /// Named imports at or below this count are "sparse".
    pub const SPARSE_IMPORT_MAX: u32 = 8;

    /// Dynamic resolution with at most this many static imports means the binary
    /// resolves essentially everything at run time.
    pub const DYNRESOLVE_IMPORT_MAX: u32 = 12;
}

use calibration::{
    DYNRESOLVE_IMPORT_MAX, FILE_ENTROPY, MIN_ENTROPY_BYTES, MIN_TRUSTED_SIZE,
    OVERLAY_DOMINANT_PERCENT, OVERLAY_ENTROPY, SECTION_ENTROPY, SPARSE_IMPORT_MAX,
};

/// Container the analyser recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// PE32 or PE32+.
    Pe,
    /// ELF32 or ELF64.
    Elf,
    /// Neither; only whole-file statistics are available.
    Unknown,
}

impl Kind {
    /// Short name for reports and JSONL.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Pe => "pe",
            Kind::Elf => "elf",
            Kind::Unknown => "unknown",
        }
    }
}

/// Result of a triage pass over one buffer.
pub struct Analysis<'a> {
    /// Observations, all of them tri-state.
    pub facts: Facts,
    /// Byte regions available to the signature engine.
    pub regions: Vec<Region<'a>>,
    /// What the buffer turned out to be.
    pub kind: Kind,
    /// Why the format was not recognised, for the report.
    pub note: &'static str,
}

/// Sound three-valued answer to "is X present?".
///
/// See the module documentation; this is the load-bearing primitive.
#[inline]
#[must_use]
pub fn presence(seen: bool, complete: bool) -> Tri {
    if seen {
        Tri::True
    } else if complete {
        Tri::False
    } else {
        Tri::Unknown
    }
}

/// Analyses a buffer.
#[must_use]
pub fn analyze(bytes: &[u8]) -> Analysis<'_> {
    let mut facts = Facts::new();
    fill_common(&mut facts, bytes);

    match pe::parse(bytes) {
        Ok(info) => {
            let regions = info.regions();
            fill_pe(&mut facts, &info);
            return Analysis {
                facts,
                regions,
                kind: Kind::Pe,
                note: if info.is_64 { "PE32+" } else { "PE32" },
            };
        }
        Err(pe::PeError::NotMz) => {}
        Err(err) => {
            // MZ present but the NT header is unusable. That is a PE-family file
            // with a damaged header, which is itself worth reporting.
            facts.set_bool(FactId::FormatPe, true, "MZ 签名存在");
            facts.set(
                FactId::SectionMalformed,
                Tri::True,
                f32::NAN,
                match err {
                    pe::PeError::BadNtHeader => "NT 头不可用",
                    pe::PeError::Truncated => "节表被截断",
                    pe::PeError::NotMz => "无 MZ",
                },
            );
            return Analysis {
                facts,
                regions: crate::region::whole_file(bytes),
                kind: Kind::Pe,
                note: "damaged PE header",
            };
        }
    }

    match elf::parse(bytes) {
        Ok(info) => {
            let regions = info.regions();
            fill_elf(&mut facts, &info);
            return Analysis {
                facts,
                regions,
                kind: Kind::Elf,
                note: if info.is_64 { "ELF64" } else { "ELF32" },
            };
        }
        Err(elf::ElfError::NotElf) => {}
        Err(err) => {
            facts.set_bool(FactId::FormatElf, true, "ELF 魔数存在");
            facts.set(
                FactId::SectionMalformed,
                Tri::True,
                f32::NAN,
                match err {
                    elf::ElfError::BadIdent => "ELF ident 不可用",
                    elf::ElfError::Truncated => "ELF 头表被截断",
                    elf::ElfError::NotElf => "无 ELF 魔数",
                },
            );
            return Analysis {
                facts,
                regions: crate::region::whole_file(bytes),
                kind: Kind::Elf,
                note: "damaged ELF header",
            };
        }
    }

    Analysis {
        facts,
        regions: crate::region::whole_file(bytes),
        kind: Kind::Unknown,
        note: "未识别的容器格式",
    }
}

fn fill_common(facts: &mut Facts, bytes: &[u8]) {
    let len = bytes.len() as u64;
    facts.set_bool_at(
        FactId::SizeSubstantial,
        len >= MIN_TRUSTED_SIZE,
        len as f32,
        "文件大小",
    );
    let whole = entropy(bytes);
    facts.set_band(
        FactId::FileHighEntropy,
        whole as f32,
        FILE_ENTROPY,
        "整文件熵",
    );
}

fn fill_pe(facts: &mut Facts, info: &PeInfo<'_>) {
    facts.set_bool(FactId::FormatPe, true, "MZ + PE 签名");
    facts.set_bool(FactId::FormatElf, false, "非 ELF");

    // ---- sections -------------------------------------------------------
    let mut packer_hit = false;
    let mut writable_exec = false;
    let mut zero_raw = false;
    let mut malformed = false;
    let mut last_raw_end = 0usize;
    let mut non_monotone = false;
    let mut entry_in_last = false;
    let mut entry_in_code = false;

    for (i, section) in info.sections().iter().enumerate() {
        if pe::is_packer_section(section.name_bytes()) {
            packer_hit = true;
        }
        if section.is_writable() && section.is_executable() {
            writable_exec = true;
        }
        if section.raw_size == 0
            && section.virtual_size >= 0x1000
            && section.is_executable()
            && !pe::is_padding_section(section.name_bytes())
        {
            zero_raw = true;
        }
        if section.raw_size > 0 {
            match section.raw_range(info.bytes.len()) {
                Some((start, end)) => {
                    if start < last_raw_end {
                        non_monotone = true;
                    }
                    last_raw_end = last_raw_end.max(end);
                }
                None => malformed = true,
            }
        }
        if section.contains_rva(info.entry_rva) && info.entry_rva != 0 {
            if section.is_code() {
                entry_in_code = true;
            }
            if i + 1 == info.section_count && info.section_count >= 2 {
                entry_in_last = true;
            }
        }
    }

    if info.sections_truncated || info.dirs_truncated {
        malformed = true;
    }
    facts.set_bool(FactId::SectionPackerName, packer_hit, "壳/保护器节名");
    facts.set_bool(
        FactId::SectionWritableExecutable,
        writable_exec,
        "可写可执行节区",
    );
    facts.set_bool(FactId::SectionZeroRaw, zero_raw, "仅虚拟大小的可执行节区");
    facts.set_bool(
        FactId::LayoutNonMonotone,
        non_monotone,
        "节区原始偏移非单调",
    );
    if malformed {
        facts.set_bool(FactId::SectionMalformed, true, "节区声明越界或表被截断");
    } else {
        facts.set_bool(FactId::SectionMalformed, false, "节区表完整");
    }

    let count = info.declared_section_count;
    facts.set_bool_at(
        FactId::SectionCountAnomalous,
        count == 0 || count > 40,
        f32::from(count),
        "节区数量",
    );

    // A resource-only image (`.mui` satellites, icon libraries) has no entry point and
    // needs no imports. Treating those absences as *unknown* leaves three hypotheses
    // permanently open and makes a quarter of a clean Windows install read as
    // undecided. They are not unknown: there is no code for them to describe.
    let code_bytes: usize = info
        .sections()
        .iter()
        .filter(|s| s.is_executable())
        .map(|s| s.raw_size as usize)
        .sum();
    let has_code = info
        .sections()
        .iter()
        .any(|s| s.is_executable() && (s.raw_size as usize) >= calibration::MIN_CODE_BYTES);

    // ---- entropy (the partial-information rule) -------------------------
    let stats = info.entropy_stats(MIN_ENTROPY_BYTES);
    set_entropy_facts(facts, stats.max_exec, stats.max_data, stats.unreadable);

    // ---- entry point ----------------------------------------------------
    if info.section_count == 0 {
        facts.set_unknown(FactId::EntryOutsideCode, "无节区，无法定位入口点");
    } else if !has_code {
        facts.set_bool(FactId::EntryOutsideCode, false, "无代码节区（资源型文件）");
    } else if info.entry_rva == 0 {
        // Some DLLs legitimately have no entry point. That is a decided value, not an
        // anomaly, and calling it unknown would leave `layout_anomaly` open forever.
        facts.set_bool(FactId::EntryOutsideCode, false, "入口点字段为 0");
    } else if entry_in_code {
        facts.set_bool(FactId::EntryOutsideCode, false, "入口点位于可执行节区");
    } else {
        facts.set_bool(FactId::EntryOutsideCode, true, "入口点不在可执行节区");
    }
    facts.set_bool(
        FactId::EntryInLastSection,
        entry_in_last,
        "入口点位于最后一个节区",
    );

    // ---- overlay --------------------------------------------------------
    if info.overlay.len == 0 {
        facts.set_bool(FactId::OverlayPresent, false, "无叠加段");
        facts.set_bool(FactId::OverlayHighEntropy, false, "无叠加段");
        facts.set_bool(FactId::OverlayDominant, false, "无叠加段");
    } else {
        facts.set_bool_at(
            FactId::OverlayPresent,
            true,
            info.overlay.len as f32,
            "叠加段字节数",
        );
        match info.overlay.entropy {
            Some(e) if info.overlay.len >= MIN_ENTROPY_BYTES => {
                facts.set_band(
                    FactId::OverlayHighEntropy,
                    e as f32,
                    OVERLAY_ENTROPY,
                    "叠加段熵",
                );
            }
            Some(e) => {
                // Too small to measure reliably; report the number, keep the trit.
                facts.set_unknown(FactId::OverlayHighEntropy, "叠加段过小");
                facts.set(
                    FactId::OverlayHighEntropy,
                    Tri::Unknown,
                    e as f32,
                    "叠加段过小",
                );
            }
            None => facts.set_unknown(FactId::OverlayHighEntropy, "叠加段未测量"),
        }
        let percent = (info.overlay.len * 100)
            .checked_div(info.bytes.len())
            .unwrap_or(100);
        facts.set_bool_at(
            FactId::OverlayDominant,
            percent >= OVERLAY_DOMINANT_PERCENT,
            percent as f32,
            "叠加段占文件百分比",
        );
    }

    // ---- imports --------------------------------------------------------
    fill_imports(facts, &info.imports, info, has_code, code_bytes);

    // ---- provenance -----------------------------------------------------
    facts.set_bool(FactId::SignedPresent, info.signed, "安全目录");
    facts.set_bool(
        FactId::PdbPresent,
        !info.debug.pdb.is_empty(),
        "CodeView 调试记录",
    );
    facts.set_bool(
        FactId::PdbDeveloperPath,
        info.debug.developer_path,
        "PDB 路径含开发者目录",
    );
    facts.set_bool(
        FactId::RichHeaderValid,
        rich_header_valid(info.bytes, info.e_lfanew),
        "Rich 头（MSVC 指纹）",
    );
    facts.set_bool(
        FactId::DotnetPresent,
        pe::directory(info, dirs_index::DIR_COM).size > 0,
        "CLR 目录",
    );
    facts.set_bool(
        FactId::RelocationsPresent,
        pe::directory(info, dirs_index::DIR_BASERELOC).size > 0,
        "重定位目录",
    );
    facts.set_bool(
        FactId::ResourcesPresent,
        pe::directory(info, dirs_index::DIR_RESOURCE).size > 0,
        "资源目录",
    );
    facts.set_bool(FactId::TlsCallbacks, has_tls_callbacks(info), "TLS 回调");

    // ---- timestamp ------------------------------------------------------
    //
    // Only a zero timestamp counts. Measured on a real Windows corpus, the low and
    // high bounds both produced nothing but false positives: modern MSVC with
    // /Brepro writes a deterministic hash into TimeDateStamp, so system binaries
    // routinely carry values from 1970 and from the future. A zero timestamp still
    // means what it always meant — the field was never filled in, which is what a
    // hand-crafted or packed binary looks like.
    let ts = info.timestamp;
    facts.set_bool_at(FactId::TimestampAnomalous, ts == 0, ts as f32, "PE 时间戳");
}

/// Records the two entropy facts, applying the sound partial-information rule.
///
/// A readable section already over the band settles the fact, because a maximum can
/// only be higher than one of its terms. Anything else, when some sections could not be
/// read, stays `Unknown`: the unmeasured section might have been the high one.
fn set_entropy_facts(
    facts: &mut Facts,
    max_exec: Option<f64>,
    max_data: Option<f64>,
    unreadable: bool,
) {
    for (id, value, label) in [
        (FactId::SectionHighEntropy, max_exec, "可执行节区最高熵"),
        (
            FactId::SectionHighEntropyData,
            max_data,
            "非可执行节区最高熵",
        ),
    ] {
        match value {
            Some(e) => {
                let decided = SECTION_ENTROPY.classify(e as f32);
                if decided.is_true() || !unreadable {
                    facts.set_band(id, e as f32, SECTION_ENTROPY, label);
                } else {
                    facts.set(id, Tri::Unknown, e as f32, label);
                }
            }
            None => facts.set_unknown(id, "无可测节区"),
        }
    }
}

/// Import-derived facts, shared by PE and ELF.
fn fill_imports(
    facts: &mut Facts,
    imports: &pe::Imports,
    info: &PeInfo<'_>,
    has_code: bool,
    code_bytes: usize,
) {
    let complete = imports.complete;
    facts.set_bool(FactId::ImportsReadable, complete, "导入表完整读取");
    // "No imports" is only interesting when there is code that would have needed
    // them. Measured on this machine's System32, 39 % of files declare no import
    // directory — nearly all of them resource-only `.mui` satellites, which are
    // legitimately import-less. Without this guard the fact fires on four thousand
    // innocent files.
    let code_len = code_bytes;
    if imports.present {
        facts.set_bool(FactId::ImportsAbsent, false, "存在导入目录");
    } else if has_code {
        facts.set_bool_at(
            FactId::ImportsAbsent,
            true,
            code_len as f32,
            "有代码但无导入目录",
        );
    } else {
        facts.set_bool_at(
            FactId::ImportsAbsent,
            false,
            code_len as f32,
            "无代码节区（资源型文件）",
        );
    }

    // Static-runtime binaries legitimately import almost nothing. Recognising that
    // here — rather than in a rule — keeps the rule language free of negation.
    let static_runtime = pe::directory(info, dirs_index::DIR_COM).size > 0 || has_go_section(info);
    if !has_code {
        // Nothing to import anything for.
        facts.set_bool(FactId::ImportsSparse, false, "无代码节区（资源型文件）");
    } else if !imports.present {
        // Maximal sparsity, and `imports.absent` already says why.
        facts.set_bool_at(FactId::ImportsSparse, true, 0.0, "无导入目录");
    } else if complete {
        if static_runtime {
            // The observation is fully explained by the runtime, so it is not
            // evidence of stripping. Handling this in the detector rather than in a
            // rule is deliberate: an explanation that *accounts for* an observation
            // belongs here, whereas the `benign` rail on a hypothesis is for
            // explanations that *conflict* with it.
            facts.set_bool_at(
                FactId::ImportsSparse,
                false,
                imports.function_count as f32,
                "静态运行时解释导入稀疏",
            );
        } else {
            facts.set_bool_at(
                FactId::ImportsSparse,
                imports.function_count <= SPARSE_IMPORT_MAX,
                imports.function_count as f32,
                "导入函数数量",
            );
        }
    } else {
        facts.set_unknown(FactId::ImportsSparse, "导入表未完整读取");
    }

    facts.set(
        FactId::ImportsLoader,
        presence(imports.has(cap::LOADER), complete),
        f32::NAN,
        "运行期装载 API",
    );
    facts.set(
        FactId::ImportsNetwork,
        presence(imports.has(cap::NETWORK), complete),
        f32::NAN,
        "网络 API",
    );
    facts.set(
        FactId::ImportsCredential,
        presence(imports.has(cap::CREDENTIAL), complete),
        f32::NAN,
        "凭据访问 API",
    );
    let trio = imports.caps & (cap::ALLOC_REMOTE | cap::WRITE_REMOTE | cap::EXEC_REMOTE)
        == (cap::ALLOC_REMOTE | cap::WRITE_REMOTE | cap::EXEC_REMOTE);
    facts.set(
        FactId::ImportsInjectionTrio,
        presence(trio, complete),
        f32::NAN,
        "远程注入三件套",
    );
    facts.set(
        FactId::ImportsAntiDebug,
        presence(imports.has(cap::ANTI_DEBUG), complete),
        f32::NAN,
        "反调试 API",
    );
    facts.set(
        FactId::ImportsProcessEnum,
        presence(imports.has(cap::PROCESS_ENUM), complete),
        f32::NAN,
        "进程枚举 API",
    );
    facts.set(
        FactId::ImportsCrypto,
        presence(imports.has(cap::CRYPTO), complete),
        f32::NAN,
        "加密 API",
    );
    facts.set(
        FactId::ImportsPersistence,
        presence(imports.has(cap::PERSISTENCE), complete),
        f32::NAN,
        "持久化接口",
    );
    let dyn_only = imports.has(cap::DYNAMIC_RESOLVE)
        && imports.present
        && imports.function_count <= DYNRESOLVE_IMPORT_MAX;
    let dyn_value = if !has_code || !imports.present {
        // No table to be sparse relative to.
        Tri::False
    } else {
        presence(dyn_only, complete)
    };
    facts.set(
        FactId::ImportsDynamicResolveOnly,
        dyn_value,
        f32::NAN,
        "动态解析且静态导入稀疏",
    );
}

/// True when the DOS stub carries a key-consistent Rich header.
///
/// The check is not "does the string `Rich` appear" — that is trivially forgeable.
/// It validates the XOR key against the `DanS` marker, which is how the toolchain
/// that produced the header would have written it.
fn rich_header_valid(bytes: &[u8], e_lfanew: usize) -> bool {
    const DANS: u32 = 0x536E_6144;
    if e_lfanew < 0x84 || e_lfanew > bytes.len() {
        return false;
    }
    let marker = e_lfanew - 8;
    if bytes.get(marker..marker + 4) != Some(b"Rich") {
        return false;
    }
    let Some(key_bytes) = bytes.get(marker + 4..marker + 8) else {
        return false;
    };
    let key = u32::from_le_bytes([key_bytes[0], key_bytes[1], key_bytes[2], key_bytes[3]]);
    let Some(dans_bytes) = bytes.get(0x80..0x84) else {
        return false;
    };
    let dans =
        u32::from_le_bytes([dans_bytes[0], dans_bytes[1], dans_bytes[2], dans_bytes[3]]) ^ key;
    dans == DANS
}

fn has_tls_callbacks(info: &PeInfo<'_>) -> bool {
    let d = pe::directory(info, dirs_index::DIR_TLS);
    if d.address == 0 || d.size == 0 {
        return false;
    }
    let Some(base) = info.rva_to_offset(d.address) else {
        return false;
    };
    let field = if info.is_64 {
        t3_reader_u64(info.bytes, base + 24)
    } else {
        t3_reader_u32(info.bytes, base + 12).map(u64::from)
    };
    field.is_some_and(|v| v != 0)
}

fn t3_reader_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let s = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn t3_reader_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let s = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

/// Go binaries carry `.gopclntab`/`.go.buildinfo` and legitimately import almost
/// nothing, so they must not read as "imports stripped".
fn has_go_section(info: &PeInfo<'_>) -> bool {
    info.sections().iter().any(|s| {
        let name = s.name_bytes();
        name == b".gopclntab" || name == b".go.buildinfo" || name.starts_with(b".go")
    })
}

#[cfg(test)]
mod tests {
    use super::{presence, Kind};
    use t3_core::Tri;

    #[test]
    fn presence_is_the_whole_soundness_story() {
        // Seen: decided regardless of how far the walk got.
        assert_eq!(presence(true, true), Tri::True);
        assert_eq!(presence(true, false), Tri::True);
        // Not seen but finished looking: genuinely absent.
        assert_eq!(presence(false, true), Tri::False);
        // Not seen and did not finish: must not read as absent.
        assert_eq!(presence(false, false), Tri::Unknown);
    }

    #[test]
    fn garbage_is_unknown_format_not_a_panic() {
        let mut s = 0x1234_5678u32;
        for len in [0usize, 1, 63, 64, 4096] {
            let mut data = vec![0u8; len];
            for b in &mut data {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                *b = s as u8;
            }
            let analysis = super::analyze(&data);
            assert_eq!(analysis.kind, Kind::Unknown, "len={len}");
            assert_eq!(analysis.regions.len(), 1);
            assert_eq!(analysis.regions[0].data.len(), len);
        }
    }

    #[test]
    fn mz_without_pe_header_is_reported_as_damaged() {
        let mut data = vec![0u8; 512];
        data[0] = b'M';
        data[1] = b'Z';
        // e_lfanew points somewhere with no PE signature.
        data[0x3C] = 0x40;
        let analysis = super::analyze(&data);
        assert_eq!(analysis.kind, Kind::Pe);
        assert!(analysis.facts.get(t3_core::FactId::FormatPe).is_true());
        assert!(analysis
            .facts
            .get(t3_core::FactId::SectionMalformed)
            .is_true());
    }
}
