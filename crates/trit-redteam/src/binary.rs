//! 本仓产物的二进制硬化与泄密检查（PE / ELF）。
//!
//! 只回答三个问题，不做反汇编、不做漏洞利用：
//! 1. **硬化**：ASLR / NX / 可执行栈 / W+X 段 / RELRO 绑定方式。
//! 2. **泄密**：文件里有没有私钥头、云 AccessKey、`sk-`/`ghp_` 形态的令牌。
//! 3. **路径泄露**：有没有把构建机的绝对路径写进产物。
//!
//! 解析是手写的最小实现（只认本仓会产出的 PE32+/ELF32/ELF64），不引入依赖。

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};

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
    pub gnu_stack: bool,
}

impl Hardening {
    fn findings(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.aslr == Some(false) {
            out.push("aslr-missing");
        }
        if self.nx == Some(false) {
            out.push("nx-missing");
        }
        if self.exec_stack == Some(true) {
            out.push("exec-stack");
        }
        if self.wx_segments > 0 {
            out.push("wx-segment");
        }
        out
    }

    fn warnings(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.relro == Some(false) {
            out.push("relro-missing");
        }
        if self.bind_now == Some(false) {
            out.push("lazy-binding");
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
        gnu_stack: false,
        // CFG 只作为附加信息，不参与判定
        // （DLL_GUARD_CF 在 json 里单列）。
    })
}

fn pe_guard_cf(bytes: &[u8]) -> Option<bool> {
    let pe_offset = u32_at(bytes, 0x3c)? as usize;
    let coff = pe_offset + 4;
    let optional = coff + 20;
    let magic = u16_at(bytes, optional)?;
    let offset = match magic {
        0x10b => 0x42,
        0x20b => 0x46,
        _ => return None,
    };
    Some(u16_at(bytes, optional + offset)? & DLL_GUARD_CF != 0)
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

const SECRET_PATTERNS: [(&str, &str); 5] = [
    ("private-key", "-----BEGIN "),
    ("aws-access-key", "AKIA"),
    ("openai-style-key", "sk-"),
    ("github-token", "ghp_"),
    ("slack-token", "xoxb-"),
];

fn scan_strings(bytes: &[u8]) -> BTreeMap<&'static str, usize> {
    let text = String::from_utf8_lossy(bytes);
    let mut hits = BTreeMap::new();
    for (name, needle) in SECRET_PATTERNS {
        let count = text.matches(needle).count();
        if count > 0 && plausible_secret(&text, needle) {
            hits.insert(name, count);
        }
    }
    hits
}

/// 降噪：`sk-` / `AKIA` 这类短前缀在正常文本里也会出现，要求后面跟够长的字母数字串。
fn plausible_secret(text: &str, needle: &str) -> bool {
    if needle == "-----BEGIN " {
        return text.contains("PRIVATE KEY-----");
    }
    let min_tail = match needle {
        "AKIA" => 16,
        "ghp_" => 30,
        "sk-" => 20,
        "xoxb-" => 20,
        _ => 8,
    };
    let mut rest = text;
    while let Some(position) = rest.find(needle) {
        let tail = &rest[position + needle.len()..];
        let mut used = 0usize;
        let mut run = 0usize;
        for c in tail.chars() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                run += 1;
                used += c.len_utf8();
            } else {
                break;
            }
        }
        if run >= min_tail {
            return true;
        }
        let step = if used == 0 {
            tail.chars().next().map_or(1, char::len_utf8)
        } else {
            used
        };
        rest = &tail[step..];
    }
    false
}

