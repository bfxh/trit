//! Named byte regions of a parsed image, with truncation tracked explicitly.
//!
//! This type is the bridge between the format parsers and the signature engine,
//! and it exists to make one thing impossible to forget: a region can be
//! **declared but not fully present**. A section header may say 0x2000 bytes while
//! the file ends after 0x800. If the matcher only ever received `&[u8]` it would
//! see 0x800 bytes, fail to find a pattern that lives at 0x1000, and report "no
//! match" — a false negative that is invisible to the operator.
//!
//! Carrying `declared_len` alongside `data` lets the matcher distinguish "looked
//! there and it was not present" from "could not look there", and report `Unknown`.
//!
//! # Why the name is copied
//!
//! Section names live inside the parsed structure, so borrowing them would tie a
//! region's lifetime to the parser instead of to the file buffer. Sixteen inline
//! bytes removes that coupling at no measurable cost and keeps `Region<'a>`
//! covariant in a single lifetime: the buffer's.

/// Maximum name length retained. PE names are 8 bytes; ELF names are rarely longer
/// than 16. Longer names are truncated for display only.
pub const MAX_NAME: usize = 16;

/// Which part of the image a region came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    /// The header block at the start of the file.
    Headers,
    /// Section `n` in table order.
    Section(usize),
    /// Bytes after the last section, excluding any Authenticode certificate.
    Overlay,
    /// An ELF program segment.
    Segment(usize),
    /// The whole file.
    WholeFile,
}

/// A named slice of the file with its declared extent.
#[derive(Clone, Copy)]
pub struct Region<'a> {
    /// Provenance.
    pub kind: RegionKind,
    name: [u8; MAX_NAME],
    name_len: u8,
    /// File offset of `data[0]`.
    pub offset: usize,
    /// Bytes actually available.
    pub data: &'a [u8],
    /// Bytes the structure claims to have.
    pub declared_len: usize,
    /// True when `data.len() < declared_len`.
    pub truncated: bool,
}

impl<'a> Region<'a> {
    /// Builds a region, truncating the name to [`MAX_NAME`].
    #[must_use]
    pub fn new(
        kind: RegionKind,
        name: &[u8],
        offset: usize,
        data: &'a [u8],
        declared_len: usize,
    ) -> Self {
        let mut buf = [0u8; MAX_NAME];
        let n = name.len().min(MAX_NAME);
        buf[..n].copy_from_slice(&name[..n]);
        Region {
            kind,
            name: buf,
            name_len: n as u8,
            offset,
            data,
            declared_len,
            truncated: data.len() < declared_len,
        }
    }

    /// Builds a region whose declared length is exactly what is present.
    #[must_use]
    pub fn complete(kind: RegionKind, name: &[u8], offset: usize, data: &'a [u8]) -> Self {
        Region::new(kind, name, offset, data, data.len())
    }

    /// The region's name bytes.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name[..usize::from(self.name_len)]
    }

    /// Bytes that were declared but are not present.
    #[must_use]
    pub fn missing(&self) -> usize {
        self.declared_len.saturating_sub(self.data.len())
    }

    /// Name as a lossy string, for reports.
    #[must_use]
    pub fn name_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.name())
    }

    /// Byte at a virtual position inside the declared extent.
    ///
    /// `None` means the byte is **not decided**: either the region is shorter than
    /// declared, or the index is beyond the declaration entirely.
    #[must_use]
    pub fn byte(&self, index: usize) -> Option<u8> {
        self.data.get(index).copied()
    }
}

impl core::fmt::Debug for Region<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Region({:?} {:?}, {} bytes",
            self.kind,
            self.name_lossy(),
            self.data.len()
        )?;
        if self.truncated {
            write!(f, ", {} missing", self.missing())?;
        }
        f.write_str(")")
    }
}

/// Wraps a whole buffer as a single region.
///
/// Used when the container is unrecognised or damaged: a signature still has a
/// well-defined answer over the bytes that exist, and the region makes clear that
/// there is no structure to scope it to.
#[must_use]
pub fn whole_file(bytes: &[u8]) -> Vec<Region<'_>> {
    vec![Region::complete(RegionKind::WholeFile, b"file", 0, bytes)]
}

#[cfg(test)]
mod tests {
    use super::{whole_file, Region, RegionKind, MAX_NAME};

    #[test]
    fn truncation_is_reported_by_difference() {
        let data = [1u8, 2, 3, 4];
        let r = Region::new(RegionKind::Section(0), b".text", 0x400, &data, 0x1000);
        assert!(r.truncated);
        assert_eq!(r.missing(), 0x1000 - 4);
        assert_eq!(r.byte(3), Some(4));
        assert_eq!(r.byte(4), None, "past the end must be undecided");
    }

    #[test]
    fn complete_region_has_no_missing_bytes() {
        let data = [9u8; 16];
        let r = Region::complete(RegionKind::WholeFile, b"file", 0, &data);
        assert!(!r.truncated);
        assert_eq!(r.missing(), 0);
        assert_eq!(r.name(), b"file");
    }

    #[test]
    fn long_names_are_truncated_not_rejected() {
        let data = [0u8; 1];
        let long = b"a-very-long-section-name-indeed";
        let r = Region::complete(RegionKind::Section(1), long, 0, &data);
        assert_eq!(r.name().len(), MAX_NAME);
        assert!(long.starts_with(r.name()));
    }

    #[test]
    fn non_utf8_names_do_not_panic() {
        let data = [0u8; 1];
        let r = Region::complete(RegionKind::Section(1), &[0xFF, 0xFE, 0x41], 0, &data);
        assert!(r.name_lossy().contains('\u{FFFD}'));
    }

    #[test]
    fn whole_file_wraps_everything() {
        let data = vec![7u8; 100];
        let regions = whole_file(&data);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].data.len(), 100);
        assert_eq!(regions[0].kind, RegionKind::WholeFile);
    }
}
