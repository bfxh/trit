#![forbid(unsafe_code)]
//! Radix packing for ternary values.
//!
//! A trit is represented as `0`, `1`, or `2`. The numeric order is
//! representation order; the semantics (K3 / Ł3 / B3) are defined by the
//! caller. This crate only packs and unpacks; it does not interpret.
//!
//! Supported radices:
//!
//! | radix | trits/byte | bit/trit | rule |
//! |---|---|---|---|
//! | [`Radix::R3`] | 5 | 1.6 | base-3; byte `0..=242`; `243..=255` invalid |
//! | [`Radix::R4`] | 4 | 2.0 | 2 bits per trit; value `3` invalid |
//! | [`Radix::R16`] | 2 | 4.0 | 4 bits per trit; value `> 2` invalid |
//!
//! All encoders are **canonical**: unused slots in the last byte are zero.
//! Decoders reject non-zero padding, so one sequence has exactly one encoding.
//!
//! `R3` uses a 10-bit pack table (5 trits, 2 bits each) and a 243-entry
//! unpack table; both tables are generated at compile time.

use std::fmt;

/// A ternary value in representation order: `T0 = 0`, `T1 = 1`, `T2 = 2`.
///
/// The names are deliberately neutral. Whether `T2` means "true", "unknown",
/// or "both" is a semantic decision made by the caller, not by this crate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Trit(u8);

impl Trit {
    /// Representation value `0`.
    pub const T0: Self = Trit(0);
    /// Representation value `1`.
    pub const T1: Self = Trit(1);
    /// Representation value `2`.
    pub const T2: Self = Trit(2);

    /// Builds a trit from `0`, `1`, or `2`; returns `None` otherwise.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if value < 3 {
            Some(Trit(value))
        } else {
            None
        }
    }

    /// Returns the representation value (`0`, `1`, or `2`).
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Trit {
    type Error = RadixError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::from_u8(value).ok_or(RadixError::InvalidTrit(value))
    }
}

impl fmt::Debug for Trit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "T{}", self.0)
    }
}

/// Packing radix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Radix {
    /// 5 trits per byte, base-3 packing, 1.6 bit/trit.
    R3,
    /// 4 trits per byte, 2 bits per trit, 2.0 bit/trit.
    R4,
    /// 2 trits per byte, 4 bits per trit, 4.0 bit/trit.
    R16,
}

impl Radix {
    /// All supported radices, in preference order for a first pass.
    pub const ALL: [Radix; 3] = [Radix::R3, Radix::R4, Radix::R16];

    /// Stable short name (`r3`, `r4`, `r16`).
    pub const fn name(self) -> &'static str {
        match self {
            Radix::R3 => "r3",
            Radix::R4 => "r4",
            Radix::R16 => "r16",
        }
    }

    /// Parses `r3`/`3`, `r4`/`4`, `r16`/`16` (case-insensitive for `r`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "r3" | "3" => Some(Radix::R3),
            "r4" | "4" => Some(Radix::R4),
            "r16" | "16" => Some(Radix::R16),
            _ => None,
        }
    }

    /// Number of whole trits stored in one byte.
    pub const fn trits_per_byte(self) -> usize {
        match self {
            Radix::R3 => 5,
            Radix::R4 => 4,
            Radix::R16 => 2,
        }
    }

    /// Numeric code stored in a TRBL block header (`3`, `4`, or `16`).
    pub const fn code(self) -> u8 {
        match self {
            Radix::R3 => 3,
            Radix::R4 => 4,
            Radix::R16 => 16,
        }
    }

    /// Parses a numeric code from a TRBL block header.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            3 => Some(Radix::R3),
            4 => Some(Radix::R4),
            16 => Some(Radix::R16),
            _ => None,
        }
    }

    /// Validates packed length and canonical padding without allocating.
    pub fn validate_packed(self, bytes: &[u8], trit_count: usize) -> Result<(), RadixError> {
        let expected = self.packed_len(trit_count);
        if bytes.len() != expected {
            return Err(RadixError::LengthMismatch {
                expected,
                actual: bytes.len(),
            });
        }

        match self {
            Radix::R3 => validate_r3(bytes, trit_count),
            Radix::R4 => validate_r4(bytes, trit_count),
            Radix::R16 => validate_r16(bytes, trit_count),
        }
    }

    /// Effective bits per trit (including padding).
    pub fn bits_per_trit(self) -> f64 {
        8.0 / self.trits_per_byte() as f64
    }

    /// Number of bytes needed for `trit_count` trits.
    pub fn packed_len(self, trit_count: usize) -> usize {
        match self {
            Radix::R3 => trit_count.div_ceil(5),
            Radix::R4 => trit_count.div_ceil(4),
            Radix::R16 => trit_count.div_ceil(2),
        }
    }
}

