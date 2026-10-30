//! `t3-sig` — three-valued signature matching.
//!
//! The engine answers "does this pattern occur in this image?" with a [`t3_core::Tri`]
//! where the third value is earned, not decorative:
//!
//! * the pattern matched → `True`;
//! * every position it could occupy was present in the file and none matched →
//!   `False`;
//! * the pattern's scope existed but bytes it needed were absent → `Unknown`.
//!
//! The third case is the whole reason this crate exists. A boolean matcher reports
//! it as "no match", which is indistinguishable from a real negative, and that is
//! how a truncated download or a section table edited to hide a region produces a
//! clean bill of health.
//!
//! # Composition with the hypothesis layer
//!
//! Rules collapse to two coarse facts (`sig.high` and `sig.medium`), which enter the
//! monotone hypothesis algebra unchanged. A rule that could not be evaluated keeps
//! the final verdict open through the same mechanism as an unmeasured entropy value:
//! no special case, no second code path.

#![deny(missing_docs)]
#![warn(clippy::pedantic)]

pub mod builtin;
pub mod pattern;
pub mod rule;

use std::sync::OnceLock;

pub use pattern::{Pattern, PatternError, Slot, MAX_PATTERN};
pub use rule::{Hit, Rule, RuleError, RuleSet, Scope, Severity};

/// The built-in rule set, parsed once per process.
///
/// Parsing is cheap but not free, and a scan evaluates thousands of files against
/// the same set, so it is cached. A parse failure here would be a build-time bug,
/// caught by the unit tests in [`builtin`].
///
/// # Panics
///
/// Panics if the built-in rule text fails to parse — that can only happen when the
/// embedded rules are edited incorrectly, and failing loudly beats scanning with an
/// empty rule set.
#[must_use]
pub fn builtin_rules() -> &'static RuleSet {
    static RULES: OnceLock<RuleSet> = OnceLock::new();
    RULES.get_or_init(|| {
        RuleSet::parse(builtin::BUILTIN_RULES).unwrap_or_else(|err| {
            // Unreachable by construction; failing loudly beats scanning with an
            // empty rule set and silently reporting everything as clean.
            panic!("built-in rules failed to parse: {err}")
        })
    })
}

/// Result of running a rule set over one image.
#[derive(Clone, Debug)]
pub struct Scan {
    /// Every rule's outcome, in rule order.
    pub hits: Vec<Hit>,
    /// Folded high-severity fact.
    pub high: t3_core::Tri,
    /// Folded medium-severity fact.
    pub medium: t3_core::Tri,
}

impl Default for Scan {
    fn default() -> Self {
        Scan {
            hits: Vec::new(),
            high: t3_core::Tri::False,
            medium: t3_core::Tri::False,
        }
    }
}

impl Scan {
    /// Rules that matched.
    pub fn matched(&self) -> impl Iterator<Item = &Hit> {
        self.hits.iter().filter(|h| h.value.is_true())
    }

    /// Rules that could not be evaluated because their bytes were missing.
    pub fn undecided(&self) -> impl Iterator<Item = &Hit> {
        self.hits.iter().filter(|h| h.value.is_unknown())
    }
}

/// Runs a rule set over a region list.
#[must_use]
pub fn evaluate(rules: &RuleSet, regions: &[t3_format::Region<'_>]) -> Scan {
    let hits = rules.evaluate(regions);
    let (high, medium) = rules.fold(&hits);
    Scan { hits, high, medium }
}

#[cfg(test)]
mod tests {
    use super::{builtin_rules, evaluate};
    use t3_core::Tri;
    use t3_format::{Region, RegionKind};

    #[test]
    fn builtin_set_loads_once() {
        let a = builtin_rules();
        let b = builtin_rules();
        assert!(std::ptr::eq(a, b), "the rule set must be cached");
    }

    #[test]
    fn detects_a_mimikatz_string_as_high() {
        let data = b"....sekurlsa::logonpasswords....";
        let regions = vec![Region::complete(RegionKind::Section(0), b".rdata", 0, data)];
        let scan = evaluate(builtin_rules(), &regions);
        assert!(scan.high.is_true(), "high severity must fold to True");
        assert!(scan.matched().any(|h| h.name == "mimikatz_sekurlsa"));
    }

    #[test]
    fn clean_data_folds_to_decided_false() {
        let data = vec![0u8; 4096];
        let regions = vec![Region::complete(RegionKind::WholeFile, b"file", 0, &data)];
        let scan = evaluate(builtin_rules(), &regions);
        assert_eq!(scan.high, Tri::False);
        assert_eq!(scan.medium, Tri::False);
    }

    #[test]
    fn truncated_region_leaves_the_signature_undecided() {
        // 8 real bytes, 1 MiB declared, nothing that looks like a banner.
        let data = [0u8; 8];
        let regions = vec![Region::new(
            RegionKind::Section(0),
            b".text",
            0,
            &data,
            1024 * 1024,
        )];
        let scan = evaluate(builtin_rules(), &regions);
        assert_eq!(scan.high, Tri::Unknown);
        assert_eq!(scan.medium, Tri::Unknown);
        assert!(scan.undecided().count() > 0);
    }
}
