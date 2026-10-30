//! Portable Executable parsing: headers, sections, imports, overlay, debug data.
//!
//! # What this parser guarantees
//!
//! * No panic, no unwrap, no arithmetic that can wrap. Every read goes through
//!   [`Reader`] and every offset is checked before use.
//! * No allocation for the section table (fixed capacity, see [`MAX_SECTIONS`]).
//! * Nothing is "assumed valid because it usually is". A field that cannot be read
//!   is reported as read-failure, and the caller turns that into `Unknown` rather
//!   than a default.
//!
//! # What it deliberately does not do
//!
//! No disassembly, no relocation, no resource tree walk, no authenticode
//! verification. It reads the *declared* structure, which is what a triage pass
//! needs and what an attacker can most easily lie about — which is exactly why the
//! output is graded rather than trusted.

use crate::reader::Reader;
use crate::region::{Region, RegionKind};
use t3_kernel::entropy;

mod imports;

pub use imports::{classify_import, Imports, PACKER_SECTION_NAMES};
use imports::{eq_ignore_case, parse_imports};

/// Maximum sections retained. The Windows loader itself caps a valid image at 96;
/// real files rarely exceed 20. Beyond this the table is marked truncated, which
/// makes the affected facts `Unknown` instead of silently partial.
pub const MAX_SECTIONS: usize = 48;

/// Maximum import descriptors walked, as a defence against a crafted cyclic table.
const MAX_IMPORT_DESCRIPTORS: usize = 1024;
/// Maximum thunks walked per descriptor.
const MAX_THUNKS: usize = 8192;

// Section characteristics.
const SCN_CNT_CODE: u32 = 0x0000_0020;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SCN_MEM_WRITE: u32 = 0x8000_0000;

// Data directory indices.
/// Export table.
pub const DIR_EXPORT: usize = 0;
/// Import table.
pub const DIR_IMPORT: usize = 1;
/// Resource directory.
pub const DIR_RESOURCE: usize = 2;
/// Authenticode certificate (a file offset, not an RVA).
pub const DIR_SECURITY: usize = 4;
/// Base relocation table.
pub const DIR_BASERELOC: usize = 5;
/// Debug directory.
pub const DIR_DEBUG: usize = 6;
/// TLS directory.
pub const DIR_TLS: usize = 9;
/// CLR (.NET) directory.
pub const DIR_COM: usize = 14;

/// Why parsing failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeError {
    /// No `MZ` signature.
    NotMz,
    /// `MZ` present but `e_lfanew` or the `PE\0\0` signature is unusable.
    BadNtHeader,
    /// The buffer ends before the section table or the optional header.
    Truncated,
}

impl core::fmt::Display for PeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            PeError::NotMz => "no MZ signature",
            PeError::BadNtHeader => "unusable NT header",
            PeError::Truncated => "truncated before the section table",
        })
    }
}

/// A data directory entry.
#[derive(Clone, Copy, Debug, Default)]
pub struct DataDir {
    /// RVA, except for the security directory where it is a file offset.
    pub address: u32,
    /// Size in bytes.
    pub size: u32,
}

/// One section header.
#[derive(Clone, Copy, Default)]
pub struct Section {
    /// Raw 8-byte name, NUL padded.
    pub name: [u8; 8],
    /// Bytes of the name before the first NUL.
    pub name_len: u8,
    /// RVA where the section is mapped.
    pub virtual_address: u32,
    /// Size in memory.
    pub virtual_size: u32,
    /// File offset of the raw data.
    pub raw_offset: u32,
    /// Size of the raw data on disk.
    pub raw_size: u32,
    /// `IMAGE_SCN_*` flags.
    pub characteristics: u32,
}