/// Structured errors returned by [`pack`] / [`unpack`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadixError {
    /// A value outside `0..=2` was passed as a trit.
    InvalidTrit(u8),
    /// A packed byte contains a value outside `0..=2`.
    InvalidPackedByte {
        /// Radix that was being decoded.
        radix: Radix,
        /// The offending byte.
        byte: u8,
    },
    /// `bytes.len()` does not equal `radix.packed_len(trit_count)`.
    LengthMismatch {
        /// Expected byte count.
        expected: usize,
        /// Actual byte count.
        actual: usize,
    },
    /// The last byte has non-zero padding, so the encoding is not canonical.
    NonCanonicalPadding {
        /// Radix that was being decoded.
        radix: Radix,
        /// The offending byte.
        byte: u8,
    },
}

impl fmt::Display for RadixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RadixError::InvalidTrit(value) => {
                write!(f, "invalid trit {value}: expected 0, 1, or 2")
            }
            RadixError::InvalidPackedByte { radix, byte } => write!(
                f,
                "{} packed byte {byte:#04x} contains a value outside 0..=2",
                radix.name()
            ),
            RadixError::LengthMismatch { expected, actual } => {
                write!(
                    f,
                    "packed length mismatch: expected {expected}, got {actual}"
                )
            }
            RadixError::NonCanonicalPadding { radix, byte } => write!(
                f,
                "{} last byte {byte:#04x} has non-zero padding",
                radix.name()
            ),
        }
    }
}

impl std::error::Error for RadixError {}

/// Maps a 10-bit key (`t0 | t1<<2 | t2<<4 | t3<<6 | t4<<8`) to a base-3 byte.
/// Only the 3^5 = 243 keys with all fields `< 3` are meaningful; the rest are
/// left at zero and are unreachable because [`Trit`] only holds `0..=2`.
const R3_PACK_TABLE: [u8; 1024] = build_r3_pack_table();

/// Maps a base-3 byte (`0..=242`) to its five trits.
const R3_UNPACK_TABLE: [[Trit; 5]; 243] = build_r3_unpack_table();

const fn build_r3_pack_table() -> [u8; 1024] {
    let mut table = [0u8; 1024];
    let mut key = 0usize;
    while key < 1024 {
        let t0 = (key & 0b11) as u8;
        let t1 = ((key >> 2) & 0b11) as u8;
        let t2 = ((key >> 4) & 0b11) as u8;
        let t3 = ((key >> 6) & 0b11) as u8;
        let t4 = ((key >> 8) & 0b11) as u8;
        if t0 < 3 && t1 < 3 && t2 < 3 && t3 < 3 && t4 < 3 {
            table[key] = t0 + 3 * t1 + 9 * t2 + 27 * t3 + 81 * t4;
        }
        key += 1;
    }
    table
}

const fn build_r3_unpack_table() -> [[Trit; 5]; 243] {
    let mut table = [[Trit::T0; 5]; 243];
    let mut byte = 0usize;
    while byte < 243 {
        let mut value = byte;
        let mut slot = 0usize;
        while slot < 5 {
            table[byte][slot] = Trit((value % 3) as u8);
            value /= 3;
            slot += 1;
        }
        byte += 1;
    }
    table
}

#[inline]
fn r3_key(chunk: &[Trit]) -> usize {
    debug_assert_eq!(chunk.len(), 5);
    (chunk[0].get() as usize)
        | (chunk[1].get() as usize) << 2
        | (chunk[2].get() as usize) << 4
        | (chunk[3].get() as usize) << 6
        | (chunk[4].get() as usize) << 8
}

/// Packs `trits` with `radix`; returns a fresh byte vector.
pub fn pack(radix: Radix, trits: &[Trit]) -> Result<Vec<u8>, RadixError> {
    let mut out = Vec::with_capacity(radix.packed_len(trits.len()));
    pack_into(radix, trits, &mut out)?;
    Ok(out)
}

