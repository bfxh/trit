//! From facts to a graded verdict, with an explicit uncertainty budget.
//!
//! A [`Verdict`] is not a label. It is a pair of grades — the lowest grade still
//! consistent with the evidence, and the highest — plus a conflict flag from
//! Belnap's `B`. Three of those four shapes are useful and none of them are
//! expressible as a single number:
//!
//! ```text
//!   [Clean, Clean]          nothing found, and everything that could have been
//!                           decided was decided
//!   [Malicious, Malicious]  established
//!   [Clean, Malicious]      undecided: the unknowns matter
//!   conflict = true         the evidence argues both ways
//! ```
//!
//! The tool also answers the question every analyst asks next: *what would change
//! this?* [`Judgment::would_establish`] lists the undecided facts that, if they
//! turned out true, would settle the hypothesis. That list is computed by
//! re-evaluation, not by guessing, and it is why the interval representation is
//! worth the extra work.

use crate::b4::B4;
use crate::expr::{Expr, Interval};
use crate::fact::{FactId, Facts};
use crate::Tri;

/// Severity contributed by a hypothesis when it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Grade {
    /// Nothing actionable.
    Clean = 0,
    /// Worth a look; insufficient for a determination.
    Suspicious = 1,
    /// A concrete malicious capability or strong structural anomaly.
    Malicious = 2,
}

impl Grade {
    /// Number of grades.
    pub const COUNT: usize = 3;

    /// All grades, ascending.
    pub const ALL: [Grade; 3] = [Grade::Clean, Grade::Suspicious, Grade::Malicious];

    /// Machine-readable name, part of the JSONL contract.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Grade::Clean => "clean",
            Grade::Suspicious => "suspicious",
            Grade::Malicious => "malicious",
        }
    }

    /// One-line human description.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Grade::Clean => "未发现可疑迹象",
            Grade::Suspicious => "可疑",
            Grade::Malicious => "恶意能力",
        }
    }

    /// Exit-code fragment: `0` clean, `1` suspicious, `2` malicious.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

impl core::fmt::Display for Grade {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// Which container a hypothesis can be evaluated against.
///
/// Applicability is separate from truth, and conflating them is expensive in both
/// directions. A rule about ELF segment permissions evaluated on a PE file is not
/// *false* — it is irrelevant, and if it is treated as undecided it leaves every PE
/// file permanently open; if it is treated as false it silently claims a check that
/// never happened. Neither is acceptable, so applicability is its own axis and an
/// inapplicable hypothesis contributes nothing to the grade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Applies to any input, including one whose container was not recognised.
    ///
    /// Reserved for checks that are meaningful on raw bytes: signature matching, for
    /// instance, works on shellcode and on truncated fragments just as well as on a
    /// well-formed image.
    Any,
    /// Applies when the input was identified as a PE *or* an ELF image.
    ///
    /// This is the right scope for anything structural. A `.nls` codepage table is not
    /// an undecided executable — it is not an executable at all, and letting structural
    /// hypotheses report `[clean, malicious]` for it turned three quarters of a Windows
    /// installation into noise.
    Binary,
    /// Only when the input was identified as PE.
    Pe,
    /// Only when the input was identified as ELF.
    Elf,
}

/// A named, monotone hypothesis plus the severity it implies.
#[derive(Clone, Debug)]
pub struct Hypothesis {
    /// Stable identifier; appears in JSONL and must not change casually.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Monotone condition.
    pub expr: Expr,
    /// Independent explanation that the condition is benign. When both hold the
    /// judgement is a Belnap conflict rather than a plain positive.
    pub benign: Option<Expr>,
    /// Grade implied when the condition holds.
    pub severity: Grade,
    /// Container this hypothesis applies to.
    pub formats: Format,
}

impl Hypothesis {
    /// Convenience constructor without a benign explanation.
    #[must_use]
    pub fn new(id: &'static str, title: &'static str, expr: Expr, severity: Grade) -> Self {
        Hypothesis {
            id,
            title,
            expr,
            benign: None,
            severity,
            formats: Format::Any,
        }
    }

    /// Restricts the hypothesis to one container.
    #[must_use]
    pub fn only(mut self, format: Format) -> Self {
        self.formats = format;
        self
    }

    /// Attaches a benign explanation.
    #[must_use]
    pub fn with_benign(mut self, benign: Expr) -> Self {
        self.benign = Some(benign);
        self
    }
}