impl Section {
    /// Section name without trailing NULs.
    #[must_use]
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..usize::from(self.name_len)]
    }

    /// Name as a lossy string, for reports.
    #[must_use]
    pub fn name_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.name_bytes())
    }

    /// Has execute permission.
    #[must_use]
    pub fn is_executable(&self) -> bool {
        self.characteristics & SCN_MEM_EXECUTE != 0
    }

    /// Has write permission.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.characteristics & SCN_MEM_WRITE != 0
    }

    /// Holds code according to the linker.
    #[must_use]
    pub fn is_code(&self) -> bool {
        self.characteristics & SCN_CNT_CODE != 0 || self.is_executable()
    }

    /// Whether `rva` falls inside this section's mapped range.
    ///
    /// The mapped extent is the larger of the virtual size and the raw size: some
    /// linkers leave `VirtualSize` at zero, and some packers do the opposite.
    #[must_use]
    pub fn contains_rva(&self, rva: u32) -> bool {
        if rva < self.virtual_address {
            return false;
        }
        let delta = rva - self.virtual_address;
        delta < self.virtual_size.max(self.raw_size)
    }

    /// Clamped file range of the raw data, or `None` when it lies outside the file.
    #[must_use]
    pub fn raw_range(&self, file_len: usize) -> Option<(usize, usize)> {
        if self.raw_size == 0 {
            return Some((0, 0));
        }
        let start = self.raw_offset as usize;
        let end = start.checked_add(self.raw_size as usize)?;
        if end > file_len {
            return None;
        }
        Some((start, end))
    }
}

/// Overlay: bytes after the last section's raw data.
#[derive(Clone, Copy, Debug, Default)]
pub struct Overlay {
    /// File offset where the overlay starts.
    pub offset: usize,
    /// Length in bytes.
    pub len: usize,
    /// Shannon entropy over the overlay, `None` when it is empty.
    pub entropy: Option<f64>,
    /// True when an Authenticode certificate occupied part of the tail and was
    /// excluded from the overlay measurement.
    pub excluded_certificate: bool,
}

/// Debug directory contents, when a `CodeView` record is present.
#[derive(Clone, Default)]
pub struct DebugInfo {
    /// PDB file name as recorded in the image.
    pub pdb: Vec<u8>,
    /// True when the recorded path looks like a developer's local profile.
    pub developer_path: bool,
}

/// Capability bits inferred from import names.
pub mod cap {
    /// Runtime module loading.
    pub const LOADER: u16 = 1 << 0;
    /// Network access.
    pub const NETWORK: u16 = 1 << 1;
    /// Credential or secret access.
    pub const CREDENTIAL: u16 = 1 << 2;
    /// Remote memory allocation.
    pub const ALLOC_REMOTE: u16 = 1 << 3;
    /// Remote memory write.
    pub const WRITE_REMOTE: u16 = 1 << 4;
    /// Remote thread or APC creation.
    pub const EXEC_REMOTE: u16 = 1 << 5;
    /// Anti-debugging.
    pub const ANTI_DEBUG: u16 = 1 << 6;
    /// Process/thread enumeration.
    pub const PROCESS_ENUM: u16 = 1 << 7;
    /// Cryptography.
    pub const CRYPTO: u16 = 1 << 8;
    /// Service or autostart control.
    pub const PERSISTENCE: u16 = 1 << 9;
    /// Runtime symbol resolution.
    pub const DYNAMIC_RESOLVE: u16 = 1 << 10;
}

/// Everything the PE parser recovered.
#[allow(
    clippy::struct_excessive_bools,
    reason = "PE 头字段本身就是一组独立的布尔标志，合成枚举反而失去与规范的对应"
)]
pub struct PeInfo<'a> {
    /// Buffer the image was parsed from.
    pub bytes: &'a [u8],
    /// True for PE32+.
    pub is_64: bool,
    /// `IMAGE_FILE_MACHINE_*`.
    pub machine: u16,
    /// COFF timestamp.
    pub timestamp: u32,
    /// COFF characteristics.
    pub characteristics: u16,
    /// `IMAGE_OPTIONAL_HEADER::Subsystem`.
    pub subsystem: u16,
    /// `IMAGE_OPTIONAL_HEADER::DllCharacteristics`.
    pub dll_characteristics: u16,
    /// Entry point RVA.
    pub entry_rva: u32,
    /// Preferred load address.
    pub image_base: u64,
    /// Section alignment.
    pub section_alignment: u32,
    /// File alignment.
    pub file_alignment: u32,
    /// Total mapped size.
    pub size_of_image: u32,
    /// Size of the headers on disk.
    pub size_of_headers: u32,
    /// Data directories, padded to 16.
    pub dirs: [DataDir; 16],
    /// True when more directories were declared than fit in `dirs`.
    pub dirs_truncated: bool,
    /// Section headers retained.
    pub sections: [Section; MAX_SECTIONS],
    /// How many section headers were actually stored.
    pub section_count: usize,
    /// How many the header declared.
    pub declared_section_count: u16,
    /// True when the declared count exceeded [`MAX_SECTIONS`].
    pub sections_truncated: bool,
    /// Import summary.
    pub imports: Imports,
    /// Overlay.
    pub overlay: Overlay,
    /// Debug/PDB data.
    pub debug: DebugInfo,
    /// Authenticode certificate present.
    pub signed: bool,
    /// File offset of the NT header, needed to locate the Rich header.
    pub e_lfanew: usize,
}