/// Packs `trits` into an existing buffer, clearing it first.
#[allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]
pub fn pack_into(radix: Radix, trits: &[Trit], out: &mut Vec<u8>) -> Result<(), RadixError> {
    out.clear();
    out.reserve(radix.packed_len(trits.len()));

    match radix {
        Radix::R3 => {
            let mut chunks = trits.chunks_exact(5);
            for chunk in &mut chunks {
                out.push(R3_PACK_TABLE[r3_key(chunk)]);
            }
            let rem = chunks.remainder();
            if !rem.is_empty() {
                let mut value = 0u8;
                let mut place = 1u8;
                for trit in rem {
                    value += trit.get() * place;
                    place *= 3;
                }
                out.push(value);
            }
        }
        Radix::R4 => {
            for chunk in trits.chunks(4) {
                let mut byte = 0u8;
                for (index, trit) in chunk.iter().enumerate() {
                    byte |= trit.get() << (2 * index);
                }
                out.push(byte);
            }
        }
        Radix::R16 => {
            for chunk in trits.chunks(2) {
                let mut byte = 0u8;
                for (index, trit) in chunk.iter().enumerate() {
                    byte |= trit.get() << (4 * index);
                }
                out.push(byte);
            }
        }
    }

    Ok(())
}

/// Unpacks `bytes` into exactly `trit_count` trits.
pub fn unpack(radix: Radix, bytes: &[u8], trit_count: usize) -> Result<Vec<Trit>, RadixError> {
    let mut out = Vec::with_capacity(trit_count);
    unpack_into(radix, bytes, trit_count, &mut out)?;
    Ok(out)
}

/// Unpacks `bytes` into an existing buffer, clearing it first.
pub fn unpack_into(
    radix: Radix,
    bytes: &[u8],
    trit_count: usize,
    out: &mut Vec<Trit>,
) -> Result<(), RadixError> {
    let expected = radix.packed_len(trit_count);
    if bytes.len() != expected {
        return Err(RadixError::LengthMismatch {
            expected,
            actual: bytes.len(),
        });
    }

    out.clear();
    out.reserve(trit_count);

    match radix {
        Radix::R3 => unpack_r3(bytes, trit_count, out)?,
        Radix::R4 => unpack_r4(bytes, trit_count, out)?,
        Radix::R16 => unpack_r16(bytes, trit_count, out)?,
    }

    debug_assert_eq!(out.len(), trit_count);
    Ok(())
}

fn unpack_r3(bytes: &[u8], trit_count: usize, out: &mut Vec<Trit>) -> Result<(), RadixError> {
    let full = trit_count / 5;
    let rem = trit_count % 5;

    for &byte in &bytes[..full] {
        if byte >= 243 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R3,
                byte,
            });
        }
        out.extend_from_slice(&R3_UNPACK_TABLE[byte as usize]);
    }

    if rem > 0 {
        let byte = bytes[full];
        if byte >= 243 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R3,
                byte,
            });
        }
        let values = &R3_UNPACK_TABLE[byte as usize];
        out.extend_from_slice(&values[..rem]);
        if values[rem..].iter().any(|trit| trit.get() != 0) {
            return Err(RadixError::NonCanonicalPadding {
                radix: Radix::R3,
                byte,
            });
        }
    }

    Ok(())
}

fn unpack_r4(bytes: &[u8], trit_count: usize, out: &mut Vec<Trit>) -> Result<(), RadixError> {
    let full = trit_count / 4;
    let rem = trit_count % 4;

    for &byte in &bytes[..full] {
        for index in 0..4 {
            let value = (byte >> (2 * index)) & 0b11;
            if value > 2 {
                return Err(RadixError::InvalidPackedByte {
                    radix: Radix::R4,
                    byte,
                });
            }
            out.push(Trit(value));
        }
    }

    if rem > 0 {
        let byte = bytes[full];
        for index in 0..rem {
            let value = (byte >> (2 * index)) & 0b11;
            if value > 2 {
                return Err(RadixError::InvalidPackedByte {
                    radix: Radix::R4,
                    byte,
                });
            }
            out.push(Trit(value));
        }
        let used_bits = 2 * rem;
        let padding_mask = !((1u8 << used_bits) - 1);
        if byte & padding_mask != 0 {
            return Err(RadixError::NonCanonicalPadding {
                radix: Radix::R4,
                byte,
            });
        }
    }

    Ok(())
}

fn unpack_r16(bytes: &[u8], trit_count: usize, out: &mut Vec<Trit>) -> Result<(), RadixError> {
    let full = trit_count / 2;
    let rem = trit_count % 2;

    for &byte in &bytes[..full] {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low > 2 || high > 2 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R16,
                byte,
            });
        }
        out.push(Trit(low));
        out.push(Trit(high));
    }

    if rem == 1 {
        let byte = bytes[full];
        let low = byte & 0x0f;
        if low > 2 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R16,
                byte,
            });
        }
        if byte >> 4 != 0 {
            return Err(RadixError::NonCanonicalPadding {
                radix: Radix::R16,
                byte,
            });
        }
        out.push(Trit(low));
    }

    Ok(())
}

