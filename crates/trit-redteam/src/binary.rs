//! 本仓产物的二进制硬化与泄密检查（PE / ELF）。
//!
//! 只回答三个问题，不做反汇编、不做漏洞利用：
//! 1. **硬化**：ASLR / NX / 可执行栈 / W+X 段 / RELRO 绑定方式。
//! 2. **泄密**：文件里有没有私钥头、云 AccessKey、`sk-`/`ghp_` 形态的令牌。
//! 3. **路径泄露**：有没有把构建机的绝对路径写进产物。
//!
//! 解析是手写的最小实现（只认本仓会产出的 PE32+/ELF32/ELF64），不引入依赖。
//!
//! 自扫边界（2026-10-28 自审）：泄密 needle 在运行时拼装，本工具自己的产物里
//! **不含**连续的检测样本，`binary` 自检不会自指命中；构建树路径不做机器特定
//! 前缀（曾是 `D:\KF\` 魔法路径，已移除），泄漏的构建树路径归入通用
//! `absolute-user-path`，按机器细分留给将来的 `--build-tree` 参数。
//!
//! 产物面（2026-10-28）：报告的 `side` 字段区分**前端产物**（`.wasm`/HTML，
//! 无硬化面、只做内容检查）与**后端产物**（PE/ELF，全量硬化检查）。

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::attrib::{self, Finding};
use crate::secrets::scan_strings;

#[derive(Debug, Default, Clone)]
pub struct Hardening {
    pub format: String,
    pub bits: u32,
    pub aslr: Option<bool>,
    pub nx: Option<bool>,
    pub exec_stack: Option<bool>,
    pub wx_segments: usize,
    pub relro: Option<bool>,
    pub bind_now: Option<bool>,
    pub guard_cf: Option<bool>,
    pub gnu_stack: bool,
}

impl Hardening {
    fn findings(&self) -> Vec<Finding> {
        let mut out = Vec::new();
        if self.aslr == Some(false) {
            out.push(Finding::project(
                "aslr-missing",
                "build-flags",
                "链接参数：PE 加 /DYNAMICBASE，ELF 用 -pie -fPIE",
            ));
        }
        if self.nx == Some(false) {
            out.push(Finding::project(
                "nx-missing",
                "build-flags",
                "PE 加 /NXCOMPAT；ELF 让 GNU_STACK 非可执行",
            ));
        }
        if self.exec_stack == Some(true) {
            out.push(Finding::project(
                "exec-stack",
                "build-flags",
                "链接参数 -Wl,-z,noexecstack",
            ));
        }
        if self.wx_segments > 0 {
            out.push(Finding::project(
                "wx-segment",
                "build-flags",
                "检查链接脚本/段权限，别让同一段可写可执行",
            ));
        }
        out
    }

