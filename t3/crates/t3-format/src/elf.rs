//! ELF parsing: 32/64-bit, both byte orders, program and section headers, dynamic
//! symbols, GNU build-id.
//!
//! Scope is deliberate. This reads the metadata a triage pass needs — segment
//! permissions, section layout, exported/dynamic symbol names — and nothing else.
//! There is no relocation processing, no PLT/GOT reconstruction and no
//! cross-reference analysis, because those change the parser's failure modes from
//! "returns `Unknown`" to "returns a wrong answer quickly".

use crate::pe::Imports;
use crate::reader::{Endian, Reader};
use crate::region::{Region, RegionKind};
use t3_kernel::entropy;

/// Maximum program headers retained.
pub const MAX_SEGMENTS: usize = 40;
/// Maximum section headers retained.
pub const MAX_SECTIONS: usize = 96;

const PT_LOAD: u32 = 1;
const PT_NOTE: u32 = 4;
const PT_GNU_STACK: u32 = 0x6474_E551;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

const SHT_SYMTAB: u32 = 2;
const SHT_NOTE: u32 = 7;
const SHT_NOBITS: u32 = 8;
const SHT_DYNSYM: u32 = 11;
const SHF_EXECINSTR: u64 = 0x4;
const SHF_WRITE: u64 = 0x1;

/// Why ELF parsing failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// No `\x7fELF` magic.
    NotElf,
    /// `EI_CLASS` or `EI_DATA` is neither the 32/64 nor the LE/BE encoding.
    BadIdent,
    /// The buffer ends before the header table it points at.
    Truncated,
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ElfError::NotElf => "no ELF magic",
            ElfError::BadIdent => "unsupported ELF class or data encoding",
            ElfError::Truncated => "truncated before a referenced table",
        })
    }
}

/// One program header.
#[derive(Clone, Copy, Default)]
pub struct Segment {
    /// Segment type.
    pub p_type: u32,
    /// Permission flags.
    pub flags: u32,
    /// File offset.
    pub offset: u64,
    /// Virtual address.
    pub vaddr: u64,
    /// Size on disk.
    pub filesz: u64,
    /// Size in memory.
    pub memsz: u64,
}

/// One section header.
#[derive(Clone, Copy, Default)]
pub struct Section {
    /// Offset of the name in the section-name string table.
    pub name_offset: u32,
    /// Section type.
    pub sh_type: u32,
    /// Section flags.
    pub flags: u64,
    /// File offset.
    pub offset: u64,
    /// Size in bytes.
    pub size: u64,
    /// Index of a linked section (string table for symbol sections).
    pub link: u32,
}

/// Everything the ELF parser recovered.
pub struct ElfInfo<'a> {
    /// Source buffer.
    pub bytes: &'a [u8],
    /// 64-bit class.
    pub is_64: bool,
    /// Byte order.
    pub endian: Endian,
    /// `ET_*`.
    pub e_type: u16,
    /// `EM_*`.
    pub machine: u16,
    /// Entry point virtual address.
    pub entry: u64,
    /// Retained program headers.
    pub segments: [Segment; MAX_SEGMENTS],
    /// Number of segments stored.
    pub segment_count: usize,
    /// Retained section headers.
    pub sections: [Section; MAX_SECTIONS],
    /// Number of sections stored.
    pub section_count: usize,
    /// Declared section count (may exceed what was stored).
    pub declared_section_count: usize,
    /// Index of the section-name string table, when valid.
    pub shstrndx: usize,
    /// `PT_GNU_STACK` present and non-executable. `None` when no such segment.
    pub nx_stack: Option<bool>,
    /// Build-id bytes from a GNU note.
    pub build_id: Option<Vec<u8>>,
    /// A `PT_LOAD` segment is both writable and executable.
    pub writable_text: bool,
    /// A `SHT_SYMTAB` section exists.
    pub has_symtab: bool,
    /// Capability bits and counts from the dynamic symbol table.
    pub imports: Imports,
}