fn validate_r3(bytes: &[u8], trit_count: usize) -> Result<(), RadixError> {
    let full = trit_count / 5;
    let rem = trit_count % 5;

    for &byte in &bytes[..full] {
        if byte >= 243 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R3,
                byte,
            });
        }
    }

    if rem > 0 {
        let byte = bytes[full];
        if byte >= 243 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R3,
                byte,
            });
        }
        if R3_UNPACK_TABLE[byte as usize][rem..]
            .iter()
            .any(|trit| trit.get() != 0)
        {
            return Err(RadixError::NonCanonicalPadding {
                radix: Radix::R3,
                byte,
            });
        }
    }

    Ok(())
}

fn validate_r4(bytes: &[u8], trit_count: usize) -> Result<(), RadixError> {
    let full = trit_count / 4;
    let rem = trit_count % 4;

    for &byte in &bytes[..full] {
        for index in 0..4 {
            if ((byte >> (2 * index)) & 0b11) > 2 {
                return Err(RadixError::InvalidPackedByte {
                    radix: Radix::R4,
                    byte,
                });
            }
        }
    }

    if rem > 0 {
        let byte = bytes[full];
        for index in 0..rem {
            if ((byte >> (2 * index)) & 0b11) > 2 {
                return Err(RadixError::InvalidPackedByte {
                    radix: Radix::R4,
                    byte,
                });
            }
        }
        let used_bits = 2 * rem;
        let padding_mask = !((1u8 << used_bits) - 1);
        if byte & padding_mask != 0 {
            return Err(RadixError::NonCanonicalPadding {
                radix: Radix::R4,
                byte,
            });
        }
    }

    Ok(())
}