/// Outcome for one hypothesis.
#[derive(Clone, Debug)]
pub struct Judgment {
    /// Hypothesis identifier.
    pub id: &'static str,
    /// Hypothesis title.
    pub title: &'static str,
    /// Severity the hypothesis implies.
    pub severity: Grade,
    /// Exact enclosure of the condition.
    pub interval: Interval,
    /// Belnap value: `B` when the condition and its benign explanation both hold.
    pub four: B4,
    /// False when the hypothesis does not apply to this container. Such a judgement
    /// is reported but never graded.
    pub applicable: bool,
    /// Undecided facts the condition depends on.
    pub undecided: Vec<FactId>,
    /// Subset of `undecided` that would establish the hypothesis if true.
    /// Only filled by [`Engine::explain`].
    pub would_establish: Vec<FactId>,
    /// Subset of `undecided` that would refute the hypothesis if false.
    /// Only filled by [`Engine::explain`].
    pub would_refute: Vec<FactId>,
}

impl Judgment {
    /// True when the condition is established.
    #[must_use]
    pub fn is_established(&self) -> bool {
        self.interval.lo.is_true()
    }

    /// True when the condition could still hold.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.interval.hi.is_false()
    }
}

/// Aggregate result for one file.
#[derive(Clone, Debug)]
pub struct Verdict {
    /// Lowest grade consistent with the evidence.
    pub lo: Grade,
    /// Highest grade consistent with the evidence.
    pub hi: Grade,
    /// True when at least one actionable hypothesis has conflicting support.
    pub conflict: bool,
    /// Per-hypothesis detail.
    pub judgments: Vec<Judgment>,
}

impl Verdict {
    /// True when the evidence pins the verdict down.
    #[must_use]
    pub fn is_decided(&self) -> bool {
        self.lo == self.hi
    }

    /// The grade to act on: the highest still-possible one.
    #[must_use]
    pub fn headline(&self) -> Grade {
        self.hi
    }

    /// Process exit code: `0` clean, `1` suspicious, `2` malicious, `3` undecided
    /// with a malicious ceiling, `4` conflict.
    ///
    /// Encoding the *shape* of the answer in the exit status is the one place a
    /// shell can see the difference between "clean" and "I do not know".
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.conflict {
            4
        } else if !self.is_decided() {
            3
        } else {
            self.hi.code()
        }
    }

    /// One-line summary.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.conflict {
            format!("{} (证据冲突)", self.hi)
        } else if self.is_decided() {
            format!("{} (确定)", self.hi)
        } else {
            format!("{}..{} (未定)", self.lo, self.hi)
        }
    }
}

/// A set of hypotheses evaluated together.
#[derive(Clone, Debug, Default)]
pub struct Engine {
    hypotheses: Vec<Hypothesis>,
}

impl Engine {
    /// Builds an engine from hypotheses.
    #[must_use]
    pub fn new(hypotheses: Vec<Hypothesis>) -> Self {
        Engine { hypotheses }
    }

    /// The hypotheses, for rule listing.
    #[must_use]
    pub fn hypotheses(&self) -> &[Hypothesis] {
        &self.hypotheses
    }

    /// Evaluates every hypothesis. No counterfactual analysis: this is the hot
    /// path used when scanning thousands of files.
    #[must_use]
    pub fn verdict(&self, facts: &Facts) -> Verdict {
        self.run(facts, false)
    }

    /// Evaluates every hypothesis and works out which undecided facts would
    /// change the answer.
    ///
    /// Each candidate costs one clone of the fact table and one traversal, so this
    /// is only used for single-file interrogation.
    #[must_use]
    pub fn explain(&self, facts: &Facts) -> Verdict {
        self.run(facts, true)
    }

