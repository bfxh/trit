//! Ternary byte patterns.
//!
//! # Why a wildcard needs a third state, not a second
//!
//! YARA-style patterns have exactly two states per position: *this byte* or
//! *anything*. That covers "I know" and "I don't care", but a reverse engineer
//! routinely means a third thing: **not this value**. A negative constraint is what
//! turns a fuzzy heuristic into a sharp one — "this is a .NET assembly header *and
//! not* a `NativeAOT` one", "the stub begins with a call *and not* the MSVC
//! prologue". Encoding that with two states forces you to enumerate alternatives,
//! which is error-prone and loses the intent.
//!
//! So each position is one of four predicates:
//!
//! | token | meaning |
//! |-------|---------|
//! | `41` | exactly `0x41` |
//! | `??` | anything |
//! | `!00` | anything **except** `0x00` |
//! | `[58,59,5A]` | one of the listed bytes |
//!
//! # The second, larger reason
//!
//! A pattern is evaluated against a [`Region`], which knows whether it is
//! truncated. When a predicate reads a byte that is not in the file, it does not
//! return "no"; it returns `Unknown`, and that propagates through the conjunction.
//! Concretely: a malware signature aimed at `.rsrc` evaluates to `False` when the
//! section exists with different content, and to `Unknown` when the section is
//! declared but lies past the end of the file. The first means "this is not it";
//! the second means "I could not check". No boolean matcher can tell those apart,
//! and a scanner that confuses them silently drops detections on truncated
//! downloads and on files whose headers were crafted to hide a section from
//! exactly that check.

use t3_core::{Tri, Tri::False};
use t3_format::Region;

/// Maximum pattern length. Long enough for any real signature, short enough that
/// verification stays cache-resident.
pub const MAX_PATTERN: usize = 256;

/// One position in a pattern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// Any byte, including one that is not present.
    Any,
    /// Exactly this byte.
    Byte(u8),
    /// Any byte except this one.
    Not(u8),
    /// One of a small explicit set.
    Set {
        /// The listed bytes.
        bytes: [u8; 6],
        /// How many are valid.
        len: u8,
    },
}

impl Slot {
    /// Evaluates the predicate against a possibly unavailable byte.
    ///
    /// `None` means the byte is not in the file; only [`Slot::Any`] is satisfiable
    /// without it.
    #[inline]
    #[must_use]
    pub fn eval(&self, byte: Option<u8>) -> Tri {
        match self {
            Slot::Any => Tri::True,
            Slot::Byte(want) => match byte {
                Some(b) if b == *want => Tri::True,
                Some(_) => False,
                None => Tri::Unknown,
            },
            Slot::Not(bad) => match byte {
                Some(b) if b == *bad => False,
                Some(_) => Tri::True,
                None => Tri::Unknown,
            },
            Slot::Set { bytes, len } => match byte {
                Some(b) => {
                    let hit = bytes[..usize::from(*len)].contains(&b);
                    Tri::from_bool(hit)
                }
                None => Tri::Unknown,
            },
        }
    }
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Pattern {
    slots: Vec<Slot>,
    /// Longest run of consecutive [`Slot::Byte`], used as a search anchor.
    anchor: Option<(usize, Vec<u8>)>,
    source: String,
}

/// Why a pattern could not be compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternError {
    /// The pattern had no tokens.
    Empty,
    /// The pattern is longer than [`MAX_PATTERN`].
    TooLong,
    /// A token was not valid hex or a known form.
    BadToken(String),
    /// A pattern made only of `??` matches everywhere and is therefore a bug.
    AllWildcard,
}

impl core::fmt::Display for PatternError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PatternError::Empty => f.write_str("pattern is empty"),
            PatternError::TooLong => write!(f, "pattern exceeds {MAX_PATTERN} slots"),
            PatternError::BadToken(t) => write!(f, "bad token `{t}`"),
            PatternError::AllWildcard => {
                f.write_str("pattern consists only of wildcards and would match everywhere")
            }
        }
    }
}

impl Pattern {
    /// Compiles a whitespace-separated hex pattern.
    ///
    /// # Errors
    /// See [`PatternError`].
    pub fn parse(source: &str) -> Result<Self, PatternError> {
        let mut slots = Vec::new();
        for token in source.split_whitespace() {
            if token.starts_with('#') {
                break;
            }
            slots.push(parse_slot(token)?);
            if slots.len() > MAX_PATTERN {
                return Err(PatternError::TooLong);
            }
        }
        if slots.is_empty() {
            return Err(PatternError::Empty);
        }
        if slots.iter().all(|s| matches!(s, Slot::Any)) {
            return Err(PatternError::AllWildcard);
        }

        let anchor = longest_byte_run(&slots);
        Ok(Pattern {
            slots,
            anchor,
            source: source.trim().to_string(),
        })
    }

