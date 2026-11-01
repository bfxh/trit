//! Bounds-checked little- and big-endian reads over an untrusted buffer.
//!
//! Every accessor returns `Option`, and every arithmetic operation goes through
//! `checked_*`. That is not defensive style for its own sake: the input is a file
//! an adversary controls, and the only two acceptable behaviours on malformed input
//! are "return `None`" and "return a correct answer". Panicking, wrapping around,
//! or reading past the end are all bugs with security consequences in a tool that
//! is pointed at malware.
//!
//! The invariant that makes this work is simple: **all offsets are validated
//! against `len()` before any slice is formed**, and slicing is only ever done with
//! `get(a..b)` or after `end <= len` has been checked. There is no `unsafe` here at
//! all, so the compiler enforces it.

/// Byte order used by a parsed container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    /// Least significant byte first (x86, ARM in practice).
    Little,
    /// Most significant byte first (some MIPS, PowerPC, SPARC).
    Big,
}

/// A cursor over a byte slice with checked reads.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    endian: Endian,
}

impl<'a> Reader<'a> {
    /// Wraps a buffer with a byte order.
    #[inline]
    #[must_use]
    pub fn new(data: &'a [u8], endian: Endian) -> Self {
        Reader { data, endian }
    }

    /// Wraps a buffer as little-endian, the PE convention.
    #[inline]
    #[must_use]
    pub fn le(data: &'a [u8]) -> Self {
        Reader::new(data, Endian::Little)
    }

    /// Underlying buffer.
    #[inline]
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Buffer length.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True when the buffer is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Byte order.
    #[inline]
    #[must_use]
    pub fn endian(&self) -> Endian {
        self.endian
    }