    fn run(&self, facts: &Facts, with_counterfactuals: bool) -> Verdict {
        let mut judgments = Vec::with_capacity(self.hypotheses.len());
        let mut lo = Grade::Clean;
        let mut hi = Grade::Clean;
        let mut conflict = false;

        for h in &self.hypotheses {
            let applicable = match h.formats {
                Format::Any => true,
                Format::Binary => {
                    facts.get(FactId::FormatPe).is_true() || facts.get(FactId::FormatElf).is_true()
                }
                Format::Pe => facts.get(FactId::FormatPe).is_true(),
                Format::Elf => facts.get(FactId::FormatElf).is_true(),
            };
            if !applicable {
                judgments.push(Judgment {
                    id: h.id,
                    title: h.title,
                    severity: h.severity,
                    interval: Interval::point(Tri::False),
                    four: B4::N,
                    applicable: false,
                    undecided: Vec::new(),
                    would_establish: Vec::new(),
                    would_refute: Vec::new(),
                });
                continue;
            }
            let interval = h.expr.eval_interval(facts);

            // Belnap rails. Both must be *established*, not merely possible: an
            // unknown benign explanation is not evidence of innocence, so it must
            // not manufacture a conflict. Using `hi` here would make every
            // all-unknown file read as contradictory, which is both noisy and
            // false.
            let four = match &h.benign {
                None => B4::from_rails(interval.lo.is_true(), interval.hi.is_false()),
                Some(benign) => {
                    let b = benign.eval_interval(facts);
                    B4::from_rails(interval.lo.is_true(), b.lo.is_true())
                }
            };

            let undecided: Vec<FactId> = h
                .expr
                .referenced()
                .into_iter()
                .filter(|id| facts.get(*id).is_unknown())
                .collect();

            let (would_establish, would_refute) = if with_counterfactuals && !undecided.is_empty() {
                counterfactuals(&h.expr, facts, &undecided)
            } else {
                (Vec::new(), Vec::new())
            };

            // Grade accounting. `lo` only moves for established conditions; `hi`
            // moves for anything that could still hold.
            if interval.lo.is_true() {
                lo = lo.max(h.severity);
            }
            if !interval.hi.is_false() {
                hi = hi.max(h.severity);
            }
            if h.severity != Grade::Clean && four.is_conflict() {
                conflict = true;
            }

            judgments.push(Judgment {
                id: h.id,
                title: h.title,
                severity: h.severity,
                interval,
                four,
                applicable: true,
                undecided,
                would_establish,
                would_refute,
            });
        }

        // A refuted-but-open hypothesis can leave lo below hi with nothing found;
        // that is the honest "undecided" shape, so no clamping is applied.
        hi = hi.max(lo);

        Verdict {
            lo,
            hi,
            conflict,
            judgments,
        }
    }
}

/// Finds undecided facts whose resolution would settle `expr`.
///
/// A fact is reported in `would_establish` when forcing it true makes the
/// condition established, and in `would_refute` when forcing it false refutes it.
/// These are single-fact counterfactuals: they do not claim that the pair is
/// jointly reachable, only that this one observation is load-bearing.
fn counterfactuals(expr: &Expr, facts: &Facts, undecided: &[FactId]) -> (Vec<FactId>, Vec<FactId>) {
    let mut establish = Vec::new();
    let mut refute = Vec::new();
    let mut probe = facts.clone();

    for id in undecided {
        let original = facts.get(*id);

        probe.set(*id, Tri::True, f32::NAN, "");
        if expr.eval_interval(&probe).lo.is_true() {
            establish.push(*id);
        }

        probe.set(*id, Tri::False, f32::NAN, "");
        if expr.eval_interval(&probe).hi.is_false() {
            refute.push(*id);
        }

        probe.set(*id, original, f32::NAN, "");
    }

    (establish, refute)
}

#[cfg(test)]
mod tests {
    use super::{Engine, Format, Grade, Hypothesis};
    use crate::b4::B4;
    use crate::expr::Expr;
    use crate::fact::{FactId, Facts};

    fn engine() -> Engine {
        Engine::new(vec![
            Hypothesis::new(
                "staged_loader",
                "运行期装载 + 网络",
                Expr::fact(FactId::ImportsLoader).and(Expr::fact(FactId::ImportsNetwork)),
                Grade::Malicious,
            ),
            Hypothesis::new(
                "packed",
                "结构像加壳",
                Expr::fact(FactId::SectionHighEntropy).or(Expr::fact(FactId::SectionPackerName)),
                Grade::Suspicious,
            )
            .with_benign(Expr::fact(FactId::SignedPresent)),
        ])
    }

    #[test]
    fn empty_evidence_yields_undecided_not_clean() {
        let v = engine().verdict(&Facts::new());
        // Nothing is established, but both hypotheses could still hold.
        assert_eq!(v.lo, Grade::Clean);
        assert_eq!(v.hi, Grade::Malicious);
        assert!(!v.is_decided());
        assert_eq!(v.exit_code(), 3);
    }

