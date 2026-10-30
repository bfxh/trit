//! Belnap's four-valued logic (`FOUR`), the right algebra for *conflicting*
//! evidence.
//!
//! Three states are not always enough. When one detector says "this binary is
//! signed and trusted" and another says "its code section is packed", the honest
//! answer is neither `True` nor `False` nor plain `Unknown`: both claims have
//! support. Belnap's `FOUR` adds exactly that fourth state.
//!
//! # Representation, and what it is *not*
//!
//! A value is two independent bits: *is there support for true* and *is there
//! support for false*. Fusion is componentwise `or`, which is a `join` in the
//! **information order** — the order where more support means a bigger value.
//!
//! It is tempting to call these operations `and`/`or` and quote De Morgan. That
//! would be wrong, and this crate was written after a test caught it:
//! `¬(T ∧ F)` is `¬N = N`, while `¬T ∨ ¬F` is `F ∨ T = B`. They differ.
//!
//! `FOUR` is a **bilattice**: it carries two lattice orders. In the information
//! order used here, `N` is bottom and `B` is top, with `T` and `F` incomparable,
//! giving a distributive lattice isomorphic to `2²`. Negation swaps the rails,
//! which makes it an involution that **preserves** the information order and
//! reverses the truth order. It is not an anti-automorphism of this lattice, so
//! De Morgan does not hold and is not relied on anywhere.
//!
//! What the design actually needs from this type is weaker and fully satisfied:
//! fusion is commutative, associative and idempotent, and it only ever adds
//! support. That is what makes verdicts independent of detector order — and of
//! thread scheduling — and what stops two detectors reporting the same thing from
//! compounding into a stronger claim.

/// A Belnap four-valued truth value.
///
/// Encoded as `(support_for_true, support_for_false)` in bits 0 and 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct B4(u8);

impl B4 {
    /// Bit 0: something argues this is true.
    const POS: u8 = 0b01;
    /// Bit 1: something argues this is false.
    const NEG: u8 = 0b10;

    /// Neither side has support.
    pub const N: B4 = B4(0);
    /// Only "true" has support.
    pub const T: B4 = B4(Self::POS);
    /// Only "false" has support.
    pub const F: B4 = B4(Self::NEG);
    /// Both sides have support: a genuine conflict.
    pub const B: B4 = B4(Self::POS | Self::NEG);

    /// All four values.
    pub const ALL: [B4; 4] = [B4::N, B4::T, B4::F, B4::B];

    /// Builds from the two support rails directly.
    #[inline]
    #[must_use]
    pub const fn from_rails(for_true: bool, for_false: bool) -> Self {
        let mut bits = 0u8;
        if for_true {
            bits |= Self::POS;
        }
        if for_false {
            bits |= Self::NEG;
        }
        B4(bits)
    }

    /// Raw two-bit encoding, for ledger serialisation.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Rebuilds from [`B4::bits`].
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        B4(bits & 0b11)
    }

    /// Is there any support for "true"?
    #[inline]
    #[must_use]
    pub const fn supports_true(self) -> bool {
        self.0 & Self::POS != 0
    }

    /// Is there any support for "false"?
    #[inline]
    #[must_use]
    pub const fn supports_false(self) -> bool {
        self.0 & Self::NEG != 0
    }

    /// Both rails have support — the state three-valued logic cannot express.
    #[inline]
    #[must_use]
    pub const fn is_conflict(self) -> bool {
        self.0 == (Self::POS | Self::NEG)
    }

    /// Truth-like in Belnap's sense: `T` or `B`, i.e. it may be true.
    #[inline]
    #[must_use]
    pub const fn is_designated(self) -> bool {
        self.supports_true()
    }

    /// Lattice `join` in the information order: componentwise `or`.
    ///
    /// Adds the support of both arguments without letting either one cancel the
    /// other. Commutative, associative, idempotent.
    #[inline]
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        B4(self.0 | other.0)
    }

    /// Lattice `meet` in the information order: componentwise `and`.
    ///
    /// Keeps only the support both arguments share. Note this is *not* logical
    /// conjunction — logical conjunction does not have a truth-functional two-bit
    /// form matching this, see the module docs.
    #[inline]
    #[must_use]
    pub const fn meet(self, other: Self) -> Self {
        B4(self.0 & other.0)
    }

    /// Evidence accumulation. Alias for [`B4::join`], named for intent.
    ///
    /// This is the only combining operation the verdict layer uses. Because it is
    /// a join, accumulating evidence is monotone: a value can only move up the
    /// information order, so a "malicious" judgement can never silently become
    /// "clean" because one more detector fired.
    #[inline]
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        self.join(other)
    }

    /// Rail swap: `T ↔ F`, with `N` and `B` fixed.
    ///
    /// An involution. Preserves the information order; reverses the truth order.
    #[inline]
    #[must_use]
    pub const fn not(self) -> Self {
        B4(((self.0 & Self::POS) << 1) | ((self.0 & Self::NEG) >> 1))
    }

    /// Projects onto `K3`, collapsing conflict and silence alike.
    ///
    /// Destructive: prefer keeping [`B4`] wherever the conflict is actionable.
    #[inline]
    #[must_use]
    pub const fn to_tri(self) -> crate::Tri {
        match self.0 {
            Self::POS => crate::Tri::True,
            Self::NEG => crate::Tri::False,
            _ => crate::Tri::Unknown,
        }
    }

    /// Single-character rendering: `N` / `T` / `F` / `B`.
    #[inline]
    #[must_use]
    pub const fn glyph(self) -> char {
        match self.0 {
            0 => 'N',
            Self::POS => 'T',
            Self::NEG => 'F',
            _ => 'B',
        }
    }
}