impl<'a> PeInfo<'a> {
    /// Sections actually present.
    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections[..self.section_count]
    }

    /// Maps an RVA to a file offset.
    ///
    /// RVAs inside the headers map identity, which is what the loader does for the
    /// first `SizeOfHeaders` bytes.
    #[must_use]
    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        if rva < self.size_of_headers {
            let off = rva as usize;
            return (off <= self.bytes.len()).then_some(off);
        }
        for section in self.sections() {
            if section.contains_rva(rva) {
                let delta = rva - section.virtual_address;
                if delta >= section.raw_size {
                    // Inside the virtual tail: no file bytes here.
                    return None;
                }
                let off = (section.raw_offset as usize).checked_add(delta as usize)?;
                return (off <= self.bytes.len()).then_some(off);
            }
        }
        None
    }

    /// Raw bytes of a section, or `None` when the section's raw data is outside the
    /// file (which is what a truncated download or a crafted header looks like).
    #[must_use]
    pub fn section_data(&self, index: usize) -> Option<&[u8]> {
        let section = self.sections.get(index)?;
        let (start, end) = section.raw_range(self.bytes.len())?;
        self.bytes.get(start..end)
    }

    /// Sections as signature-engine regions.
    ///
    /// A section whose raw data is declared but not fully present becomes a
    /// truncated region, so a signature that would have to read the missing bytes
    /// evaluates to `Unknown` rather than to "no match".
    #[must_use]
    pub fn regions(&self) -> Vec<Region<'a>> {
        let mut out = Vec::with_capacity(self.section_count + 2);
        let headers_len = self.size_of_headers as usize;
        let headers_data = self
            .bytes
            .get(..headers_len.min(self.bytes.len()))
            .unwrap_or(&[]);
        out.push(Region::new(
            RegionKind::Headers,
            b"headers",
            0,
            headers_data,
            headers_len,
        ));
        for (i, section) in self.sections().iter().enumerate() {
            let start = section.raw_offset as usize;
            let declared = section.raw_size as usize;
            let available = self.bytes.len().saturating_sub(start).min(declared);
            let data = self.bytes.get(start..start + available).unwrap_or(&[]);
            out.push(Region::new(
                RegionKind::Section(i),
                section.name_bytes(),
                start,
                data,
                declared,
            ));
        }
        if self.overlay.len > 0 {
            let start = self.overlay.offset;
            let available = self.bytes.len().saturating_sub(start).min(self.overlay.len);
            let data = self.bytes.get(start..start + available).unwrap_or(&[]);
            out.push(Region::new(
                RegionKind::Overlay,
                b"overlay",
                start,
                data,
                self.overlay.len,
            ));
        }
        out
    }

    /// Entropy statistics over the readable sections, split by whether the section is
    /// executable.
    ///
    /// Splitting is not a refinement, it is a correction: a packed *code* section and a
    /// *data* section full of compressed assets are different observations, and the
    /// first version of this tool conflated them, flagging every graphics driver in
    /// `System32` as packed.
    #[must_use]
    pub fn entropy_stats(&self, min_bytes: usize) -> EntropyStats {
        let mut stats = EntropyStats::default();
        for (i, section) in self.sections().iter().enumerate() {
            if section.raw_size as usize == 0 {
                continue;
            }
            if (section.raw_size as usize) < min_bytes {
                // Tiny sections have meaningless entropy; a 16-byte stub reports 4 bits
                // by construction and would dominate the maximum.
                if self.section_data(i).is_none() {
                    stats.unreadable = true;
                }
                continue;
            }
            match self.section_data(i) {
                Some(data) => {
                    let e = entropy(data);
                    stats.max_any = Some(stats.max_any.map_or(e, |b: f64| b.max(e)));
                    if section.is_executable() {
                        stats.max_exec = Some(stats.max_exec.map_or(e, |b: f64| b.max(e)));
                    } else {
                        stats.max_data = Some(stats.max_data.map_or(e, |b: f64| b.max(e)));
                    }
                }
                None => stats.unreadable = true,
            }
        }
        stats
    }
}

