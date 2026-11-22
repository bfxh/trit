#![forbid(unsafe_code)]
//! 8 KiB page with a small header and CRC32.
//!
//! Layout (little-endian):
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `TRPG` |
//! | 4 | 1 | version (`1`) |
//! | 5 | 1 | kind (`1` data, `2` index, `3` manifest) |
//! | 6 | 2 | flags (v0 must be `0`) |
//! | 8 | 4 | payload length |
//! | 12 | 4 | CRC32 (IEEE) over header bytes `0..12` + payload |
//! | 16 | … | payload |
//!
//! The CRC is for corruption detection only; it is not a security mechanism.
//! A page with a bad magic, version, kind, flags, length, or checksum is
//! rejected with a structured error; parsing never panics on arbitrary bytes.

use std::fmt;

/// Page size in bytes.
pub const PAGE_SIZE: usize = 8192;
/// Header size in bytes.
pub const HEADER_SIZE: usize = 16;
/// Maximum payload size in bytes.
pub const MAX_PAYLOAD: usize = PAGE_SIZE - HEADER_SIZE;
/// Magic bytes at offset 0.
pub const MAGIC: [u8; 4] = *b"TRPG";
/// Page format version.
pub const VERSION: u8 = 1;

/// What the page carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PageKind {
    /// Table data.
    Data = 1,
    /// Index data.
    Index = 2,
    /// Manifest / catalog data.
    Manifest = 3,
}

impl PageKind {
    /// Numeric code stored in the page header.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Parses a numeric code.
    pub fn from_u8(value: u8) -> Result<Self, PageError> {
        match value {
            1 => Ok(PageKind::Data),
            2 => Ok(PageKind::Index),
            3 => Ok(PageKind::Manifest),
            other => Err(PageError::BadKind { found: other }),
        }
    }

    /// Stable short name.
    pub const fn name(self) -> &'static str {
        match self {
            PageKind::Data => "data",
            PageKind::Index => "index",
            PageKind::Manifest => "manifest",
        }
    }
}

/// Structured errors returned by page construction and parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageError {
    /// Magic bytes are not `TRPG`.
    BadMagic {
        /// The bytes that were found.
        found: [u8; 4],
    },
    /// Unsupported version.
    BadVersion {
        /// The version that was found.
        found: u8,
    },
    /// Unknown page kind.
    BadKind {
        /// The kind code that was found.
        found: u8,
    },
    /// Reserved flags are non-zero.
    BadFlags {
        /// The flags that were found.
        found: u16,
    },
    /// Input is shorter/longer than exactly one page.
    Truncated {
        /// Expected byte count.
        expected: usize,
        /// Actual byte count.
        actual: usize,
    },
    /// Payload does not fit in one page.
    PayloadTooLarge {
        /// Requested payload length.
        len: usize,
        /// Maximum payload length.
        max: usize,
    },
    /// Stored CRC does not match the computed CRC.
    ChecksumMismatch {
        /// CRC stored in the header.
        expected: u32,
        /// CRC computed over header + payload.
        actual: u32,
    },
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageError::BadMagic { found } => {
                write!(f, "bad page magic: found {found:?}, expected {MAGIC:?}")
            }
            PageError::BadVersion { found } => {
                write!(f, "bad page version: found {found}, expected {VERSION}")
            }
            PageError::BadKind { found } => write!(f, "bad page kind: found {found}"),
            PageError::BadFlags { found } => write!(f, "bad page flags: found {found}, expected 0"),
            PageError::Truncated { expected, actual } => {
                write!(f, "truncated page: expected {expected} bytes, got {actual}")
            }
            PageError::PayloadTooLarge { len, max } => {
                write!(f, "payload too large: {len} bytes, max {max}")
            }
            PageError::ChecksumMismatch { expected, actual } => write!(
                f,
                "page checksum mismatch: stored {expected:#010x}, computed {actual:#010x}"
            ),
        }
    }
}

impl std::error::Error for PageError {}