fn validate_r16(bytes: &[u8], trit_count: usize) -> Result<(), RadixError> {
    let full = trit_count / 2;
    let rem = trit_count % 2;

    for &byte in &bytes[..full] {
        if (byte & 0x0f) > 2 || (byte >> 4) > 2 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R16,
                byte,
            });
        }
    }

    if rem == 1 {
        let byte = bytes[full];
        if (byte & 0x0f) > 2 {
            return Err(RadixError::InvalidPackedByte {
                radix: Radix::R16,
                byte,
            });
        }
        if byte >> 4 != 0 {
            return Err(RadixError::NonCanonicalPadding {
                radix: Radix::R16,
                byte,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trits(values: &[u8]) -> Vec<Trit> {
        values
            .iter()
            .copied()
            .map(|value| Trit::from_u8(value).expect("valid trit"))
            .collect()
    }

    #[test]
    fn radix_codes_roundtrip() {
        for radix in Radix::ALL {
            assert_eq!(Radix::from_code(radix.code()), Some(radix));
        }
        assert_eq!(Radix::from_code(0), None);
        assert_eq!(Radix::from_code(5), None);
    }

    #[test]
    fn validate_packed_rejects_non_canonical_padding() {
        assert!(matches!(
            Radix::R3.validate_packed(&[5], 1),
            Err(RadixError::NonCanonicalPadding { .. })
        ));
        assert!(matches!(
            Radix::R4.validate_packed(&[0b0000_1000], 1),
            Err(RadixError::NonCanonicalPadding { .. })
        ));
        assert!(matches!(
            Radix::R16.validate_packed(&[0x12], 1),
            Err(RadixError::NonCanonicalPadding { .. })
        ));
    }

    #[test]
    fn r3_pack_table_covers_all_valid_keys() {
        let mut encoded = Vec::with_capacity(243);
        for (key, &encoded_byte) in R3_PACK_TABLE.iter().enumerate() {
            let t0 = (key & 0b11) as u8;
            let t1 = ((key >> 2) & 0b11) as u8;
            let t2 = ((key >> 4) & 0b11) as u8;
            let t3 = ((key >> 6) & 0b11) as u8;
            let t4 = ((key >> 8) & 0b11) as u8;
            if t0 > 2 || t1 > 2 || t2 > 2 || t3 > 2 || t4 > 2 {
                continue;
            }
            let values = [
                Trit::from_u8(t0).expect("valid"),
                Trit::from_u8(t1).expect("valid"),
                Trit::from_u8(t2).expect("valid"),
                Trit::from_u8(t3).expect("valid"),
                Trit::from_u8(t4).expect("valid"),
            ];
            let packed = pack(Radix::R3, &values).expect("pack");
            assert_eq!(packed, vec![encoded_byte]);
            assert_eq!(unpack(Radix::R3, &packed, 5).expect("unpack"), values);
            encoded.push(encoded_byte);
        }

        assert_eq!(encoded.len(), 243);
        encoded.sort_unstable();
        assert_eq!(encoded, (0u8..243).collect::<Vec<u8>>());
    }

    #[test]
    fn known_r3_vector() {
        let input = trits(&[0, 1, 2, 0, 1]);
        let packed = pack(Radix::R3, &input).expect("pack");
        assert_eq!(packed, vec![102]);
        let decoded = unpack(Radix::R3, &packed, input.len()).expect("unpack");
        assert_eq!(decoded, input);
    }

    #[test]
    fn known_r4_vector() {
        let input = trits(&[0, 1, 2, 0]);
        let packed = pack(Radix::R4, &input).expect("pack");
        assert_eq!(packed, vec![36]);
        let decoded = unpack(Radix::R4, &packed, input.len()).expect("unpack");
        assert_eq!(decoded, input);
    }

    #[test]
    fn known_r16_vector() {
        let input = trits(&[2, 1]);
        let packed = pack(Radix::R16, &input).expect("pack");
        assert_eq!(packed, vec![0x12]);
        let decoded = unpack(Radix::R16, &packed, input.len()).expect("unpack");
        assert_eq!(decoded, input);
    }

    #[test]
    fn rejects_invalid_trit() {
        assert_eq!(Trit::from_u8(3), None);
        assert_eq!(
            Trit::try_from(9).expect_err("must reject"),
            RadixError::InvalidTrit(9)
        );
    }

    #[test]
    fn rejects_r3_invalid_byte() {
        assert_eq!(
            unpack(Radix::R3, &[243], 5).expect_err("must reject"),
            RadixError::InvalidPackedByte {
                radix: Radix::R3,
                byte: 243
            }
        );
    }

    #[test]
    fn rejects_r4_invalid_slot() {
        assert_eq!(
            unpack(Radix::R4, &[0b0000_0011], 4).expect_err("must reject"),
            RadixError::InvalidPackedByte {
                radix: Radix::R4,
                byte: 0b0000_0011
            }
        );
    }

    #[test]
    fn rejects_r16_invalid_nibble() {
        assert_eq!(
            unpack(Radix::R16, &[0x13], 2).expect_err("must reject"),
            RadixError::InvalidPackedByte {
                radix: Radix::R16,
                byte: 0x13
            }
        );
    }

    #[test]
    fn rejects_r3_non_canonical_padding() {
        // n=1: canonical byte is 2 (T2). 5 = 2 + 3*1 has a non-zero unused slot.
        assert_eq!(
            unpack(Radix::R3, &[5], 1).expect_err("must reject"),
            RadixError::NonCanonicalPadding {
                radix: Radix::R3,
                byte: 5
            }
        );
    }

    #[test]
    fn rejects_r4_non_canonical_padding() {
        // n=1: canonical byte is 2. 0b0000_1000 has a non-zero unused slot.
        assert_eq!(
            unpack(Radix::R4, &[0b0000_1000], 1).expect_err("must reject"),
            RadixError::NonCanonicalPadding {
                radix: Radix::R4,
                byte: 0b0000_1000
            }
        );
    }

    #[test]
    fn rejects_r16_non_canonical_padding() {
        // n=1: canonical byte is 2. 0x12 has a non-zero high nibble.
        assert_eq!(
            unpack(Radix::R16, &[0x12], 1).expect_err("must reject"),
            RadixError::NonCanonicalPadding {
                radix: Radix::R16,
                byte: 0x12
            }
        );
    }

    #[test]
    fn rejects_length_mismatch() {
        // n=11 needs 3 bytes for R3; only 2 supplied.
        assert_eq!(
            unpack(Radix::R3, &[1, 2], 11).expect_err("must reject"),
            RadixError::LengthMismatch {
                expected: 3,
                actual: 2
            }
        );
        // n=6 needs 2 bytes for R3; only 1 supplied.
        assert_eq!(
            unpack(Radix::R3, &[1], 6).expect_err("must reject"),
            RadixError::LengthMismatch {
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn empty_roundtrip() {
        for radix in Radix::ALL {
            let packed = pack(radix, &[]).expect("pack");
            assert!(packed.is_empty());
            let decoded = unpack(radix, &[], 0).expect("unpack");
            assert!(decoded.is_empty());
        }
    }
}