/// Entropy maxima over a set of sections.
#[derive(Clone, Copy, Debug, Default)]
pub struct EntropyStats {
    /// Highest over all sections.
    pub max_any: Option<f64>,
    /// Highest over executable sections.
    pub max_exec: Option<f64>,
    /// Highest over non-executable sections.
    pub max_data: Option<f64>,
    /// True when a section declared more bytes than the file holds.
    pub unreadable: bool,
}

/// Parses a PE image.
///
/// # Errors
/// Returns [`PeError`] when the file is not a PE image. A file that *is* a PE but
/// whose optional header or section table is damaged parses successfully with
/// `section_count == 0`; callers see the damage through the derived facts rather
/// than through an error, because "damaged PE" is itself a finding.
pub fn parse(bytes: &[u8]) -> Result<PeInfo<'_>, PeError> {
    let r = Reader::le(bytes);
    if !r.tag_is(0, b"MZ") {
        return Err(PeError::NotMz);
    }
    let e_lfanew = r.u32(0x3C).ok_or(PeError::BadNtHeader)? as usize;
    if !r.tag_is(e_lfanew, b"PE\0\0") {
        return Err(PeError::BadNtHeader);
    }

    let coff = e_lfanew.checked_add(4).ok_or(PeError::BadNtHeader)?;
    let machine = r.u16(coff).ok_or(PeError::Truncated)?;
    let declared_section_count = r.u16(coff + 2).ok_or(PeError::Truncated)?;
    let timestamp = r.u32(coff + 4).ok_or(PeError::Truncated)?;
    let size_of_optional_header = r.u16(coff + 16).ok_or(PeError::Truncated)?;
    let characteristics = r.u16(coff + 18).ok_or(PeError::Truncated)?;

    let opt = coff + 20;
    let magic = r.u16(opt).ok_or(PeError::Truncated)?;
    let (is_64, dir_count_off, dir_base) = match magic {
        0x010B => (false, opt + 92, opt + 96),
        0x020B => (true, opt + 108, opt + 112),
        // 0x0107 is a ROM image; anything else is not a supported PE.
        _ => return Err(PeError::BadNtHeader),
    };

    let entry_rva = r.u32(opt + 16).ok_or(PeError::Truncated)?;
    let image_base = if is_64 {
        r.u64(opt + 24).ok_or(PeError::Truncated)?
    } else {
        u64::from(r.u32(opt + 28).ok_or(PeError::Truncated)?)
    };
    let section_alignment = r.u32(opt + 32).unwrap_or(0);
    let file_alignment = r.u32(opt + 36).unwrap_or(0);
    let size_of_image = r.u32(opt + 56).unwrap_or(0);
    let size_of_headers = r.u32(opt + 60).unwrap_or(0);
    let subsystem = r.u16(opt + 68).unwrap_or(0);
    let dll_characteristics = r.u16(opt + 70).unwrap_or(0);

    // Directories: never read past the declared optional header, so a small
    // SizeOfOptionalHeader cannot make us walk into the section table.
    let declared_dirs = r.u32(dir_count_off).unwrap_or(0) as usize;
    let opt_end = opt
        .checked_add(size_of_optional_header as usize)
        .unwrap_or(opt);
    let mut dirs = [DataDir::default(); 16];
    let mut dirs_truncated = false;
    for (i, slot) in dirs.iter_mut().enumerate() {
        if i >= declared_dirs {
            break;
        }
        let at = dir_base + i * 8;
        if at + 8 > opt_end {
            dirs_truncated = true;
            break;
        }
        let (Some(address), Some(size)) = (r.u32(at), r.u32(at + 4)) else {
            dirs_truncated = true;
            break;
        };
        *slot = DataDir { address, size };
    }

    // Section table.
    let table = opt
        .checked_add(size_of_optional_header as usize)
        .ok_or(PeError::Truncated)?;
    let wants = usize::from(declared_section_count);
    let stored = wants.min(MAX_SECTIONS);
    let mut sections = [Section::default(); MAX_SECTIONS];
    let mut section_count = 0usize;
    for i in 0..stored {
        let at = table + i * 40;
        let Some(name_slice) = r.slice(at, 8) else {
            return Err(PeError::Truncated);
        };
        let mut name = [0u8; 8];
        name.copy_from_slice(name_slice);
        let name_len = name.iter().position(|&b| b == 0).unwrap_or(8) as u8;
        sections[section_count] = Section {
            name,
            name_len,
            virtual_size: r.u32(at + 8).unwrap_or(0),
            virtual_address: r.u32(at + 12).unwrap_or(0),
            raw_size: r.u32(at + 16).unwrap_or(0),
            raw_offset: r.u32(at + 20).unwrap_or(0),
            characteristics: r.u32(at + 36).unwrap_or(0),
        };
        section_count += 1;
    }

    let mut info = PeInfo {
        bytes,
        is_64,
        machine,
        timestamp,
        characteristics,
        subsystem,
        dll_characteristics,
        entry_rva,
        image_base,
        section_alignment,
        file_alignment,
        size_of_image,
        size_of_headers,
        dirs,
        dirs_truncated,
        sections,
        section_count,
        declared_section_count,
        sections_truncated: wants > MAX_SECTIONS,
        imports: Imports::default(),
        overlay: Overlay::default(),
        debug: DebugInfo::default(),
        signed: dirs[DIR_SECURITY].size > 8,
        e_lfanew,
    };

    info.imports = parse_imports(&info);
    info.overlay = compute_overlay(&info);
    info.debug = parse_debug(&info);
    Ok(info)
}

