//! Strong Kleene three-valued logic (`K3`).
//!
//! The value set is `{False, Unknown, True}` ordered by *truth*
//! (`False < Unknown < True`), which makes `and` a minimum and `or` a maximum.
//! That ordering is what gives the logic its defining property: any monotone
//! formula is monotone in each variable, so an input left at `Unknown` can only
//! pull the result towards `Unknown`, never past it.
//!
//! This module deliberately exposes no "coerce unknown to false" helper. Such a
//! coercion is the single most common way tri-state systems degrade back to
//! two-state ones, and every call site that genuinely wants it has to write
//! [`Tri::is_true`] and accept the explicit narrowing.

/// A strong-Kleene truth value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum Tri {
    /// Definitively false.
    False = 0,
    /// Not enough evidence to decide.
    #[default]
    Unknown = 1,
    /// Definitively true.
    True = 2,
}

impl Tri {
    /// Number of variants; useful for counting tables.
    pub const COUNT: usize = 3;

    /// All three values in truth order.
    pub const ALL: [Tri; 3] = [Tri::False, Tri::Unknown, Tri::True];

    /// Maps a boolean to a decided truth value.
    #[inline]
    #[must_use]
    pub const fn from_bool(value: bool) -> Self {
        if value {
            Tri::True
        } else {
            Tri::False
        }
    }

    /// Lifts to `Option<bool>`: `Unknown` becomes `None`.
    ///
    /// Prefer this over `is_true()` when the caller must *handle* the undecided
    /// case, because the type forces the branch to exist.
    #[inline]
    #[must_use]
    pub const fn decided(self) -> Option<bool> {
        match self {
            Tri::False => Some(false),
            Tri::True => Some(true),
            Tri::Unknown => None,
        }
    }

    /// True only for [`Tri::True`]. `Unknown` is not true.
    #[inline]
    #[must_use]
    pub const fn is_true(self) -> bool {
        matches!(self, Tri::True)
    }

    /// True only for [`Tri::False`]. `Unknown` is not false.
    #[inline]
    #[must_use]
    pub const fn is_false(self) -> bool {
        matches!(self, Tri::False)
    }

    /// True only for [`Tri::Unknown`].
    #[inline]
    #[must_use]
    pub const fn is_unknown(self) -> bool {
        matches!(self, Tri::Unknown)
    }

    /// Kleene negation. `Unknown` is a fixed point.
    #[inline]
    #[must_use]
    pub const fn not(self) -> Self {
        match self {
            Tri::False => Tri::True,
            Tri::True => Tri::False,
            Tri::Unknown => Tri::Unknown,
        }
    }

    /// Kleene conjunction: minimum in truth order.
    #[inline]
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Tri::False, _) | (_, Tri::False) => Tri::False,
            (Tri::True, Tri::True) => Tri::True,
            _ => Tri::Unknown,
        }
    }

    /// Kleene disjunction: maximum in truth order.
    #[inline]
    #[must_use]
    pub const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Tri::True, _) | (_, Tri::True) => Tri::True,
            (Tri::False, Tri::False) => Tri::False,
            _ => Tri::Unknown,
        }
    }

    /// Kleene material implication `(!self) | other`.
    #[inline]
    #[must_use]
    pub const fn implies(self, other: Self) -> Self {
        self.not().or(other)
    }

    /// Single-character rendering used by human-facing reports.
    #[inline]
    #[must_use]
    pub const fn glyph(self) -> char {
        match self {
            Tri::False => 'F',
            Tri::Unknown => 'U',
            Tri::True => 'T',
        }
    }

    /// Position in truth order, also the array index used by [`crate::Facts`].
    #[inline]
    #[must_use]
    pub const fn rank(self) -> u8 {
        self as u8
    }
}

impl core::fmt::Debug for Tri {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Tri::False => "F",
            Tri::Unknown => "U",
            Tri::True => "T",
        })
    }
}

impl core::fmt::Display for Tri {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Tri::False => "false",
            Tri::Unknown => "unknown",
            Tri::True => "true",
        })
    }
}

impl From<bool> for Tri {
    #[inline]
    fn from(value: bool) -> Self {
        Tri::from_bool(value)
    }
}

#[cfg(test)]
mod tests {
    use super::Tri;

    #[test]
    fn truth_order_is_a_total_lattice() {
        // F < U < T, and min/max agree with and/or on every decided pair.
        assert!(Tri::False < Tri::Unknown);
        assert!(Tri::Unknown < Tri::True);
        for a in Tri::ALL {
            assert_eq!(a.and(a), a);
            assert_eq!(a.or(a), a);
            for b in Tri::ALL {
                assert_eq!(a.and(b), b.and(a));
                assert_eq!(a.or(b), b.or(a));
                assert_eq!(a.and(b).rank(), a.rank().min(b.rank()));
                assert_eq!(a.or(b).rank(), a.rank().max(b.rank()));
            }
        }
    }

    #[test]
    fn kleene_laws_hold() {
        for a in Tri::ALL {
            // Involution.
            assert_eq!(a.not().not(), a);
            // For decided values, excluded middle and non-contradiction both fail:
            // that is the whole point of a third value. At `Unknown` the two sides
            // coincide, which is the fixed point of negation.
            if let Some(b) = a.decided() {
                assert_eq!(a.and(a.not()), Tri::from_bool(false));
                assert_eq!(a.or(a.not()), Tri::from_bool(true));
                assert_ne!(a.and(a.not()), a.or(a.not()));
                let _ = b;
            } else {
                assert_eq!(a.and(a.not()), Tri::Unknown);
                assert_eq!(a.or(a.not()), Tri::Unknown);
            }
            // De Morgan.
            for b in Tri::ALL {
                assert_eq!(a.and(b).not(), a.not().or(b.not()));
                assert_eq!(a.or(b).not(), a.not().and(b.not()));
                // Absorption.
                assert_eq!(a.or(a.and(b)), a);
                assert_eq!(a.and(a.or(b)), a);
                // Distributivity.
                for c in Tri::ALL {
                    assert_eq!(a.and(b.or(c)), a.and(b).or(a.and(c)));
                    assert_eq!(a.or(b.and(c)), a.or(b).and(a.or(c)));
                }
            }
        }
    }

    #[test]
    fn unknown_never_becomes_decided() {
        for a in Tri::ALL {
            assert_eq!(a.and(Tri::Unknown).is_unknown(), !a.is_false());
            assert_eq!(a.or(Tri::Unknown).is_unknown(), !a.is_true());
        }
    }

    #[test]
    fn implication_matches_classical_when_decided() {
        for a in [Tri::False, Tri::True] {
            for b in [Tri::False, Tri::True] {
                let expected = (!a.decided().unwrap()) | b.decided().unwrap();
                assert_eq!(a.implies(b), Tri::from_bool(expected));
            }
        }
        // U -> F is U (could still be falsified), F -> U is T (vacuously true).
        assert!(Tri::Unknown.implies(Tri::False).is_unknown());
        assert!(Tri::False.implies(Tri::Unknown).is_true());
    }

    #[test]
    fn decided_roundtrip() {
        assert_eq!(Tri::from_bool(true).decided(), Some(true));
        assert_eq!(Tri::from_bool(false).decided(), Some(false));
        assert_eq!(Tri::Unknown.decided(), None);
        assert!(!Tri::Unknown.is_true());
        assert!(!Tri::Unknown.is_false());
    }
}