    #[test]
    fn established_hypothesis_pins_the_verdict() {
        let mut f = Facts::new();
        f.set_bool(FactId::ImportsLoader, true, "");
        f.set_bool(FactId::ImportsNetwork, true, "");
        f.set_bool(FactId::SectionHighEntropy, false, "");
        f.set_bool(FactId::SectionPackerName, false, "");
        f.set_bool(FactId::SignedPresent, false, "");
        let v = engine().verdict(&f);
        assert_eq!(v.lo, Grade::Malicious);
        assert_eq!(v.hi, Grade::Malicious);
        assert_eq!(v.exit_code(), 2);
    }

    #[test]
    fn signed_and_packed_is_a_conflict() {
        let mut f = Facts::new();
        f.set_bool(FactId::SectionHighEntropy, true, "");
        f.set_bool(FactId::SectionPackerName, false, "");
        f.set_bool(FactId::SignedPresent, true, "");
        f.set_bool(FactId::ImportsLoader, false, "");
        f.set_bool(FactId::ImportsNetwork, false, "");
        let v = engine().verdict(&f);
        assert!(v.conflict, "签名 + 加壳必须报冲突");
        assert_eq!(v.exit_code(), 4);
        let packed = v.judgments.iter().find(|j| j.id == "packed").unwrap();
        assert_eq!(packed.four, B4::B);
    }

    #[test]
    fn counterfactuals_name_the_load_bearing_fact() {
        let mut f = Facts::new();
        f.set_bool(FactId::ImportsLoader, true, "");
        f.set_bool(FactId::SectionHighEntropy, false, "");
        f.set_bool(FactId::SectionPackerName, false, "");
        f.set_bool(FactId::SignedPresent, false, "");
        // ImportsNetwork is left unknown: it is exactly the missing evidence.
        let v = engine().explain(&f);
        let j = v
            .judgments
            .iter()
            .find(|j| j.id == "staged_loader")
            .unwrap();
        assert_eq!(j.undecided, vec![FactId::ImportsNetwork]);
        assert_eq!(j.would_establish, vec![FactId::ImportsNetwork]);
        // It also refutes: with the loader established, a false network rail
        // settles the conjunction as false. Both directions are load-bearing.
        assert_eq!(j.would_refute, vec![FactId::ImportsNetwork]);
    }

    #[test]
    fn disjunction_can_be_refuted_but_not_established_by_one_fact() {
        let mut f = Facts::new();
        f.set_bool(FactId::ImportsLoader, false, "");
        f.set_bool(FactId::ImportsNetwork, false, "");
        let v = engine().explain(&f);
        let j = v.judgments.iter().find(|j| j.id == "packed").unwrap();
        // Two unknowns in a disjunction: either could establish it, neither alone
        // refutes it.
        assert_eq!(j.would_establish.len(), 2);
        assert_eq!(j.would_refute.len(), 0);
    }

    #[test]
    fn a_hypothesis_for_another_container_contributes_nothing() {
        let engine = Engine::new(vec![Hypothesis::new(
            "elf_only",
            "仅 ELF",
            Expr::fact(FactId::ElfWritableText),
            Grade::Malicious,
        )
        .only(Format::Elf)]);
        // No format fact set: not applicable, so it must not widen the verdict.
        let v = engine.verdict(&Facts::new());
        assert!(
            v.is_decided(),
            "an inapplicable rule must not leave the verdict open"
        );
        assert_eq!(v.hi, Grade::Clean);
        assert!(!v.judgments[0].applicable);

        // Now claim the file is ELF and leave the fact unknown: the rule applies and
        // the verdict opens, which is the intended behaviour.
        let mut f = Facts::new();
        f.set_bool(FactId::FormatElf, true, "");
        let v = engine.verdict(&f);
        assert!(!v.is_decided());
        assert_eq!(v.hi, Grade::Malicious);
        assert!(v.judgments[0].applicable);
    }

    #[test]
    fn fast_path_skips_counterfactuals() {
        let v = engine().verdict(&Facts::new());
        assert!(v.judgments.iter().all(|j| j.would_establish.is_empty()));
    }

    #[test]
    fn grade_ordering_and_codes() {
        assert!(Grade::Clean < Grade::Suspicious);
        assert!(Grade::Suspicious < Grade::Malicious);
        assert_eq!(Grade::Clean.code(), 0);
        assert_eq!(Grade::Malicious.code(), 2);
    }
}
