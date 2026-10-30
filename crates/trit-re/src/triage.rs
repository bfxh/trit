//! 台账：把一个文件压成"事实 + 判定"。
//!
//! 判定层不知道格式细节，只消费事实表；这样换解析器、加新证据面都不会
//! 改动判定表达式。

use trit_core::value::Ternary;

use crate::elf;
use crate::hash;
use crate::imports;
use crate::pe;
use crate::verdict::{self, Judgment};
use crate::witness::Facts;
use crate::xref::{self, XrefReport};

/// 高熵阈值：8 bit/byte 里 7.2 已经接近压缩/加密数据的典型值。
pub const HIGH_ENTROPY: f64 = 7.2;
/// 叠加段高熵阈值。
pub const OVERLAY_HIGH_ENTROPY: f64 = 7.0;
/// 参与熵判定的最小字节数，避免把几个字节的噪声当结论。
pub const MIN_ENTROPY_BYTES: u32 = 4096;
/// 静态字符串扫描上限。
pub const STRING_SCAN_LIMIT: usize = 16 * 1024 * 1024;

const PACKER_NAMES: &[&str] = &[
    "upx0",
    "upx1",
    "upx2",
    ".aspack",
    ".adata",
    ".themida",
    ".vmp0",
    ".vmp1",
    ".mpress1",
    ".mpress2",
    ".petite",
    ".nsp0",
    ".nsp1",
    "pecompact",
    ".packed",
    ".enigma1",
    ".enigma2",
    ".boom",
    ".ccg",
    ".perplex",
];

/// 一次完整分析的结果。
#[derive(Debug, Clone)]
pub struct Triage {
    /// 内容哈希（小写十六进制）。
    pub sha256: String,
    /// 字节数。
    pub size: u64,
    /// 格式短名。
    pub format: String,
    /// 架构短名。
    pub arch: String,
    /// 节数。
    pub sections: usize,
    /// 导入函数数；`None` 表示导入表不可信。
    pub imported_functions: Option<usize>,
    /// 事实表。
    pub facts: Facts,
    /// 导入函数交叉引用；架构不支持时为 `Some` 但状态全为未知。
    pub xref: Option<XrefReport>,
    /// 判定结果。
    pub judgments: Vec<Judgment>,
    /// 补充说明（不参与判定）。
    pub notes: Vec<String>,
}

/// 分析一个样本。
pub fn analyze(bytes: &[u8]) -> Triage {
    let mut triage = Triage {
        sha256: hash::sha256_hex(bytes),
        size: bytes.len() as u64,
        format: "unknown".to_string(),
        arch: "unknown".to_string(),
        sections: 0,
        imported_functions: None,
        facts: Facts::new(),
        xref: None,
        judgments: Vec::new(),
        notes: Vec::new(),
    };

    if bytes.starts_with(b"MZ") {
        match pe::parse(bytes) {
            Ok(info) => {
                fill_pe(bytes, &info, &mut triage);
                triage.xref = Some(xref::scan_pe(bytes, &info));
            }
            Err(error) => triage
                .notes
                .push(format!("PE 解析失败：{error}（判定保持未知）")),
        }
    } else if bytes.starts_with(b"\x7fELF") {
        match elf::parse(bytes) {
            Ok(info) => {
                fill_elf(bytes, &info, &mut triage);
                triage.xref = Some(xref::scan_elf(bytes, &info));
            }
            Err(error) => triage
                .notes
                .push(format!("ELF 解析失败：{error}（判定保持未知）")),
        }
    } else {
        triage
            .notes
            .push("未识别的格式：只支持 PE32/PE32+ 与小端 ELF".to_string());
    }

    if let Some(report) = &triage.xref {
        triage.facts.set(
            "xref.scan_complete",
            if report.complete {
                Ternary::True
            } else {
                Ternary::Unknown
            },
            format!(
                "扫描 {} 字节，导入 {} 条",
                report.scanned_bytes,
                report.imports.len()
            ),
        );
        triage.facts.set(
            "xref.has_unreferenced",
            if report.complete {
                Ternary::from(report.unreferenced() > 0)
            } else {
                Ternary::Unknown
            },
            format!(
                "有调用点 {} / 无调用点 {} / 未知 {}",
                report.referenced(),
                report.unreferenced(),
                report.unknown()
            ),
        );
        triage.notes.push(format!(
            "交叉引用：{} 字节；有调用点 {}，无调用点 {}，未知 {}",
            report.scanned_bytes,
            report.referenced(),
            report.unreferenced(),
            report.unknown()
        ));
        triage.notes.extend(report.notes.iter().cloned());
    }

    triage.judgments = verdict::evaluate(&verdict::default_hypotheses(), &triage.facts);
    triage
}