fn path_leaks(bytes: &[u8]) -> (Vec<&'static str>, Vec<String>) {
    let mut names: Vec<&'static str> = Vec::new();
    let mut samples: Vec<String> = Vec::new();
    for (name, needle) in [
        ("build-tree-path", "D:\\KF\\"),
        ("absolute-user-path", "C:\\Users\\"),
        ("absolute-user-path", "/home/"),
    ] {
        let mut rest = bytes;
        while let Some(position) = find_bytes(rest, needle.as_bytes()) {
            if !names.contains(&name) {
                names.push(name);
            }
            if samples.len() < 3 {
                let tail = &rest[position..];
                let end = tail
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(tail.len());
                let text: String = tail[..end.min(100)]
                    .iter()
                    .map(|byte| {
                        if (0x20..0x7f).contains(byte) {
                            *byte as char
                        } else {
                            '?'
                        }
                    })
                    .collect();
                if !samples.contains(&text) {
                    samples.push(text);
                }
            }
            rest = &rest[position + needle.len()..];
        }
    }
    (names, samples)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn run(bytes: &[u8], file_name: &str) -> Result<Value> {
    let hardening = if let Some(parsed) = parse_pe(bytes) {
        parsed
    } else if let Some(parsed) = parse_elf(bytes) {
        parsed
    } else {
        bail!("认不出格式（只支持 PE32/PE32+ 与 ELF32/ELF64 小端）");
    };
    let secrets = scan_strings(bytes);
    let (leaks, leak_samples) = path_leaks(bytes);
    let findings = hardening.findings();
    let warnings = hardening.warnings();
    let report = json!({
        "tool": "trit-redteam",
        "check": "artifact-hardening",
        "file": file_name,
        "bytes": bytes.len(),
        "format": hardening.format,
        "bits": hardening.bits,
        "hardening": {
            "aslr": hardening.aslr,
            "nx": hardening.nx,
            "exec_stack": hardening.exec_stack,
            "wx_segments": hardening.wx_segments,
            "relro": hardening.relro,
            "bind_now": hardening.bind_now,
            "guard_cf": if hardening.format.starts_with("pe") { pe_guard_cf(bytes) } else { None },
            "gnu_stack": hardening.gnu_stack,
        },
        "findings": findings,
        "warnings": warnings,
        "secrets": secrets,
        "path_leaks": leaks,
        "path_leak_samples": leak_samples,
        "note": "只看硬化与泄密，不做反汇编、不给利用手段",
    });
    Ok(report)
}

pub fn has_findings(report: &Value) -> bool {
    report["findings"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
        || report["secrets"]
            .as_object()
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

    #[test]
    fn pe_hardening_is_read_from_dll_characteristics() {
        let hardened = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT, 0x6000_0020);
        let report = run(&hardened, "ok.exe").unwrap();
        assert_eq!(report["format"], "pe");
        assert_eq!(report["hardening"]["aslr"], true);
        assert_eq!(report["hardening"]["nx"], true);
        assert_eq!(report["hardening"]["wx_segments"], 0);
        assert!(report["findings"].as_array().unwrap().is_empty());
        assert!(!has_findings(&report));

        let soft = pe_image(0, SCN_MEM_EXECUTE | SCN_MEM_WRITE);
        let report = run(&soft, "soft.exe").unwrap();
        let findings = report["findings"].as_array().unwrap();
        assert!(
            findings.iter().any(|item| item == "aslr-missing"),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|item| item == "nx-missing"),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|item| item == "wx-segment"),
            "{findings:?}"
        );
        assert!(has_findings(&report));
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
        let findings = report["findings"].as_array().unwrap();
        assert!(
            findings.iter().any(|item| item == "exec-stack"),
            "{findings:?}"
        );
        let warnings = report["warnings"].as_array().unwrap();
        assert!(
            warnings.iter().any(|item| item == "relro-missing"),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|item| item == "lazy-binding"),
            "{warnings:?}"
        );
    }

    #[test]
    fn detects_secret_shaped_strings_and_path_leaks() {
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT, 0x6000_0020);
        bytes.extend_from_slice(b"\0-----BEGIN RSA PRIVATE KEY-----\0");
        bytes.extend_from_slice(b"\0AKIAIOSFODNN7EXAMPLE\0");
        bytes.extend_from_slice(b"\0D:\\KF\\trit\\target\\release\\trit-build.exe\0");
        let report = run(&bytes, "leaky.exe").unwrap();
        let secrets = report["secrets"].as_object().unwrap();
        assert!(secrets.contains_key("private-key"), "{secrets:?}");
        assert!(secrets.contains_key("aws-access-key"), "{secrets:?}");
        let leaks = report["path_leaks"].as_array().unwrap();
        assert!(
            leaks.iter().any(|item| item == "build-tree-path"),
            "{leaks:?}"
        );
        assert!(has_findings(&report));
    }

    #[test]
    fn short_prefixes_do_not_trigger_false_positives() {
        let mut bytes = pe_image(DLL_DYNAMIC_BASE | DLL_NX_COMPAT, 0x6000_0020);
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