/// An owned 8 KiB page.
#[derive(Clone)]
pub struct Page {
    bytes: [u8; PAGE_SIZE],
}

impl Page {
    /// Builds a page from `kind` and `payload`.
    pub fn new(kind: PageKind, payload: &[u8]) -> Result<Self, PageError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(PageError::PayloadTooLarge {
                len: payload.len(),
                max: MAX_PAYLOAD,
            });
        }

        let mut bytes = [0u8; PAGE_SIZE];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4] = VERSION;
        bytes[5] = kind.code();
        bytes[6..8].copy_from_slice(&0u16.to_le_bytes());
        bytes[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);

        let crc = page_crc(&bytes);
        bytes[12..16].copy_from_slice(&crc.to_le_bytes());

        Ok(Self { bytes })
    }

    /// Borrows the raw page bytes.
    pub fn as_bytes(&self) -> &[u8; PAGE_SIZE] {
        &self.bytes
    }

    /// Consumes the page and returns the raw bytes.
    pub fn into_bytes(self) -> [u8; PAGE_SIZE] {
        self.bytes
    }

    /// Parses exactly one page.
    pub fn parse(bytes: &[u8; PAGE_SIZE]) -> Result<PageView<'_>, PageError> {
        let found_magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if found_magic != MAGIC {
            return Err(PageError::BadMagic { found: found_magic });
        }

        let version = bytes[4];
        if version != VERSION {
            return Err(PageError::BadVersion { found: version });
        }

        let kind = PageKind::from_u8(bytes[5])?;
        let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
        if flags != 0 {
            return Err(PageError::BadFlags { found: flags });
        }

        let payload_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        if payload_len > MAX_PAYLOAD {
            return Err(PageError::PayloadTooLarge {
                len: payload_len,
                max: MAX_PAYLOAD,
            });
        }

        let stored_crc = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let computed_crc = page_crc(bytes);
        if stored_crc != computed_crc {
            return Err(PageError::ChecksumMismatch {
                expected: stored_crc,
                actual: computed_crc,
            });
        }

        Ok(PageView {
            bytes,
            kind,
            payload_len,
        })
    }

    /// Parses a byte slice that must be exactly one page.
    pub fn parse_slice(bytes: &[u8]) -> Result<PageView<'_>, PageError> {
        if bytes.len() != PAGE_SIZE {
            return Err(PageError::Truncated {
                expected: PAGE_SIZE,
                actual: bytes.len(),
            });
        }
        let array: &[u8; PAGE_SIZE] = bytes.try_into().map_err(|_| PageError::Truncated {
            expected: PAGE_SIZE,
            actual: bytes.len(),
        })?;
        Self::parse(array)
    }
}

/// A validated view over one page.
#[derive(Clone, Copy, Debug)]
pub struct PageView<'a> {
    bytes: &'a [u8; PAGE_SIZE],
    kind: PageKind,
    payload_len: usize,
}

impl<'a> PageView<'a> {
    /// Page kind.
    pub fn kind(&self) -> PageKind {
        self.kind
    }

    /// Payload length in bytes.
    pub fn payload_len(&self) -> usize {
        self.payload_len
    }

    /// Borrowed payload.
    pub fn payload(&self) -> &'a [u8] {
        &self.bytes[HEADER_SIZE..HEADER_SIZE + self.payload_len]
    }
}

/// CRC32 (IEEE) over one slice.
///
/// This is corruption detection, not a security hash.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_parts(&[], data)
}

/// CRC32 (IEEE) over `first` followed by `second`, without allocating.
pub fn crc32_parts(first: &[u8], second: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    crc = crc32_update(crc, first);
    crc = crc32_update(crc, second);
    !crc
}

fn page_crc(bytes: &[u8; PAGE_SIZE]) -> u32 {
    let payload_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    crc32_parts(
        &bytes[0..12],
        &bytes[HEADER_SIZE..HEADER_SIZE + payload_len],
    )
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        let index = ((crc ^ u32::from(byte)) & 0xff) as usize;
        crc = (crc >> 8) ^ CRC_TABLE[index];
    }
    crc
}