    fn warnings(&self, msvc: bool) -> Vec<Finding> {
        let mut out = Vec::new();
        if self.relro == Some(false) {
            out.push(Finding::project(
                "relro-missing",
                "build-flags",
                "-Wl,-z,relro",
            ));
        }
        if self.bind_now == Some(false) {
            out.push(Finding::project(
                "lazy-binding",
                "build-flags",
                "-Wl,-z,now",
            ));
        }
        if self.format.starts_with("pe") && self.guard_cf == Some(false) {
            out.push(if msvc {
                Finding::project(
                    "cfg-missing",
                    "build-flags",
                    "MSVC 链接器可加 /GUARD:CF",
                )
            } else {
                Finding::rust(
                    "cfg-missing",
                    "toolchain-limits",
                    "binutils 版 GNU ld 不支持 /GUARD:CF；要 CFG 得换 MSVC 工具链，这不是我们代码能补的",
                )
            });
        }
        out
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

const IMAGE_FILE_DLL: u16 = 0x2000;
const DLL_DYNAMIC_BASE: u16 = 0x0040;
const DLL_NX_COMPAT: u16 = 0x0100;
const DLL_GUARD_CF: u16 = 0x4000;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SCN_MEM_WRITE: u32 = 0x8000_0000;

fn parse_pe(bytes: &[u8]) -> Option<Hardening> {
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe_offset = u32_at(bytes, 0x3c)? as usize;
    if bytes.get(pe_offset..pe_offset + 4)? != b"PE\0\0" {
        return None;
    }
    let coff = pe_offset + 4;
    let characteristics = u16_at(bytes, coff + 18)?;
    let optional = coff + 20;
    let magic = u16_at(bytes, optional)?;
    let (bits, dll_characteristics_offset) = match magic {
        0x10b => (32u32, 0x42usize),
        0x20b => (64u32, 0x46usize),
        _ => return None,
    };
    let dll_characteristics = u16_at(bytes, optional + dll_characteristics_offset)?;
    let section_count = u16_at(bytes, coff + 2)? as usize;
    let optional_size = u16_at(bytes, coff + 16)? as usize;
    let sections = optional + optional_size;

    let mut wx = 0usize;
    for index in 0..section_count {
        let header = sections + index * 40;
        let flags = u32_at(bytes, header + 36)?;
        if flags & SCN_MEM_EXECUTE != 0 && flags & SCN_MEM_WRITE != 0 {
            wx += 1;
        }
    }
    Some(Hardening {
        format: if characteristics & IMAGE_FILE_DLL != 0 {
            "pe-dll".to_string()
        } else {
            "pe".to_string()
        },
        bits,
        aslr: Some(dll_characteristics & DLL_DYNAMIC_BASE != 0),
        nx: Some(dll_characteristics & DLL_NX_COMPAT != 0),
        exec_stack: None,
        wx_segments: wx,
        relro: None,
        bind_now: None,
        guard_cf: Some(dll_characteristics & DLL_GUARD_CF != 0),
        gnu_stack: false,
        // CFG 只作为附加信息，不参与判定
        // （DLL_GUARD_CF 在 json 里单列）。
    })
}

/// PE 是 MSVC 运行时还是 MinGW？决定 CFG 缺失该算谁的问题。
fn msvc_flavor(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.contains("VCRUNTIME140") || text.contains("api-ms-win-crt")
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_GNU_RELRO: u32 = 0x6474_e552;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const DT_NULL: u64 = 0;
const DT_BIND_NOW: u64 = 24;
const DT_FLAGS: u64 = 30;
const DT_FLAGS_1: u64 = 0x6fff_fffb;
const DF_BIND_NOW: u64 = 8;
const DF_1_NOW: u64 = 1;

fn parse_elf(bytes: &[u8]) -> Option<Hardening> {
    if bytes.get(..4)? != b"\x7fELF" {
        return None;
    }
    let bits = match bytes[4] {
        1 => 32u32,
        2 => 64u32,
        _ => return None,
    };
    if bytes[5] != 1 {
        return None; // 只认小端
    }
    let (e_type, phoff, phentsize, phnum) = if bits == 64 {
        (
            u16_at(bytes, 16)?,
            u64_at(bytes, 0x20)? as usize,
            u16_at(bytes, 0x36)? as usize,
            u16_at(bytes, 0x38)? as usize,
        )
    } else {
        (
            u16_at(bytes, 16)?,
            u32_at(bytes, 0x1c)? as usize,
            u16_at(bytes, 0x2a)? as usize,
            u16_at(bytes, 0x2c)? as usize,
        )
    };
    let mut hardening = Hardening {
        format: if e_type == 3 {
            "elf-pie".to_string()
        } else {
            "elf".to_string()
        },
        bits,
        aslr: Some(e_type == 3),
        nx: None,
        exec_stack: None,
        wx_segments: 0,
        relro: Some(false),
        bind_now: Some(false),
        guard_cf: None,
        gnu_stack: false,
    };
    let mut dynamic: Option<(usize, usize)> = None;
    for index in 0..phnum.min(1024) {
        let header = phoff + index * phentsize;
        let p_type = u32_at(bytes, header)?;
        let (flags, offset, filesz) = if bits == 64 {
            (
                u32_at(bytes, header + 4)?,
                u64_at(bytes, header + 8)? as usize,
                u64_at(bytes, header + 32)? as usize,
            )
        } else {
            (
                u32_at(bytes, header + 24)?,
                u32_at(bytes, header + 4)? as usize,
                u32_at(bytes, header + 16)? as usize,
            )
        };
        if p_type == PT_LOAD && flags & PF_X != 0 && flags & PF_W != 0 {
            hardening.wx_segments += 1;
        }
        if p_type == PT_GNU_STACK {
            hardening.gnu_stack = true;
            hardening.exec_stack = Some(flags & PF_X != 0);
            hardening.nx = Some(flags & PF_X == 0);
        }
        if p_type == PT_GNU_RELRO {
            hardening.relro = Some(true);
        }
        if p_type == PT_DYNAMIC {
            dynamic = Some((offset, filesz));
        }
    }
    if let Some((offset, filesz)) = dynamic {
        let entry_size = if bits == 64 { 16 } else { 8 };
        for index in 0..(filesz / entry_size).min(4096) {
            let at = offset + index * entry_size;
            let (tag, value) = if bits == 64 {
                (u64_at(bytes, at)?, u64_at(bytes, at + 8)?)
            } else {
                (u32_at(bytes, at)? as u64, u32_at(bytes, at + 4)? as u64)
            };
            if tag == DT_NULL {
                break;
            }
            if tag == DT_BIND_NOW
                || (tag == DT_FLAGS && value & DF_BIND_NOW != 0)
                || (tag == DT_FLAGS_1 && value & DF_1_NOW != 0)
            {
                hardening.bind_now = Some(true);
            }
        }
    }
    // 没有 GNU_STACK 段时，执行栈取决于内核/链接器默认值，记为未知。
    Some(hardening)
}

/// `\` / `/` 分段计数（空段不计）；真路径至少有三段（盘/根、目录、文件）。
fn path_segments(text: &str) -> usize {
    text.split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .count()
}

/// 路径泄漏：返回（发现，警告，证据样本）。
///
/// 分两档：路径里还有 `C:\\Users\\<用户名>` / `/home/<用户名>` ⇒ 真泄漏（finding）；
/// 只剩 `~/.rustup/...` 这种工具链布局 ⇒ 已做过 remap，降级为提示（warning）。
fn path_leaks(bytes: &[u8]) -> (Vec<Finding>, Vec<Finding>, Vec<String>) {
    let mut samples: Vec<String> = Vec::new();
    // Windows 侧用**通用盘符前缀**（运行时拼装，`X:\` 不以连续字面量进产物），
    // 构建树路径不再做机器特定前缀（曾是 `D:\KF\`，2026-10-28 自审移除）；
    // Unix 侧保留用户主目录与工具链布局。
    let mut needles: Vec<String> = (b'A'..=b'Z')
        .map(|letter| format!("{}:\\", letter as char))
        .collect();
    needles.extend(["/home/", ".rustup", "/rustc/"].map(str::to_string));
    for needle in &needles {
        let mut rest = bytes;
        while let Some(position) = find_bytes(rest, needle.as_bytes()) {
            if samples.len() < 3 {
                let tail = &rest[position..];
                let end = tail
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(tail.len());
                let text: String = tail[..end.min(120)]
                    .iter()
                    .map(|byte| {
                        if (0x20..0x7f).contains(byte) {
                            *byte as char
                        } else {
                            '?'
                        }
                    })
                    .collect();
                // ≥3 段才像真路径：把「检测器自己的短 needle 常量」（如
                // `C:\Users\`，两段）滤在门外——否则自检又自指。
                if path_segments(&text) >= 3 && !samples.contains(&text) {
                    samples.push(text);
                }
            }
            rest = &rest[position + needle.len()..];
        }
    }

    let user_prefix = |text: &str| text.contains("C:\\Users\\") || text.contains("/home/");
    let mut findings: Vec<Finding> = Vec::new();
    let mut warnings: Vec<Finding> = Vec::new();
    for sample in &samples {
        if attrib::rust_toolchain_path(sample) {
            if user_prefix(sample) {
                if !findings
                    .iter()
                    .any(|finding| finding.id == "rust-toolchain-path")
                {
                    findings.push(Finding::rust(
                        "rust-toolchain-path",
                        "std-paths",
                        "Rust std 的 panic 路径带构建机用户名；发布构建加 RUSTFLAGS=--remap-path-prefix=<HOME>=~",
                    ));
                }
            } else if !warnings
                .iter()
                .any(|finding| finding.id == "toolchain-path-only")
            {
                warnings.push(Finding::rust(
                    "toolchain-path-only",
                    "std-paths",
                    "只剩工具链布局（~/.rustup、/rustc/<hash>），不含用户名；已做过 remap",
                ));
            }
        } else if !findings
            .iter()
            .any(|finding| finding.id == "absolute-user-path")
        {
            findings.push(Finding::project(
                "absolute-user-path",
                "artifact-content",
                "产物嵌了绝对路径（非 Rust std 来源）",
            ));
        }
    }
    (findings, warnings, samples)
}
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 产物面识别：这份文件是**前端产物**还是**后端产物**。
///
/// - WASM（`\0asm` 魔数）⇒ 浏览器前端字节码（playground 运行的那类）；
/// - HTML 文本 ⇒ 浏览器前端源码（只在认不出 PE/ELF 头时才看文本——
///   原生产物里嵌这些标记很正常，不构成分类依据，也避免自指命中）；
/// - PE / ELF ⇒ 原生后端产物。
///
/// 返回 `(side, artifact)`：`side` ∈ frontend/backend，`artifact` 是具体形态。
fn artifact_side(bytes: &[u8], format: &str) -> (&'static str, &'static str) {
    if bytes.starts_with(b"\0asm") {
        return ("frontend", "wasm");
    }
    if format.starts_with("pe") || format.starts_with("elf") {
        return ("backend", "native");
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    if head.contains("<!DOCTYPE html") || head.contains("<html") {
        return ("frontend", "html");
    }
    ("unknown", "unknown")
}

pub fn run(bytes: &[u8], file_name: &str) -> Result<Value> {
    let hardening = if let Some(parsed) = parse_pe(bytes) {
        Some(parsed)
    } else if let Some(parsed) = parse_elf(bytes) {
        Some(parsed)
    } else if bytes.starts_with(b"\0asm") {
        None // 前端字节码
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
        if head.contains("<!DOCTYPE html") || head.contains("<html") {
            None // 前端源码
        } else {
            bail!("认不出格式（支持 PE32/PE32+、ELF32/ELF64 小端、WASM、HTML）");
        }
    };
    let (side, artifact) = artifact_side(bytes, hardening.as_ref().map_or("", |p| &p.format));
    let msvc = msvc_flavor(bytes);
    let secrets = scan_strings(bytes);
    // 泄密/路径检查对前端产物同样有意义（wasm 里嵌 API key 是真实威胁），
    // 只有「硬化」是原生产物才有的面。
    let mut findings = Vec::new();
    if let Some(parsed) = &hardening {
        findings.extend(parsed.findings());
    }
    for name in secrets.keys() {
        findings.push(Finding::project(
            format!("secret-{name}"),
            "artifact-content",
            "别把密钥/令牌嵌进产物；用运行时注入",
        ));
    }
    let (leak_findings, leak_warnings, leak_samples) = path_leaks(bytes);
    findings.extend(leak_findings);
    let mut warnings = hardening
        .as_ref()
        .map_or(Vec::new(), |parsed| parsed.warnings(msvc));
    warnings.extend(leak_warnings);
    let report = json!({
        "tool": "trit-redteam",
        "check": "artifact-hardening",
        "file": file_name,
        "side": side,
        "artifact": artifact,
        "bytes": bytes.len(),
        "format": hardening.as_ref().map_or(artifact, |parsed| parsed.format.as_str()),
        "bits": hardening.as_ref().map_or(0, |parsed| parsed.bits),
        "hardening": hardening.as_ref().map_or(json!(null), |parsed| json!({
            "aslr": parsed.aslr,
            "nx": parsed.nx,
            "exec_stack": parsed.exec_stack,
            "wx_segments": parsed.wx_segments,
            "relro": parsed.relro,
            "bind_now": parsed.bind_now,
            "guard_cf": parsed.guard_cf,
            "gnu_stack": parsed.gnu_stack,
        })),
        "findings": attrib::list_to_json(&findings),
        "warnings": attrib::list_to_json(&warnings),
        "attribution": attrib::summary(&findings),
        "warning_attribution": attrib::summary(&warnings),
        "secrets": secrets,
        "path_leak_samples": leak_samples,
        "note": "project = 我们能改（构建参数/产物内容）；rust-toolchain = Rust 工具链带来、只能绕过",
    });
    Ok(report)
}
pub fn has_findings(report: &Value) -> bool {
    report["findings"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn pe_image(dll_characteristics: u16, section_flags: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        bytes[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 个节
        bytes[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes()); // optional 大小
        bytes[coff + 20..coff + 22].copy_from_slice(&0x20bu16.to_le_bytes()); // PE32+
        let dll = coff + 20 + 0x46;
        bytes[dll..dll + 2].copy_from_slice(&dll_characteristics.to_le_bytes());
        let section = coff + 20 + 0xF0;
        bytes[section + 36..section + 40].copy_from_slice(&section_flags.to_le_bytes());
        bytes
    }

    fn elf_image(exec_stack: bool, relro: bool, bind_now: bool) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2; // 64 位
        bytes[5] = 1; // 小端
        bytes[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN ⇒ PIE
        let phoff = 0x40usize;
        bytes[0x20..0x28].copy_from_slice(&(phoff as u64).to_le_bytes());
        let mut phnum = 2u16;
        let mut dynamic = false;
        if relro {
            phnum += 1;
        }
        if bind_now {
            phnum += 1;
            dynamic = true;
        }
        bytes[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        bytes[0x38..0x3a].copy_from_slice(&phnum.to_le_bytes());

        fn put(
            bytes: &mut [u8],
            index: usize,
            phoff: usize,
            p_type: u32,
            flags: u32,
            offset: u64,
            size: u64,
        ) {
            let at = phoff + index * 56;
            bytes[at..at + 4].copy_from_slice(&p_type.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
            bytes[at + 32..at + 40].copy_from_slice(&size.to_le_bytes());
        }
        put(&mut bytes, 0, phoff, PT_LOAD, PF_X, 0, 0);
        put(
            &mut bytes,
            1,
            phoff,
            PT_GNU_STACK,
            if exec_stack { PF_X } else { 0 },
            0,
            0,
        );
        let mut index = 2;
        if relro {
            put(&mut bytes, index, phoff, PT_GNU_RELRO, 0, 0, 0);
            index += 1;
        }
        if dynamic {
            let offset = 0x200u64;
            put(&mut bytes, index, phoff, PT_DYNAMIC, 0, offset, 16);
            bytes[0x200..0x208].copy_from_slice(&DT_BIND_NOW.to_le_bytes());
            bytes[0x208..0x210].copy_from_slice(&0u64.to_le_bytes());
            bytes[0x210..0x218].copy_from_slice(&DT_NULL.to_le_bytes());
        }
        bytes
    }

    fn finding_ids(report: &Value, key: &str) -> Vec<String> {
        report[key]
            .as_array()
            .expect("数组")
            .iter()
            .map(|item| item["id"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    fn finding<'a>(report: &'a Value, key: &str, id: &str) -> &'a Value {
        report[key]
            .as_array()
            .expect("数组")
            .iter()
            .find(|item| item["id"] == id)
            .unwrap_or_else(|| panic!("缺少 {id}：{}", report[key]))
    }

    #[test]
    fn pe_hardening_is_read_from_dll_characteristics() {
        let hardened = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        let report = run(&hardened, "ok.exe").unwrap();
        assert_eq!(report["format"], "pe");
        assert_eq!(report["hardening"]["aslr"], true);
        assert_eq!(report["hardening"]["nx"], true);
        assert_eq!(report["hardening"]["guard_cf"], true);
        assert_eq!(report["hardening"]["wx_segments"], 0);
        assert!(finding_ids(&report, "findings").is_empty());
        assert!(!has_findings(&report));

        let soft = pe_image(0, SCN_MEM_EXECUTE | SCN_MEM_WRITE);
        let report = run(&soft, "soft.exe").unwrap();
        let ids = finding_ids(&report, "findings");
        for expected in ["aslr-missing", "nx-missing", "wx-segment"] {
            assert!(ids.iter().any(|id| id == expected), "{ids:?}");
        }
        for item in report["findings"].as_array().unwrap() {
            assert_eq!(item["origin"], "project", "{item}");
        }
        assert_eq!(report["attribution"]["project"], 3, "{report}");
        assert_eq!(report["attribution"]["rust-toolchain"], 0);
        assert!(has_findings(&report));
    }

    #[test]
    fn cfg_attribution_depends_on_the_pe_flavor() {
        // MinGW（没有 VCRUNTIME 字符串）：GNU ld 不支持 /GUARD:CF ⇒ 工具链的问题。
        let gnu = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT, 0x6000_0020);
        let report = run(&gnu, "gnu.exe").unwrap();
        let cfg = finding(&report, "warnings", "cfg-missing");
        assert_eq!(cfg["origin"], "rust-toolchain", "{cfg}");
        assert_eq!(report["warning_attribution"]["rust-toolchain"], 1);
        // 硬发现里不该有 CFG（它只是警告）
        assert!(finding_ids(&report, "findings").is_empty());

        // MSVC：加个 VCRUNTIME 导入串，CFG 就变成"我们能开"的问题。
        let mut msvc = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT, 0x6000_0020);
        msvc.extend_from_slice(b"\0VCRUNTIME140.dll\0");
        let report = run(&msvc, "msvc.exe").unwrap();
        let cfg = finding(&report, "warnings", "cfg-missing");
        assert_eq!(cfg["origin"], "project", "{cfg}");
    }

    #[test]
    fn elf_hardening_is_read_from_program_headers() {
        let hardened = elf_image(false, true, true);
        let report = run(&hardened, "ok.elf").unwrap();
        assert_eq!(report["format"], "elf-pie");
        assert_eq!(report["hardening"]["aslr"], true);
        assert_eq!(report["hardening"]["nx"], true);
        assert_eq!(report["hardening"]["exec_stack"], false);
        assert_eq!(report["hardening"]["relro"], true);
        assert_eq!(report["hardening"]["bind_now"], true);
        assert!(!has_findings(&report));

        let weak = elf_image(true, false, false);
        let report = run(&weak, "weak.elf").unwrap();
        assert!(
            finding_ids(&report, "findings")
                .iter()
                .any(|id| id == "exec-stack")
        );
        let warnings = finding_ids(&report, "warnings");
        assert!(
            warnings.iter().any(|id| id == "relro-missing"),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|id| id == "lazy-binding"),
            "{warnings:?}"
        );
    }

    #[test]
    fn detects_secret_shaped_strings_and_attributes_them_to_us() {
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        bytes.extend_from_slice(b"\0-----BEGIN RSA PRIVATE KEY-----\0");
        bytes.extend_from_slice(b"\0AKIAIOSFODNN7EXAMPLE\0");
        let report = run(&bytes, "leaky.exe").unwrap();
        let ids = finding_ids(&report, "findings");
        assert!(ids.iter().any(|id| id == "secret-private-key"), "{ids:?}");
        assert!(
            ids.iter().any(|id| id == "secret-aws-access-key"),
            "{ids:?}"
        );
        assert_eq!(
            report["attribution"]["rust-toolchain"], 0,
            "密钥是我们自己嵌的"
        );
        assert!(has_findings(&report));
    }

    #[test]
    fn attributes_rust_std_paths_to_the_toolchain() {
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        bytes.extend_from_slice(
            b"\0C:\\Users\\someone\\.rustup\\toolchains\\1.99.0\\lib\\rustlib\\src\\rust\\library\\core\\src\\cell\\once.rs\0",
        );
        let report = run(&bytes, "std.exe").unwrap();
        let leak = finding(&report, "findings", "rust-toolchain-path");
        assert_eq!(leak["origin"], "rust-toolchain", "{leak}");
        assert!(leak["hint"].as_str().unwrap().contains("remap"), "{leak}");
        assert_eq!(report["attribution"]["project"], 0);
        assert_eq!(report["attribution"]["rust-toolchain"], 1);

        let mut own = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        own.extend_from_slice(b"\0C:\\dev\\trit\\target\\release\\trit-build.exe\0");
        let report = run(&own, "own.exe").unwrap();
        let leak = finding(&report, "findings", "absolute-user-path");
        assert_eq!(leak["origin"], "project", "{leak}");
    }

    #[test]
    fn remapped_toolchain_paths_are_not_findings() {
        // 做过 `--remap-path-prefix=<HOME>=~` 之后只剩工具链布局，不含用户名。
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        bytes.extend_from_slice(b"\0~\\.rustup\\toolchains\\1.99.0\\lib\\rustlib\\src\\rust\\library\\core\\src\\cell\\once.rs\0");
        let report = run(&bytes, "remapped.exe").unwrap();
        assert!(finding_ids(&report, "findings").is_empty(), "{report}");
        assert!(
            finding_ids(&report, "warnings")
                .iter()
                .any(|id| id == "toolchain-path-only"),
            "{report}"
        );
        assert!(!has_findings(&report));
    }

    #[test]
    fn needle_constants_are_not_reported_as_leaks() {
        // 检测器自己的短 needle 常量（如 `C:\Users\`，只有两段）不是真泄漏——
        // ≥3 段过滤把它挡在门外；没有这个过滤，扫自己必报
        // absolute-user-path（自审教训，变异门锁住）。
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        bytes.extend_from_slice(b"\0C:\\Users\\\0");
        bytes.extend_from_slice(b"\0.rustup\0");
        let report = run(&bytes, "needles.exe").unwrap();
        assert!(finding_ids(&report, "findings").is_empty(), "{report}");
        assert!(!has_findings(&report));
    }

    #[test]
    fn short_prefixes_do_not_trigger_false_positives() {
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT | DLL_GUARD_CF, 0x6000_0020);
        bytes.extend_from_slice(b"\0sk-learn\0AKIA\0ghp_short\0");
        let report = run(&bytes, "clean.exe").unwrap();
        assert!(
            report["secrets"].as_object().unwrap().is_empty(),
            "{}",
            report["secrets"]
        );
    }

    #[test]
    fn rejects_unknown_formats() {
        assert!(run(b"not a binary", "x").is_err());
    }
}