/// The directory describing, in order.
#[inline]
fn dir(info: &PeInfo<'_>, index: usize) -> DataDir {
    info.dirs.get(index).copied().unwrap_or_default()
}

fn compute_overlay(info: &PeInfo<'_>) -> Overlay {
    let mut raw_end = (info.size_of_headers as usize).min(info.bytes.len());
    for section in info.sections() {
        if section.raw_size == 0 {
            continue;
        }
        let end = (section.raw_offset as usize).saturating_add(section.raw_size as usize);
        raw_end = raw_end.max(end.min(info.bytes.len()));
    }
    let len = info.bytes.len();
    if raw_end >= len {
        return Overlay::default();
    }

    // An Authenticode certificate lives after the last section and must not be
    // counted as overlay: doing so is the single most common cause of "signed
    // Microsoft binary detected as packed".
    let sec = dir(info, DIR_SECURITY);
    let mut excluded = false;
    let mut end = len;
    if sec.size > 8 && sec.address != 0 {
        let cert_start = sec.address as usize;
        let cert_end = cert_start.saturating_add(sec.size as usize);
        if cert_start >= raw_end && cert_end <= len {
            excluded = true;
            end = cert_start;
        }
    }

    if end <= raw_end {
        let len = 0;
        return Overlay {
            offset: raw_end,
            len,
            entropy: None,
            excluded_certificate: excluded,
        };
    }
    let len = end - raw_end;
    let data = &info.bytes[raw_end..end];
    Overlay {
        offset: raw_end,
        len,
        entropy: Some(entropy(data)),
        excluded_certificate: excluded,
    }
}