fn fill_pe(bytes: &[u8], info: &pe::PeInfo, triage: &mut Triage) {
    triage.format = info.format().to_string();
    triage.arch = info.arch().to_string();
    triage.sections = info.sections.len();
    triage.imported_functions = info
        .imports
        .as_ref()
        .map(|dlls| dlls.iter().map(|dll| dll.functions.len()).sum());
    let facts = &mut triage.facts;
    let notes = &mut triage.notes;

    facts.set(
        "authenticode.present",
        if info.has_signed_data {
            Ternary::True
        } else if info.has_security_dir {
            Ternary::Unknown
        } else {
            Ternary::False
        },
        if info.has_signed_data {
            "安全目录里找到 signedData 结构（未验证证书链）"
        } else if info.has_security_dir {
            "有安全目录但没找到 signedData 结构"
        } else {
            "没有安全目录（无 Authenticode 签名）"
        },
    );
    facts.set(
        "authenticode.verified",
        Ternary::Unknown,
        "本工具不做证书链/时间戳验证",
    );

    set_section_entropy_fact(
        facts,
        info.sections
            .iter()
            .filter(|section| section.executable() || section.writable())
            .filter(|section| section.raw_size >= MIN_ENTROPY_BYTES)
            .map(|section| (section.entropy, section.name.as_str())),
    );
    set_packer_name_fact(
        facts,
        info.sections.iter().map(|section| section.name.as_str()),
    );

    match info.entry_in_executable() {
        Some(true) => facts.set_bool("entry.outside_code", false, "入口落在可执行节内"),
        Some(false) => facts.set_bool("entry.outside_code", true, "入口不在任何可执行节内"),
        None => facts.set(
            "entry.outside_code",
            Ternary::Unknown,
            "入口 RVA 无法映射到节（或为 0）",
        ),
    }

    set_overlay_facts(
        facts,
        info.overlay
            .as_ref()
            .map(|overlay| (overlay.size, overlay.entropy)),
    );

    let now = unix_now();
    let timestamp = info.timestamp as u64;
    let odd = timestamp == 0 || timestamp > now.saturating_add(86_400);
    facts.set_bool(
        "header.timestamp_odd",
        odd,
        if timestamp == 0 {
            "时间戳为 0".to_string()
        } else {
            format!("COFF 时间戳 {timestamp}")
        },
    );

    facts.set_bool(
        "rich.present",
        info.rich_header,
        if info.rich_header {
            "DOS stub 含 Rich 头（MSVC 工具链痕迹）"
        } else {
            "无 Rich 头"
        },
    );
    match &info.debug_pdb {
        Some(path) => facts.set_bool("debug.pdb_present", true, format!("PDB: {path}")),
        None => facts.set_bool("debug.pdb_present", false, "无 CodeView 调试记录"),
    }

    let flattened: Option<Vec<String>> = info.imports.as_ref().map(|dlls| {
        dlls.iter()
            .flat_map(|dll| {
                dll.functions
                    .iter()
                    .map(move |function| format!("{}!{}", dll.name, function.name))
            })
            .collect()
    });
    set_import_facts(facts, flattened.as_deref());

    let (url_found, url_truncated) = scan_urls(bytes);
    set_url_fact(facts, url_found, url_truncated);

    if let Some(overlay) = &info.overlay {
        notes.push(format!(
            "叠加段 {} 字节，熵 {:.2}",
            overlay.size, overlay.entropy
        ));
    }
    if let Some(pdb) = &info.debug_pdb {
        notes.push(format!("PDB 路径：{pdb}"));
    }
    if !info.exports.is_empty() {
        notes.push(format!("导出 {} 个名字", info.exports.len()));
    }
}