const CRC_TABLE: [u32; 256] = make_crc_table();

const fn make_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xedb8_8320;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_bytes(kind: PageKind, payload: &[u8]) -> [u8; PAGE_SIZE] {
        Page::new(kind, payload).expect("build page").into_bytes()
    }

    #[test]
    fn crc32_known_vector() {
        // Standard IEEE CRC32 check value for "123456789".
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn crc32_parts_matches_concatenation() {
        assert_eq!(crc32_parts(b"1234", b"56789"), crc32(b"123456789"));
    }

    #[test]
    fn roundtrip() {
        let bytes = page_bytes(PageKind::Data, b"hello");
        let view = Page::parse(&bytes).expect("parse");
        assert_eq!(view.kind(), PageKind::Data);
        assert_eq!(view.payload_len(), 5);
        assert_eq!(view.payload(), b"hello");
    }

    #[test]
    fn empty_payload_roundtrip() {
        let bytes = page_bytes(PageKind::Manifest, b"");
        let view = Page::parse(&bytes).expect("parse");
        assert_eq!(view.kind(), PageKind::Manifest);
        assert_eq!(view.payload_len(), 0);
        assert_eq!(view.payload(), b"");
    }

    #[test]
    fn all_kinds_roundtrip() {
        for kind in [PageKind::Data, PageKind::Index, PageKind::Manifest] {
            let bytes = page_bytes(kind, b"payload");
            let view = Page::parse(&bytes).expect("parse");
            assert_eq!(view.kind(), kind);
        }
    }

    #[test]
    fn rejects_too_large_payload() {
        let payload = vec![0u8; MAX_PAYLOAD + 1];
        assert_eq!(
            Page::new(PageKind::Data, &payload)
                .err()
                .expect("must reject"),
            PageError::PayloadTooLarge {
                len: MAX_PAYLOAD + 1,
                max: MAX_PAYLOAD
            }
        );
    }

    #[test]
    fn rejects_truncated_slice() {
        let bytes = page_bytes(PageKind::Data, b"hello");
        assert_eq!(
            Page::parse_slice(&bytes[..100]).expect_err("must reject"),
            PageError::Truncated {
                expected: PAGE_SIZE,
                actual: 100
            }
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        bytes[0] = b'X';
        assert_eq!(
            Page::parse(&bytes).expect_err("must reject"),
            PageError::BadMagic { found: *b"XRPG" }
        );
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        bytes[4] = 2;
        assert_eq!(
            Page::parse(&bytes).expect_err("must reject"),
            PageError::BadVersion { found: 2 }
        );
    }

    #[test]
    fn rejects_bad_kind() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        bytes[5] = 99;
        assert_eq!(
            Page::parse(&bytes).expect_err("must reject"),
            PageError::BadKind { found: 99 }
        );
    }

    #[test]
    fn rejects_bad_flags() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        bytes[6] = 1;
        assert_eq!(
            Page::parse(&bytes).expect_err("must reject"),
            PageError::BadFlags { found: 1 }
        );
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        bytes[HEADER_SIZE] ^= 0xff;
        let error = Page::parse(&bytes).expect_err("must reject");
        match error {
            PageError::ChecksumMismatch { expected, actual } => {
                assert_ne!(expected, actual);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rejects_zeroed_crc_as_half_write() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        bytes[12..16].fill(0);
        let error = Page::parse(&bytes).expect_err("must reject");
        assert!(matches!(error, PageError::ChecksumMismatch { .. }));
    }

    #[test]
    fn rejects_payload_len_beyond_max() {
        let mut bytes = page_bytes(PageKind::Data, b"hello");
        let too_large = (MAX_PAYLOAD + 1) as u32;
        bytes[8..12].copy_from_slice(&too_large.to_le_bytes());
        assert_eq!(
            Page::parse(&bytes).expect_err("must reject"),
            PageError::PayloadTooLarge {
                len: MAX_PAYLOAD + 1,
                max: MAX_PAYLOAD
            }
        );
    }
}