impl<'a> ElfInfo<'a> {
    /// Stored segments.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.segment_count]
    }

    /// Stored sections.
    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections[..self.section_count]
    }

    fn reader(&self) -> Reader<'a> {
        Reader::new(self.bytes, self.endian)
    }

    /// Resolves a section name.
    ///
    /// Returns the raw bytes; ELF names are conventionally ASCII but nothing
    /// enforces it.
    #[must_use]
    pub fn section_name(&self, index: usize) -> Option<&'a [u8]> {
        let section = self.sections.get(index)?;
        let strtab = self.sections.get(self.shstrndx)?;
        if strtab.sh_type == SHT_NOBITS {
            return None;
        }
        let base = usize::try_from(strtab.offset).ok()?;
        let len = usize::try_from(strtab.size).ok()?;
        let table = self.reader().slice(base, len)?;
        let off = section.name_offset as usize;
        if off >= table.len() {
            return None;
        }
        let tail = &table[off..];
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        Some(&tail[..end])
    }

    /// Finds a section by exact name.
    #[must_use]
    pub fn find_section(&self, wanted: &[u8]) -> Option<usize> {
        (0..self.section_count).find(|&i| self.section_name(i).is_some_and(|n| n == wanted))
    }

    /// Sections as signature regions, skipping `SHT_NOBITS` (which occupy no file
    /// bytes) and tracking truncation.
    #[must_use]
    pub fn regions(&self) -> Vec<Region<'a>> {
        let mut out = Vec::with_capacity(self.section_count + 1);
        for (i, section) in self.sections().iter().enumerate() {
            if section.sh_type == SHT_NOBITS || section.size == 0 {
                continue;
            }
            let Ok(start) = usize::try_from(section.offset) else {
                continue;
            };
            let Ok(declared) = usize::try_from(section.size) else {
                continue;
            };
            let available = self.bytes.len().saturating_sub(start).min(declared);
            let data = self.bytes.get(start..start + available).unwrap_or(&[]);
            out.push(Region::new(
                RegionKind::Section(i),
                self.section_name(i).unwrap_or(b""),
                start,
                data,
                declared,
            ));
        }
        if let Some(overlay) = self.overlay() {
            out.push(overlay);
        }
        out
    }

    /// Bytes past the last file-backed section.
    #[must_use]
    pub fn overlay(&self) -> Option<Region<'a>> {
        let mut end = 0usize;
        for section in self.sections() {
            if section.sh_type == SHT_NOBITS {
                continue;
            }
            let Ok(start) = usize::try_from(section.offset) else {
                continue;
            };
            let Ok(size) = usize::try_from(section.size) else {
                continue;
            };
            end = end.max(start.saturating_add(size).min(self.bytes.len()));
        }
        for segment in self.segments() {
            let Ok(start) = usize::try_from(segment.offset) else {
                continue;
            };
            let Ok(size) = usize::try_from(segment.filesz) else {
                continue;
            };
            end = end.max(start.saturating_add(size).min(self.bytes.len()));
        }
        let len = self.bytes.len();
        if end >= len {
            return None;
        }
        let remaining = len - end;
        Some(Region::new(
            RegionKind::Overlay,
            b"overlay",
            end,
            self.bytes.get(end..).unwrap_or(&[]),
            remaining,
        ))
    }

    /// Entropy statistics over file-backed sections, split by executability.
    #[must_use]
    pub fn entropy_stats(&self, min_bytes: usize) -> crate::pe::EntropyStats {
        let mut stats = crate::pe::EntropyStats::default();
        for (index, section) in self.sections().iter().enumerate() {
            if section.sh_type == SHT_NOBITS || section.size == 0 {
                continue;
            }
            let Ok(start) = usize::try_from(section.offset) else {
                stats.unreadable = true;
                continue;
            };
            let declared = section.size as usize;
            let available = self.bytes.len().saturating_sub(start).min(declared);
            if available < declared {
                stats.unreadable = true;
                continue;
            }
            if available < min_bytes {
                continue;
            }
            let Some(data) = self.bytes.get(start..start + available) else {
                stats.unreadable = true;
                continue;
            };
            let e = entropy(data);
            stats.max_any = Some(stats.max_any.map_or(e, |b: f64| b.max(e)));
            if section.flags & SHF_EXECINSTR != 0 {
                stats.max_exec = Some(stats.max_exec.map_or(e, |b: f64| b.max(e)));
            } else {
                stats.max_data = Some(stats.max_data.map_or(e, |b: f64| b.max(e)));
            }
            let _ = index;
        }
        stats
    }

    /// True when the image's declared stack segment forbids execution.
    #[must_use]
    pub fn is_pie(&self) -> bool {
        self.e_type == 3
    }
}