fn fill_elf(bytes: &[u8], info: &elf::ElfInfo, triage: &mut Triage) {
    triage.format = info.format().to_string();
    triage.arch = info.arch().to_string();
    triage.sections = info.sections.len();
    triage.imported_functions = info.imports.as_ref().map(Vec::len);
    let facts = &mut triage.facts;
    let notes = &mut triage.notes;

    facts.set(
        "authenticode.present",
        Ternary::False,
        "ELF 没有 Authenticode 概念（不代表可信）",
    );
    facts.set(
        "authenticode.verified",
        Ternary::Unknown,
        "无签名体系可验证",
    );
    facts.set_bool("debug.pdb_present", false, "ELF 没有 PDB");
    facts.set_bool("rich.present", false, "PE 专有结构");

    set_elf_entropy_fact(facts, info);
    set_packer_name_fact(
        facts,
        info.sections.iter().map(|section| section.name.as_str()),
    );
    match info.entry_in_executable() {
        Some(true) => facts.set_bool("entry.outside_code", false, "入口落在可执行段内"),
        Some(false) => facts.set_bool("entry.outside_code", true, "入口不在可执行段内"),
        None => facts.set(
            "entry.outside_code",
            Ternary::Unknown,
            "没有程序头或入口为 0，无法判定",
        ),
    }
    set_overlay_facts(
        facts,
        info.overlay
            .as_ref()
            .map(|overlay| (overlay.size, overlay.entropy)),
    );
    set_import_facts(facts, info.imports.as_deref());

    let (url_found, url_truncated) = scan_urls(bytes);
    set_url_fact(facts, url_found, url_truncated);

    facts.set_bool(
        "build_id.present",
        info.build_id.is_some(),
        info.build_id
            .clone()
            .unwrap_or_else(|| "无 build-id".to_string()),
    );
    facts.set_bool(
        "comment.present",
        info.comment.is_some(),
        info.comment
            .clone()
            .unwrap_or_else(|| "无 .comment".to_string()),
    );

    if let Some(interp) = &info.interp {
        notes.push(format!("解释器：{interp}"));
    }
    if let Some(comment) = &info.comment {
        notes.push(format!("编译器指纹：{comment}"));
    }
    if !info.has_section_headers {
        notes.push("无节表（可能是删节表加壳）".to_string());
    }
}

fn set_section_entropy_fact<'a>(
    facts: &mut Facts,
    sections: impl Iterator<Item = (Option<f64>, &'a str)>,
) {
    let mut best: Option<(f64, &str)> = None;
    let mut unknown = false;
    let mut considered = 0usize;
    for (entropy, name) in sections {
        considered += 1;
        match entropy {
            Some(value) => {
                if best.is_none_or(|(current, _)| value > current) {
                    best = Some((value, name));
                }
            }
            None => unknown = true,
        }
    }
    match best {
        Some((value, name)) if value >= HIGH_ENTROPY => facts.set(
            "section.high_entropy",
            Ternary::True,
            format!("{name} = {value:.2} bit/byte（≥ {HIGH_ENTROPY}）"),
        ),
        Some((value, name)) if !unknown => facts.set(
            "section.high_entropy",
            Ternary::False,
            format!("最高 {name} = {value:.2} bit/byte（< {HIGH_ENTROPY}）"),
        ),
        Some((value, name)) => facts.set(
            "section.high_entropy",
            Ternary::Unknown,
            format!("最高 {name} = {value:.2}，但仍有节数据读不出"),
        ),
        None => facts.set(
            "section.high_entropy",
            if unknown {
                Ternary::Unknown
            } else {
                Ternary::False
            },
            if considered == 0 {
                "没有可参与判定的节".to_string()
            } else {
                "节数据读不出，无法计算熵".to_string()
            },
        ),
    }
}

fn set_elf_entropy_fact(facts: &mut Facts, info: &elf::ElfInfo) {
    let best = info.max_section_entropy();
    let unknown = info.has_unknown_entropy();
    let considered = info
        .sections
        .iter()
        .filter(|section| {
            section.size >= MIN_ENTROPY_BYTES as u64
                && section.flags & (0x1 | 0x4) != 0
                && section.sh_type != 8
        })
        .count();
    match best {
        Some(value) if value >= HIGH_ENTROPY => facts.set(
            "section.high_entropy",
            Ternary::True,
            format!("最高可执行/可写节熵 {value:.2} bit/byte（≥ {HIGH_ENTROPY}）"),
        ),
        Some(value) if unknown => facts.set(
            "section.high_entropy",
            Ternary::Unknown,
            format!("最高 {value:.2} bit/byte，但仍有节数据读不出"),
        ),
        Some(value) => facts.set(
            "section.high_entropy",
            Ternary::False,
            format!("最高可执行/可写节熵 {value:.2} bit/byte（< {HIGH_ENTROPY}）"),
        ),
        None => facts.set(
            "section.high_entropy",
            if unknown {
                Ternary::Unknown
            } else {
                Ternary::False
            },
            if considered == 0 {
                "没有可参与判定的节".to_string()
            } else {
                "节数据读不出，无法计算熵".to_string()
            },
        ),
    }
}