impl From<crate::Tri> for B4 {
    #[inline]
    fn from(value: crate::Tri) -> Self {
        match value {
            crate::Tri::False => B4::F,
            crate::Tri::Unknown => B4::N,
            crate::Tri::True => B4::T,
        }
    }
}

impl core::fmt::Debug for B4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self.0 {
            0 => "N",
            Self::POS => "T",
            Self::NEG => "F",
            _ => "B",
        })
    }
}

impl core::fmt::Display for B4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self.0 {
            0 => "neither",
            Self::POS => "true",
            Self::NEG => "false",
            _ => "both",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::B4;
    use crate::Tri;

    /// The information order, stated without reference to the bit layout.
    fn leq(a: B4, b: B4) -> bool {
        (!a.supports_true() || b.supports_true()) && (!a.supports_false() || b.supports_false())
    }

    #[test]
    fn it_is_a_distributive_lattice() {
        for a in B4::ALL {
            assert_eq!(a.meet(a), a);
            assert_eq!(a.join(a), a);
            assert_eq!(a.not().not(), a);
            for b in B4::ALL {
                assert_eq!(a.meet(b), b.meet(a));
                assert_eq!(a.join(b), b.join(a));
                // Meet and join really are the bounds.
                assert!(leq(a.meet(b), a) && leq(a.meet(b), b));
                assert!(leq(a, a.join(b)) && leq(b, a.join(b)));
                // Absorption.
                assert_eq!(a.join(a.meet(b)), a);
                assert_eq!(a.meet(a.join(b)), a);
                for c in B4::ALL {
                    assert_eq!(a.meet(b.join(c)), a.meet(b).join(a.meet(c)));
                    assert_eq!(a.join(b.meet(c)), a.join(b).meet(a.join(c)));
                }
            }
        }
    }

    /// The algebra is a bilattice, not a De Morgan lattice. This test records the
    /// counterexample so nobody "fixes" it back into a false claim.
    #[test]
    fn de_morgan_does_not_hold_and_is_not_assumed() {
        assert_eq!(B4::T.meet(B4::F), B4::N);
        assert_eq!(B4::T.not().join(B4::F.not()), B4::B);
        // ¬(T ∧ F) = N but ¬T ∨ ¬F = B.
        assert_ne!(B4::T.meet(B4::F).not(), B4::T.not().join(B4::F.not()));
        // Negation preserves the information order instead.
        for a in B4::ALL {
            for b in B4::ALL {
                assert_eq!(leq(a, b), leq(a.not(), b.not()));
            }
        }
    }

    #[test]
    fn conflict_is_distinct_from_silence() {
        assert!(B4::B.is_conflict());
        assert!(!B4::N.is_conflict());
        assert!(!B4::T.is_conflict());
        // Both collapse to Unknown in K3, which is exactly why K3 alone is not
        // enough for evidence fusion.
        assert!(B4::B.to_tri().is_unknown());
        assert!(B4::N.to_tri().is_unknown());
        // But they differ on designation.
        assert!(B4::B.is_designated());
        assert!(!B4::N.is_designated());
    }

    #[test]
    fn merge_is_monotone_commutative_and_idempotent() {
        for a in B4::ALL {
            // Idempotence: a detector firing twice cannot strengthen a claim.
            assert_eq!(a.merge(a), a);
            for b in B4::ALL {
                let merged = a.merge(b);
                assert!(leq(a, merged), "{a:?} + {b:?} escaped upwards");
                assert!(leq(b, merged));
                assert_eq!(merged, b.merge(a), "fusion must not depend on order");
                for c in B4::ALL {
                    assert_eq!(a.merge(b).merge(c), a.merge(b.merge(c)), "associativity");
                }
            }
        }
    }

    #[test]
    fn designation_survives_fusion() {
        // Once anything argues "true", no amount of contrary evidence can remove
        // that — it becomes a conflict, which is the point.
        for a in B4::ALL {
            for b in B4::ALL {
                if a.is_designated() || b.is_designated() {
                    assert!(a.merge(b).is_designated());
                }
            }
        }
    }

    #[test]
    fn tri_roundtrip_through_decided_values() {
        assert_eq!(B4::from(Tri::True), B4::T);
        assert_eq!(B4::from(Tri::False), B4::F);
        assert_eq!(B4::from(Tri::Unknown), B4::N);
        assert_eq!(B4::T.to_tri(), Tri::True);
        assert_eq!(B4::F.to_tri(), Tri::False);
    }

    #[test]
    fn bits_roundtrip() {
        for a in B4::ALL {
            assert_eq!(B4::from_bits(a.bits()), a);
            assert_eq!(B4::from_bits(a.bits() | 0xFC), a, "高位必须被屏蔽");
        }
    }
}