/// Parses an ELF image.
///
/// # Errors
/// Returns [`ElfError`] when the file is not an ELF or its ident block is
/// unusable. A truncated table is reported through `Truncated` only when the
/// header table itself cannot be read; a truncated *section* payload parses fine
/// and shows up as a truncated region.
pub fn parse(bytes: &[u8]) -> Result<ElfInfo<'_>, ElfError> {
    if bytes.len() < 16 || !bytes.starts_with(b"\x7fELF") {
        return Err(ElfError::NotElf);
    }
    let is_64 = match bytes[4] {
        1 => false,
        2 => true,
        _ => return Err(ElfError::BadIdent),
    };
    let endian = match bytes[5] {
        1 => Endian::Little,
        2 => Endian::Big,
        _ => return Err(ElfError::BadIdent),
    };
    let r = Reader::new(bytes, endian);

    let e_type = r.u16(16).ok_or(ElfError::Truncated)?;
    let machine = r.u16(18).ok_or(ElfError::Truncated)?;
    let (entry, phoff, shoff, phentsize, phnum, shentsize, shnum, shstrndx) = if is_64 {
        (
            r.u64(24).ok_or(ElfError::Truncated)?,
            r.u64(32).ok_or(ElfError::Truncated)?,
            r.u64(40).ok_or(ElfError::Truncated)?,
            r.u16(54).ok_or(ElfError::Truncated)?,
            r.u16(56).ok_or(ElfError::Truncated)?,
            r.u16(58).ok_or(ElfError::Truncated)?,
            r.u16(60).ok_or(ElfError::Truncated)?,
            r.u16(62).ok_or(ElfError::Truncated)?,
        )
    } else {
        (
            u64::from(r.u32(24).ok_or(ElfError::Truncated)?),
            u64::from(r.u32(28).ok_or(ElfError::Truncated)?),
            u64::from(r.u32(32).ok_or(ElfError::Truncated)?),
            r.u16(42).ok_or(ElfError::Truncated)?,
            r.u16(44).ok_or(ElfError::Truncated)?,
            r.u16(46).ok_or(ElfError::Truncated)?,
            r.u16(48).ok_or(ElfError::Truncated)?,
            r.u16(50).ok_or(ElfError::Truncated)?,
        )
    };

    let shoff = usize::try_from(shoff).map_err(|_| ElfError::Truncated)?;
    let phoff = usize::try_from(phoff).map_err(|_| ElfError::Truncated)?;

    // PN_XNUM: the real program-header count lives in section 0's sh_info.
    let mut phnum = phnum as usize;
    if phnum == 0xFFFF && shoff != 0 && shentsize >= 40 {
        let info_off = if is_64 { 44 } else { 28 };
        if let Some(v) = r.u32(shoff + info_off) {
            phnum = v as usize;
        }
    }

    let mut segments = [Segment::default(); MAX_SEGMENTS];
    let mut segment_count = 0usize;
    if phoff != 0 && phnum > 0 && phentsize > 0 {
        let stride = phentsize as usize;
        for i in 0..phnum.min(MAX_SEGMENTS) {
            let Some(at) = phoff.checked_add(i.checked_mul(stride).ok_or(ElfError::Truncated)?)
            else {
                break;
            };
            let (p_type, flags, offset, vaddr, filesz, memsz) = if is_64 {
                let Some(p_type) = r.u32(at) else { break };
                let flags = r.u32(at + 4).unwrap_or(0);
                let offset = r.u64(at + 8).unwrap_or(0);
                let vaddr = r.u64(at + 16).unwrap_or(0);
                let filesz = r.u64(at + 32).unwrap_or(0);
                let memsz = r.u64(at + 40).unwrap_or(0);
                (p_type, flags, offset, vaddr, filesz, memsz)
            } else {
                let Some(p_type) = r.u32(at) else { break };
                let offset = u64::from(r.u32(at + 4).unwrap_or(0));
                let vaddr = u64::from(r.u32(at + 8).unwrap_or(0));
                let filesz = u64::from(r.u32(at + 16).unwrap_or(0));
                let memsz = u64::from(r.u32(at + 20).unwrap_or(0));
                let flags = r.u32(at + 24).unwrap_or(0);
                (p_type, flags, offset, vaddr, filesz, memsz)
            };
            segments[segment_count] = Segment {
                p_type,
                flags,
                offset,
                vaddr,
                filesz,
                memsz,
            };
            segment_count += 1;
        }
    }

    let mut sections = [Section::default(); MAX_SECTIONS];
    let mut section_count = 0usize;
    let declared_section_count = shnum as usize;
    if shoff != 0 && shnum > 0 && shentsize > 0 {
        let stride = shentsize as usize;
        for i in 0..declared_section_count.min(MAX_SECTIONS) {
            let Some(at) = shoff.checked_add(i.checked_mul(stride).ok_or(ElfError::Truncated)?)
            else {
                break;
            };
            let Some(name_offset) = r.u32(at) else { break };
            let Some(sh_type) = r.u32(at + 4) else { break };
            let (flags, offset, size, link) = if is_64 {
                (
                    r.u64(at + 8).unwrap_or(0),
                    r.u64(at + 24).unwrap_or(0),
                    r.u64(at + 32).unwrap_or(0),
                    r.u32(at + 40).unwrap_or(0),
                )
            } else {
                (
                    u64::from(r.u32(at + 8).unwrap_or(0)),
                    u64::from(r.u32(at + 16).unwrap_or(0)),
                    u64::from(r.u32(at + 20).unwrap_or(0)),
                    r.u32(at + 24).unwrap_or(0),
                )
            };
            sections[section_count] = Section {
                name_offset,
                sh_type,
                flags,
                offset,
                size,
                link,
            };
            section_count += 1;
        }
    }

    let mut info = ElfInfo {
        bytes,
        is_64,
        endian,
        e_type,
        machine,
        entry,
        segments,
        segment_count,
        sections,
        section_count,
        declared_section_count,
        shstrndx: shstrndx as usize,
        nx_stack: None,
        build_id: None,
        writable_text: false,
        has_symtab: false,
        imports: Imports::default(),
    };

    let mut nx_stack = None;
    let mut writable_text = false;
    for segment in info.segments() {
        match segment.p_type {
            PT_GNU_STACK => nx_stack = Some(segment.flags & PF_X == 0),
            PT_LOAD if segment.flags & PF_X != 0 && segment.flags & PF_W != 0 => {
                writable_text = true;
            }
            _ => {}
        }
    }
    info.nx_stack = nx_stack;
    info.writable_text = writable_text;
    info.has_symtab = info.sections().iter().any(|s| s.sh_type == SHT_SYMTAB);
    if !info.writable_text {
        info.writable_text = info
            .sections()
            .iter()
            .any(|s| s.flags & SHF_WRITE != 0 && s.flags & SHF_EXECINSTR != 0);
    }
    info.build_id = find_build_id(&info);
    info.imports = parse_dynamic_symbols(&info);
    Ok(info)
}