fn set_packer_name_fact<'a>(facts: &mut Facts, names: impl Iterator<Item = &'a str>) {
    let hits: Vec<String> = names
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            PACKER_NAMES.iter().any(|packer| {
                lower == *packer || lower.trim_start_matches('.') == packer.trim_start_matches('.')
            })
        })
        .map(|name| name.to_string())
        .collect();
    if hits.is_empty() {
        facts.set_bool("packer.section_name", false, "节名里没有已知壳特征");
    } else {
        facts.set("packer.section_name", Ternary::True, hits.join(", "));
    }
}

fn set_overlay_facts(facts: &mut Facts, overlay: Option<(usize, f64)>) {
    match overlay {
        Some((size, entropy)) => {
            facts.set_bool("overlay.present", true, format!("{size} 字节"));
            facts.set_bool(
                "overlay.high_entropy",
                size as u32 >= MIN_ENTROPY_BYTES && entropy >= OVERLAY_HIGH_ENTROPY,
                format!("{size} 字节，熵 {entropy:.2}"),
            );
        }
        None => {
            facts.set_bool("overlay.present", false, "最后一个节之后没有数据");
            facts.set_bool("overlay.high_entropy", false, "没有叠加段");
        }
    }
}

fn set_import_facts(facts: &mut Facts, imports: Option<&[String]>) {
    match imports {
        Some(list) => {
            let hits = imports::classify(list);
            for bucket in imports::BUCKETS {
                match hits.get(bucket.id) {
                    Some(matched) => {
                        facts.set(bucket.id, Ternary::True, matched.join(", "));
                    }
                    None => {
                        facts.set(bucket.id, Ternary::False, "完整导入表未命中");
                    }
                }
            }
        }
        None => {
            for bucket in imports::BUCKETS {
                facts.set(bucket.id, Ternary::Unknown, "导入表无法完整解析");
            }
        }
    }
}

fn set_url_fact(facts: &mut Facts, found: bool, truncated: bool) {
    if found {
        facts.set(
            "strings.url",
            Ternary::True,
            "静态字符串里出现 http:// 或 https://",
        );
    } else if truncated {
        facts.set(
            "strings.url",
            Ternary::Unknown,
            format!("只扫描了前 {} MiB", STRING_SCAN_LIMIT / 1024 / 1024),
        );
    } else {
        facts.set_bool("strings.url", false, "完整扫描未见 http(s) URL");
    }
}

fn scan_urls(bytes: &[u8]) -> (bool, bool) {
    let window = &bytes[..bytes.len().min(STRING_SCAN_LIMIT)];
    let found =
        window.windows(7).any(|w| w == b"http://") || window.windows(8).any(|w| w == b"https://");
    (found, bytes.len() > STRING_SCAN_LIMIT)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{HIGH_ENTROPY, analyze, scan_urls};
    use trit_core::value::Ternary;

    #[test]
    fn unknown_format_produces_all_unknown_judgments() {
        let triage = analyze(b"just some text, not a binary");
        assert_eq!(triage.format, "unknown");
        assert!(triage.facts.is_empty());
        assert!(!triage.judgments.is_empty());
        assert!(triage.judgments.iter().all(|j| j.value == Ternary::Unknown));
    }

    #[test]
    fn truncated_pe_is_reported_not_guessed() {
        let mut bytes = b"MZ".to_vec();
        bytes.resize(0x40, 0);
        bytes[0x3c..0x40].copy_from_slice(&0x7fffu32.to_le_bytes());
        let triage = analyze(&bytes);
        assert_eq!(triage.format, "unknown");
        assert!(triage.notes.iter().any(|note| note.contains("PE 解析失败")));
    }

    #[test]
    fn url_scan_reports_truncation() {
        assert_eq!(scan_urls(b"xx http://a xx"), (true, false));
        assert_eq!(scan_urls(b"nothing here"), (false, false));
    }

    #[test]
    fn threshold_is_the_documented_value() {
        assert!((HIGH_ENTROPY - 7.2).abs() < f64::EPSILON);
    }
}