    /// Sub-buffer `[offset, offset+len)`, or `None` when out of range.
    ///
    /// `len == 0` is allowed at `offset == self.len()` so that zero-length regions
    /// (a section with no raw data) are representable without special-casing.
    #[inline]
    #[must_use]
    pub fn slice(&self, offset: usize, len: usize) -> Option<&'a [u8]> {
        let end = offset.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        self.data.get(offset..end)
    }

    /// Everything from `offset` onwards.
    #[inline]
    #[must_use]
    pub fn tail(&self, offset: usize) -> Option<&'a [u8]> {
        self.data.get(offset..)
    }

    /// Single byte.
    #[inline]
    #[must_use]
    pub fn u8(&self, offset: usize) -> Option<u8> {
        self.data.get(offset).copied()
    }

    /// Unsigned 16-bit integer at `offset`.
    #[inline]
    #[must_use]
    pub fn u16(&self, offset: usize) -> Option<u16> {
        let b = self.slice(offset, 2)?;
        Some(match self.endian {
            Endian::Little => u16::from_le_bytes([b[0], b[1]]),
            Endian::Big => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    /// Unsigned 32-bit integer at `offset`.
    #[inline]
    #[must_use]
    pub fn u32(&self, offset: usize) -> Option<u32> {
        let b = self.slice(offset, 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(match self.endian {
            Endian::Little => u32::from_le_bytes(a),
            Endian::Big => u32::from_be_bytes(a),
        })
    }

    /// Unsigned 64-bit integer at `offset`.
    #[inline]
    #[must_use]
    pub fn u64(&self, offset: usize) -> Option<u64> {
        let b = self.slice(offset, 8)?;
        let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        Some(match self.endian {
            Endian::Little => u64::from_le_bytes(a),
            Endian::Big => u64::from_be_bytes(a),
        })
    }

    /// Pointer-sized unsigned integer (4 or 8 bytes).
    #[inline]
    #[must_use]
    pub fn uint(&self, offset: usize, wide: bool) -> Option<u64> {
        if wide {
            self.u64(offset)
        } else {
            self.u32(offset).map(u64::from)
        }
    }

    /// NUL-terminated byte string starting at `offset`, at most `max` bytes.
    ///
    /// Returns the bytes *before* the terminator. When no terminator is found
    /// within `max`, returns the truncated prefix along with `false` so callers can
    /// distinguish "complete string" from "ran off the end" — a distinction that
    /// matters when deciding whether an import table was fully read.
    #[must_use]
    pub fn cstr(&self, offset: usize, max: usize) -> Option<(&'a [u8], bool)> {
        let end = offset.checked_add(max)?.min(self.data.len());
        if offset >= self.data.len() {
            return None;
        }
        let window = self.data.get(offset..end)?;
        match window.iter().position(|&b| b == 0) {
            Some(n) => Some((&window[..n], true)),
            None => Some((window, false)),
        }
    }

    /// Compares a fixed-length tag at `offset`.
    #[must_use]
    pub fn tag_is(&self, offset: usize, expected: &[u8]) -> bool {
        match self.slice(offset, expected.len()) {
            Some(got) => got == expected,
            None => false,
        }
    }
}

/// Adds `a + b`, or `None` on overflow.
///
/// Used everywhere offsets are combined, so that a malicious 32-bit size cannot
/// wrap into a small offset.
#[inline]
#[must_use]
pub fn add(a: usize, b: usize) -> Option<usize> {
    a.checked_add(b)
}

#[cfg(test)]
mod tests {
    use super::{add, Endian, Reader};

    #[test]
    fn reads_respect_byte_order() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let le = Reader::new(&data, Endian::Little);
        let be = Reader::new(&data, Endian::Big);
        assert_eq!(le.u16(0), Some(0x0201));
        assert_eq!(be.u16(0), Some(0x0102));
        assert_eq!(le.u32(0), Some(0x0403_0201));
        assert_eq!(be.u32(0), Some(0x0102_0304));
        assert_eq!(le.u64(0), Some(0x0807_0605_0403_0201));
    }

    #[test]
    fn out_of_range_is_none_not_panic() {
        let data = [1u8, 2, 3];
        let r = Reader::le(&data);
        assert_eq!(r.u8(2), Some(3));
        assert_eq!(r.u8(3), None);
        assert_eq!(r.u16(2), None);
        assert_eq!(r.u32(0), None);
        assert_eq!(r.u64(0), None);
        assert_eq!(r.slice(0, 4), None);
        assert_eq!(r.slice(4, 0), None, "offset past end even for zero length");
        assert_eq!(r.slice(3, 0), Some(&[][..]), "zero length at end is valid");
    }

    #[test]
    fn huge_lengths_do_not_wrap() {
        let data = [0u8; 8];
        let r = Reader::le(&data);
        assert_eq!(r.slice(1, usize::MAX), None);
        assert_eq!(r.slice(usize::MAX, 1), None);
        assert_eq!(add(usize::MAX, 1), None);
    }

    #[test]
    fn cstr_reports_truncation() {
        let data = b"abc\0def";
        let r = Reader::le(data);
        assert_eq!(r.cstr(0, 8), Some((&b"abc"[..], true)));
        // No terminator inside the window.
        assert_eq!(r.cstr(0, 3), Some((&b"abc"[..], false)));
        assert_eq!(r.cstr(0, 2), Some((&b"ab"[..], false)));
        assert_eq!(r.cstr(4, 8), Some((&b"def"[..], false)));
        assert_eq!(r.cstr(8, 8), None);
    }

    #[test]
    fn tag_matching() {
        let r = Reader::le(b"MZ\x90\x00");
        assert!(r.tag_is(0, b"MZ"));
        assert!(!r.tag_is(0, b"ZM"));
        assert!(!r.tag_is(3, b"MZ\x00\x00"));
        // An empty tag matches only where a zero-length slice exists: at or before the
        // end. Past the end it does not, because the offset itself is out of range.
        assert!(r.tag_is(0, b""));
        assert!(r.tag_is(4, b""));
        assert!(!r.tag_is(5, b""));
    }

    /// A fuzz-shaped smoke test: random buffers must never panic.
    #[test]
    fn random_buffers_never_panic() {
        let mut s = 0xDEAD_BEEFu64;
        for len in 0..200usize {
            let mut data = vec![0u8; len];
            for b in &mut data {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *b = s as u8;
            }
            let r = Reader::le(&data);
            for off in 0..len + 2 {
                let _ = r.u8(off);
                let _ = r.u16(off);
                let _ = r.u32(off);
                let _ = r.u64(off);
                let _ = r.cstr(off, 16);
                let _ = r.tail(off);
            }
        }
    }
}