/// Walks GNU notes looking for a build-id.
fn find_build_id(info: &ElfInfo<'_>) -> Option<Vec<u8>> {
    let r = info.reader();
    // Prefer a dedicated section, fall back to PT_NOTE segments.
    let mut note_blobs: Vec<(usize, usize)> = Vec::new();
    for (i, section) in info.sections().iter().enumerate() {
        let is_note = section.sh_type == SHT_NOTE
            || info
                .section_name(i)
                .is_some_and(|n| n == b".note.gnu.build-id");
        if is_note {
            if let (Ok(off), Ok(size)) = (
                usize::try_from(section.offset),
                usize::try_from(section.size),
            ) {
                note_blobs.push((off, size));
            }
        }
    }
    if note_blobs.is_empty() {
        for segment in info.segments() {
            if segment.p_type == PT_NOTE {
                if let (Ok(off), Ok(size)) = (
                    usize::try_from(segment.offset),
                    usize::try_from(segment.filesz),
                ) {
                    note_blobs.push((off, size));
                }
            }
        }
    }

    for (start, size) in note_blobs {
        let Some(blob) = r.slice(start, size) else {
            continue;
        };
        let mut at = 0usize;
        // Cap iterations: a malformed note header can otherwise claim zero padding
        // forever.
        for _ in 0..64 {
            let Some(namesz) = r.u32(start + at) else {
                break;
            };
            let Some(descsz) = r.u32(start + at + 4) else {
                break;
            };
            let Some(kind) = r.u32(start + at + 8) else {
                break;
            };
            let Some(name_start) = at.checked_add(12) else {
                break;
            };
            let Some(name_end) = name_start.checked_add(namesz as usize) else {
                break;
            };
            if name_end > blob.len() {
                break;
            }
            let name = &blob[name_start..name_end];
            let desc_start = align4(name_end);
            let desc_end = desc_start.saturating_add(descsz as usize);
            if desc_start > blob.len() {
                break;
            }
            // NT_GNU_BUILD_ID == 3, owner "GNU".
            if kind == 3 && name.starts_with(b"GNU") && desc_end <= blob.len() {
                return Some(blob[desc_start..desc_end].to_vec());
            }
            let next = align4(desc_end);
            if next <= at {
                break;
            }
            at = next;
            if at >= blob.len() {
                break;
            }
        }
    }
    None
}