fn parse_debug(info: &PeInfo<'_>) -> DebugInfo {
    let mut out = DebugInfo::default();
    let d = dir(info, DIR_DEBUG);
    if d.address == 0 || d.size < 28 {
        return out;
    }
    let Some(base) = info.rva_to_offset(d.address) else {
        return out;
    };
    let count = (d.size as usize / 28).min(64);
    let r = Reader::le(info.bytes);
    for i in 0..count {
        let at = base + i * 28;
        let Some(kind) = r.u32(at + 12) else {
            break;
        };
        if kind != 2 {
            // 2 == IMAGE_DEBUG_TYPE_CODEVIEW
            continue;
        }
        let size = r.u32(at + 16).unwrap_or(0) as usize;
        let ptr = r.u32(at + 24).unwrap_or(0) as usize;
        if ptr == 0 || size < 8 || ptr >= info.bytes.len() {
            continue;
        }
        let data = &info.bytes[ptr..info.bytes.len().min(ptr + size)];
        // "RSDS" (PDB 7.0) or "NB10" (PDB 2.0).
        let header = if data.len() > 24 && data.starts_with(b"RSDS") {
            24
        } else if data.len() > 16 && data.starts_with(b"NB10") {
            16
        } else {
            continue;
        };
        let path = &data[header.min(data.len())..];
        let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
        out.pdb = path[..end.min(260)].to_vec();
        out.developer_path = looks_like_developer_path(&out.pdb);
        return out;
    }
    out
}

/// True when a PDB path points into a user profile or a desktop folder.
fn looks_like_developer_path(path: &[u8]) -> bool {
    const MARKERS: [&[u8]; 6] = [
        b"\\users\\",
        b"/users/",
        b"/home/",
        b"\\desktop\\",
        b"\\documents\\",
        b"/desktop/",
    ];
    let lower: Vec<u8> = path.iter().map(u8::to_ascii_lowercase).collect();
    MARKERS
        .iter()
        .any(|m| lower.windows(m.len()).any(|w| w == *m))
}

/// Tests a section name against [`PACKER_SECTION_NAMES`].
#[must_use]
pub fn is_packer_section(name: &[u8]) -> bool {
    let trimmed: &[u8] = {
        let mut end = name.len();
        while end > 0 && (name[end - 1] == b' ' || name[end - 1] == 0) {
            end -= 1;
        }
        &name[..end]
    };
    PACKER_SECTION_NAMES
        .iter()
        .any(|candidate| eq_ignore_case(trimmed, candidate.as_bytes()))
}

/// True for the linker-generated padding sections named `Pad1`, `Pad2`, `PadN`.
///
/// These matter because they look exactly like a packer's empty stub section: no raw
/// data, a large virtual size, and execute permission. `ntoskrnl.exe` carries three of
/// them, and flagging it was the single most convincing false positive found while
/// calibrating this tool against a live Windows installation. The convention is narrow
/// on purpose — `Pad` followed only by digits — so a section named `Padlock` or
/// `PAYLOAD` still counts as suspicious.
#[must_use]
pub fn is_padding_section(name: &[u8]) -> bool {
    let trimmed: &[u8] = {
        let mut end = name.len();
        while end > 0 && (name[end - 1] == b' ' || name[end - 1] == 0) {
            end -= 1;
        }
        &name[..end]
    };
    let lower: Vec<u8> = trimmed.iter().map(u8::to_ascii_lowercase).collect();
    if lower == b".pad" {
        return true;
    }
    if lower.len() < 4 || &lower[..3] != b"pad" {
        return false;
    }
    lower[3..].iter().all(u8::is_ascii_digit)
}

/// Directory lookups used by the detector, exposed so it does not need to know
/// indices.
#[must_use]
pub fn directory(info: &PeInfo<'_>, index: usize) -> DataDir {
    dir(info, index)
}

/// Indices of the directories the detector asks about.
pub mod dirs_index {
    pub use super::{
        DIR_BASERELOC, DIR_COM, DIR_DEBUG, DIR_EXPORT, DIR_IMPORT, DIR_RESOURCE, DIR_SECURITY,
        DIR_TLS,
    };
}