    /// Number of positions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Always false; present for symmetry with `len`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The original text, for reports.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// True when the region could possibly contain this pattern, judged from a
    /// 256-bit presence bitmap of the bytes it holds.
    ///
    /// This is a sound *prefilter*: if a byte the pattern requires does not occur
    /// anywhere in the region, no offset can match, and the full scan can be skipped.
    /// It matters because the naive cost of a rule set is
    /// `rules × regions × region_length`, and on a 300 MiB image that is tens of
    /// gigabytes of scanning per file. One pass to build the bitmap replaces most of
    /// those passes.
    ///
    /// Callers must only use this on a region whose bytes are all present. On a
    /// truncated region an absent byte might simply be in the missing tail, so the
    /// filter would report a false negative — which is exactly the failure this crate
    /// exists to prevent.
    #[must_use]
    pub fn could_match(&self, presence: &[u64; 4]) -> bool {
        let has = |byte: u8| presence[usize::from(byte) / 64] & (1u64 << (byte % 64)) != 0;
        for slot in &self.slots {
            match slot {
                Slot::Any | Slot::Not(_) => {}
                Slot::Byte(b) => {
                    if !has(*b) {
                        return false;
                    }
                }
                Slot::Set { bytes, len } => {
                    if !bytes[..usize::from(*len)].iter().any(|b| has(*b)) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Evaluates the pattern at one start offset.
    #[must_use]
    pub fn eval_at(&self, region: &Region<'_>, start: usize) -> Tri {
        let mut acc = Tri::True;
        for (i, slot) in self.slots.iter().enumerate() {
            let value = slot.eval(region.byte(start + i));
            acc = acc.and(value);
            if acc.is_false() {
                return False;
            }
        }
        acc
    }

    /// Does the pattern occur anywhere in the region?
    ///
    /// Existence over offsets is a disjunction, so the result is:
    ///
    /// * `True` — some fully-present offset matched;
    /// * `False` — every feasible offset was evaluable and none matched;
    /// * `Unknown` — no match was found, but at least one feasible offset fell into
    ///   bytes the file does not contain.
    #[must_use]
    pub fn find(&self, region: &Region<'_>) -> Tri {
        let m = self.slots.len();
        let declared = region.declared_len;
        if m == 0 || declared < m {
            // A pattern longer than the declared region cannot occur in it. This is
            // a decidable false: the extent is asserted even if the bytes are absent.
            return False;
        }
        let last_start = declared - m;
        let avail = region.data.len();

        if avail >= m {
            // Phase A: starts whose whole window is present, so answers are definite.
            let definite_last = (avail - m).min(last_start);
            match self.scan_definite(region, definite_last) {
                Tri::True => return Tri::True,
                Tri::Unknown => return Tri::Unknown,
                False => {}
            }
            if definite_last == last_start {
                return False;
            }
            // Phase B: the remaining starts straddle the end of the data.
            return self.scan_straddling(region, definite_last + 1, last_start);
        }

        // Not one full window fits inside the present bytes: every feasible start
        // straddles, and the very first one already involves a missing byte.
        self.scan_straddling(region, 0, last_start)
    }

    /// Evaluates the starts that overlap missing bytes, bounding the work.
    fn scan_straddling(&self, region: &Region<'_>, first: usize, last: usize) -> Tri {
        let mut acc = False;
        for start in first..=last {
            acc = acc.or(self.eval_at(region, start));
            if acc.is_true() {
                return Tri::True;
            }
            // A heavily truncated region has an enormous missing window. Once a few
            // pattern-widths of starts have all come back undecided it is certain
            // the answer is undecided, so stop enumerating.
            if start.saturating_sub(first) > 4 * self.slots.len() {
                return Tri::Unknown;
            }
        }
        acc
    }

    /// Scans starts `0..=last` where every byte is present.
    fn scan_definite(&self, region: &Region<'_>, last: usize) -> Tri {
        match &self.anchor {
            // No run of two or more literal bytes: verify position by position,
            // seeding from the first required slot to skip obvious misses.
            None => {
                let seed = self.slots.iter().position(|s| *s != Slot::Any);
                let Some(seed) = seed else { return False };
                let want = if let Slot::Byte(b) = self.slots[seed] {
                    Some(b)
                } else {
                    None
                };
                let Some(b) = want else {
                    for start in 0..=last {
                        if self.eval_at(region, start).is_true() {
                            return Tri::True;
                        }
                    }
                    return False;
                };
                let mut from = 0usize;
                while from <= last {
                    let Some(hit) = t3_kernel::bytes::find_byte_from(region.data, b, from) else {
                        return False;
                    };
                    if hit < seed {
                        from = hit + 1;
                        continue;
                    }
                    let start = hit - seed;
                    if start > last {
                        return False;
                    }
                    let value = self.eval_at(region, start);
                    if value.is_true() {
                        return Tri::True;
                    }
                    from = hit + 1;
                }
                False
            }
            Some((offset, needle)) => {
                let first = needle[0];
                let mut from = 0usize;
                while from <= last {
                    let Some(hit) = t3_kernel::bytes::find_byte_from(region.data, first, from)
                    else {
                        return False;
                    };
                    let anchor_start = hit;
                    if anchor_start < *offset {
                        from = hit + 1;
                        continue;
                    }
                    let start = anchor_start - offset;
                    if start > last {
                        return False;
                    }
                    // Cheap in-place anchor verification before the full check.
                    let ok = needle
                        .iter()
                        .enumerate()
                        .all(|(i, want)| region.data.get(anchor_start + i) == Some(want));
                    if ok && self.eval_at(region, start).is_true() {
                        return Tri::True;
                    }
                    from = hit + 1;
                }
                False
            }
        }
    }
}

/// Finds the longest run of consecutive literal bytes.
fn longest_byte_run(slots: &[Slot]) -> Option<(usize, Vec<u8>)> {
    let mut best: Option<(usize, Vec<u8>)> = None;
    let mut i = 0usize;
    while i < slots.len() {
        if let Slot::Byte(_) = slots[i] {
            let start = i;
            let mut bytes = Vec::new();
            while let Some(Slot::Byte(b)) = slots.get(i) {
                bytes.push(*b);
                i += 1;
            }
            let better = match &best {
                None => bytes.len() >= 2,
                Some((_, prev)) => bytes.len() > prev.len(),
            };
            if better {
                best = Some((start, bytes));
            }
        } else {
            i += 1;
        }
    }
    best
}

fn parse_slot(token: &str) -> Result<Slot, PatternError> {
    let token = token.trim();
    if token == "??" || token == "?" || token == "**" {
        return Ok(Slot::Any);
    }
    if let Some(rest) = token.strip_prefix('!') {
        let b = parse_hex_byte(rest).ok_or_else(|| PatternError::BadToken(token.to_string()))?;
        return Ok(Slot::Not(b));
    }
    if token.starts_with('[') {
        if !token.ends_with(']') {
            return Err(PatternError::BadToken(token.to_string()));
        }
        let inner = &token[1..token.len() - 1];
        let mut bytes = [0u8; 6];
        let mut len = 0usize;
        for piece in inner.split(',') {
            if len == bytes.len() {
                return Err(PatternError::BadToken(token.to_string()));
            }
            let b = parse_hex_byte(piece.trim())
                .ok_or_else(|| PatternError::BadToken(token.to_string()))?;
            bytes[len] = b;
            len += 1;
        }
        if len == 0 {
            return Err(PatternError::BadToken(token.to_string()));
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "len 受 bytes 数组长度约束（≤ 6），转 u8 不会截断"
        )]
        let len = len as u8;
        return Ok(Slot::Set { bytes, len });
    }
    parse_hex_byte(token)
        .map(Slot::Byte)
        .ok_or_else(|| PatternError::BadToken(token.to_string()))
}

fn parse_hex_byte(token: &str) -> Option<u8> {
    let token = token.trim();
    if token.len() != 2 {
        return None;
    }
    u8::from_str_radix(token, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::{Pattern, PatternError, Slot};
    use t3_core::Tri;
    use t3_format::{Region, RegionKind};

    fn region(data: &[u8], declared: usize) -> Region<'_> {
        Region::new(RegionKind::Section(0), b".text", 0, data, declared)
    }

    #[test]
    fn parses_every_slot_form() {
        let p = Pattern::parse("41 ?? !00 [58,59,5A]").unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(p.slots[0], Slot::Byte(0x41));
        assert_eq!(p.slots[1], Slot::Any);
        assert_eq!(p.slots[2], Slot::Not(0));
        assert!(matches!(p.slots[3], Slot::Set { len: 3, .. }));
    }

    #[test]
    fn rejects_nonsense() {
        assert!(matches!(Pattern::parse(""), Err(PatternError::Empty)));
        assert!(matches!(
            Pattern::parse("ZZ"),
            Err(PatternError::BadToken(_))
        ));
        assert!(matches!(
            Pattern::parse("4"),
            Err(PatternError::BadToken(_))
        ));
        assert!(matches!(
            Pattern::parse("?? ??"),
            Err(PatternError::AllWildcard)
        ));
        assert!(matches!(
            Pattern::parse("[58,59,5A,5B,5C,5D,5E]"),
            Err(PatternError::BadToken(_))
        ));
    }

    #[test]
    fn slot_semantics() {
        assert_eq!(
            Slot::Any.eval(None),
            Tri::True,
            "a wildcard is satisfiable unseen"
        );
        assert_eq!(Slot::Byte(0x41).eval(None), Tri::Unknown);
        assert_eq!(Slot::Byte(0x41).eval(Some(0x42)), Tri::False);
        assert_eq!(Slot::Not(0).eval(None), Tri::Unknown);
        assert_eq!(Slot::Not(0).eval(Some(0)), Tri::False);
        assert_eq!(Slot::Not(0).eval(Some(1)), Tri::True);
        assert_eq!(
            Slot::Set {
                bytes: [1, 2, 3, 0, 0, 0],
                len: 3
            }
            .eval(Some(2)),
            Tri::True
        );
    }

    #[test]
    fn finds_a_match_anywhere() {
        let data = b"....\x41\x42\x43....";
        let p = Pattern::parse("41 42 43").unwrap();
        assert_eq!(p.find(&region(data, data.len())), Tri::True);
    }

    #[test]
    fn definite_absence_is_false() {
        let data = b"nothing here at all";
        let p = Pattern::parse("41 42 43").unwrap();
        assert_eq!(p.find(&region(data, data.len())), Tri::False);
    }

    /// The core claim: a pattern whose window falls into missing bytes is undecided,
    /// not a negative.
    #[test]
    fn truncated_region_yields_unknown_not_false() {
        let data = [0u8; 8];
        // Declared 64 bytes, only 8 present. Put the pattern at offset 6 so it
        // straddles the boundary.
        let r = region(&data, 64);
        let p = Pattern::parse("00 00 00 00 00 00").unwrap();
        // Data[0..8] are zeros, so offsets 0..=2 match entirely -> True.
        assert_eq!(p.find(&r), Tri::True);

        // Now a pattern that only matches at a straddling offset.
        let mut data2 = [0u8; 8];
        data2[7] = 0xAB;
        let r2 = region(&data2, 64);
        let p2 = Pattern::parse("AB CD EF").unwrap();
        // AB is present at 7 but CD/EF are not, and no other offset starts with AB.
        assert_eq!(p2.find(&r2), Tri::Unknown, "straddle must be unknown");

        // And with the region complete, the same bytes are a definite false.
        let r3 = region(&data2, 8);
        assert_eq!(p2.find(&r3), Tri::False);
    }

    #[test]
    fn pattern_longer_than_region_is_false_not_unknown() {
        let data = [0u8; 4];
        let r = region(&data, 4);
        let p = Pattern::parse("00 00 00 00 00").unwrap();
        assert_eq!(p.find(&r), Tri::False);
    }

    #[test]
    fn negative_slots_reject_matches() {
        // A pattern that requires a non-zero byte where the data has zeros.
        let data = [0u8; 32];
        let r = region(&data, 32);
        let p = Pattern::parse("!00").unwrap();
        assert_eq!(p.find(&r), Tri::False);
        let q = Pattern::parse("00 !00").unwrap();
        assert_eq!(q.find(&r), Tri::False);
    }

    #[test]
    fn anchor_verification_does_not_miss_late_matches() {
        // Exercise the anchor loop with many decoy first-byte hits.
        let mut data = vec![0u8; 4096];
        for i in (0..4096).step_by(7) {
            data[i] = 0x41;
        }
        data[4000] = 0x41;
        data[4001] = 0x42;
        data[4002] = 0x43;
        let p = Pattern::parse("41 42 43").unwrap();
        assert_eq!(p.find(&region(&data, data.len())), Tri::True);
    }

    #[test]
    fn all_zeros_in_a_huge_declared_region_is_unknown_when_pattern_absent() {
        // 8 bytes present, 1 MiB declared, pattern absent from the present part.
        let data = [0u8; 8];
        let r = region(&data, 1024 * 1024);
        let p = Pattern::parse("AB CD EF 01 02 03 04 05 06 07 08").unwrap();
        assert_eq!(p.find(&r), Tri::Unknown);
    }
}