/// Rounds up to a 4-byte boundary, saturating rather than wrapping.
fn align4(value: usize) -> usize {
    value.saturating_add(3) & !3usize
}

/// Reads dynamic symbol names and maps them to capability bits.
fn parse_dynamic_symbols(info: &ElfInfo<'_>) -> Imports {
    let mut out = Imports::default();
    let r = info.reader();
    let Some(dynsym_index) = info.sections().iter().position(|s| s.sh_type == SHT_DYNSYM) else {
        // No dynsym: statically linked, or stripped of dynamic metadata. Either way
        // the import surface is not "empty", it is unobserved.
        out.complete = false;
        return out;
    };
    let dynsym = info.sections[dynsym_index];
    let Some(strtab_index) = usize::try_from(dynsym.link).ok() else {
        out.complete = false;
        return out;
    };
    let Some(strtab) = info.sections.get(strtab_index) else {
        out.complete = false;
        return out;
    };
    let (Ok(sym_off), Ok(sym_size)) =
        (usize::try_from(dynsym.offset), usize::try_from(dynsym.size))
    else {
        out.complete = false;
        return out;
    };
    let (Ok(str_off), Ok(str_size)) =
        (usize::try_from(strtab.offset), usize::try_from(strtab.size))
    else {
        out.complete = false;
        return out;
    };
    let (Some(sym_table), Some(strings)) = (r.slice(sym_off, sym_size), r.slice(str_off, str_size))
    else {
        out.complete = false;
        return out;
    };

    let ent = if info.is_64 { 24 } else { 16 };
    let name_off_field = 0usize;
    let mut complete = true;
    out.present = true;
    for i in 0..(sym_table.len() / ent) {
        let base = i * ent;
        let Some(name_off) = r.u32(sym_off + base + name_off_field) else {
            complete = false;
            break;
        };
        if name_off == 0 {
            continue;
        }
        let idx = name_off as usize;
        if idx >= strings.len() {
            complete = false;
            continue;
        }
        let tail = &strings[idx..];
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        if end == tail.len() {
            complete = false;
        }
        let name = &tail[..end];
        if name.is_empty() {
            continue;
        }
        out.function_count += 1;
        out.caps |= crate::pe::classify_import(name);
    }
    out.complete = complete;
    out
}
