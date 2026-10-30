//! Rules: a tiny declarative language, and the three-valued evaluation of a rule
//! set against a parsed image.
//!
//! # The one behaviour that matters
//!
//! Two different things can go wrong with a scoped rule, and conflating them either
//! loses detections or destroys the output.
//!
//! * **Applicability** — is the structure the rule targets present at all? A rule
//!   scoped to `.rsrc` has no bearing on a file with no `.rsrc` section. That is a
//!   decidable *no*, because the section table was read successfully and said so.
//!   Reporting this as `Unknown` is the tempting mistake: it makes every rule
//!   contribute uncertainty to every file, and the verdict becomes permanently
//!   undecided — precise in theory, useless in practice.
//! * **Decidability** — the region exists but some of its bytes are not in the
//!   file. This is where `Unknown` belongs, and where a conventional boolean
//!   matcher silently returns "no match". A signature aimed at offset 0x1800 of a
//!   section that the file cuts off at 0x800 is *untested*, not negative.
//!
//! [`Hit`] carries both facts, so a report can say "no hit, and the rule did not
//! apply" separately from "no hit, and I could not see the region".
//!
//! # Format
//!
//! ```text
//! # comments run to end of line
//! rule upx_magic
//!   severity medium
//!   scope section:UPX
//!   match 55 50 58 21          # "UPX!"
//!   forbid 00 00 00
//! end
//! ```
//!
//! * `severity` — `high`, `medium`, `low`. High escalates the file to Malicious,
//!   medium to Suspicious, low is informational and never graded.
//! * `scope` — `any`, `file`, `overlay`, `headers`, or `section:<name>`. A
//!   `section:` prefix match is used, so `section:UPX` covers `UPX0`, `UPX1`, ...
//! * `match` — may repeat; **all** must match.
//! * `forbid` — may repeat; **none** may match.
//! * `desc` — free text.
//!
//! Rule-level negation (`forbid`) is safe here because it is resolved *before* the
//! monotone hypothesis layer: the rule reduces to a single trit, and the hypothesis
//! algebra only ever sees that trit.

use t3_core::Tri;
use t3_format::{Region, RegionKind};

use crate::pattern::{Pattern, PatternError};

/// How much weight a hit carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational; never raises the grade.
    Low,
    /// Raises the file to `Suspicious`.
    Medium,
    /// Raises the file to `Malicious`.
    High,
}

impl Severity {
    /// Parses the wire form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "high" => Some(Severity::High),
            "medium" => Some(Severity::Medium),
            "low" => Some(Severity::Low),
            _ => None,
        }
    }

    /// Wire form.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

/// Which regions a rule applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every region, including headers and overlay.
    Any,
    /// The whole file as one region.
    File,
    /// The overlay only.
    Overlay,
    /// The header block only.
    Headers,
    /// Sections whose name starts with the given bytes.
    Section(Vec<u8>),
}

impl Scope {
    /// Parses the wire form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "any" => Some(Scope::Any),
            "file" => Some(Scope::File),
            "overlay" => Some(Scope::Overlay),
            "headers" => Some(Scope::Headers),
            _ => text
                .strip_prefix("section:")
                .map(|name| Scope::Section(name.as_bytes().to_vec())),
        }
    }

    /// Does this scope select the region?
    #[must_use]
    pub fn matches(&self, region: &Region<'_>) -> bool {
        match self {
            Scope::Any => true,
            Scope::File => matches!(region.kind, RegionKind::WholeFile),
            Scope::Overlay => matches!(region.kind, RegionKind::Overlay),
            Scope::Headers => matches!(region.kind, RegionKind::Headers),
            Scope::Section(prefix) => {
                matches!(region.kind, RegionKind::Section(_)) && region.name().starts_with(prefix)
            }
        }
    }

    /// Wire form, for reports.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Scope::Any => "any".to_string(),
            Scope::File => "file".to_string(),
            Scope::Overlay => "overlay".to_string(),
            Scope::Headers => "headers".to_string(),
            Scope::Section(p) => format!("section:{}", String::from_utf8_lossy(p)),
        }
    }
}

/// One compiled rule.
#[derive(Clone, Debug)]
pub struct Rule {
    /// Unique identifier, `snake_case`.
    pub name: String,
    /// Weight of a hit.
    pub severity: Severity,
    /// Where to look.
    pub scope: Scope,
    /// All of these must match.
    pub matches: Vec<Pattern>,
    /// None of these may match.
    pub forbids: Vec<Pattern>,
    /// Free description.
    pub description: String,
}

impl Rule {
    /// Evaluates the rule against a region set, returning the value and whether
    /// the rule's scope was present at all.
    #[must_use]
    pub fn eval(&self, regions: &[Region<'_>], bitmaps: &[Option<[u64; 4]>]) -> (Tri, bool) {
        if self.matches.is_empty() {
            // A rule with no positive pattern would fire on everything; the parser
            // rejects this, but the type cannot enforce it.
            return (Tri::False, false);
        }
        let mut value = Tri::True;
        let mut applicable = true;
        for pattern in &self.matches {
            let (v, applies) = pattern_in_scope(pattern, &self.scope, regions, bitmaps);
            applicable &= applies;
            value = value.and(v);
            if value.is_false() {
                return (Tri::False, applicable);
            }
        }
        for pattern in &self.forbids {
            let (forbidden, applies) = pattern_in_scope(pattern, &self.scope, regions, bitmaps);
            applicable &= applies;
            value = value.and(forbidden.not());
            if value.is_false() {
                return (Tri::False, applicable);
            }
        }
        (value, applicable)
    }
}

fn pattern_in_scope(
    pattern: &Pattern,
    scope: &Scope,
    regions: &[Region<'_>],
    bitmaps: &[Option<[u64; 4]>],
) -> (Tri, bool) {
    let mut acc = Tri::False;
    let mut candidates = 0usize;
    for (index, region) in regions.iter().enumerate() {
        if !scope.matches(region) {
            continue;
        }
        candidates += 1;
        if let Some(Some(presence)) = bitmaps.get(index) {
            if !pattern.could_match(presence) {
                // Sound skip: a byte the pattern requires does not occur anywhere in
                // this region, so no offset can match. This is what keeps the cost near
                // one pass per region instead of one pass per rule.
                continue;
            }
        }
        acc = acc.or(pattern.find(region));
        if acc.is_true() {
            return (Tri::True, true);
        }
    }
    if candidates == 0 {
        // The structure the rule targets is not in this file. Decidable, and not a gap
        // in coverage.
        return (Tri::False, false);
    }
    (acc, true)
}

/// Builds the 256-bit byte-presence bitmap for one region.
///
/// `None` for a truncated region: a byte missing from `data` may still be present in
/// the part of the region the file does not carry, so the prefilter would be unsound.
fn presence_bitmap(region: &Region<'_>) -> Option<[u64; 4]> {
    if region.truncated {
        return None;
    }
    let mut map = [0u64; 4];
    for &byte in region.data {
        map[usize::from(byte) / 64] |= 1u64 << (byte % 64);
    }
    Some(map)
}

/// Result of one rule against one file.
#[derive(Clone, Debug)]
pub struct Hit {
    /// Rule name.
    pub name: String,
    /// Severity of the rule.
    pub severity: Severity,
    /// Three-valued verdict.
    pub value: Tri,
    /// False when the rule's scope selected no region at all. Such a rule is not
    /// evidence either way, and `value` is `False` by convention.
    pub applicable: bool,
    /// Name of the region the match was found in, when it was found.
    pub region: Option<String>,
}

/// A set of rules.
#[derive(Clone, Debug, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    /// Wraps compiled rules.
    #[must_use]
    pub fn new(rules: Vec<Rule>) -> Self {
        RuleSet { rules }
    }

    /// The rules.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Number of rules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// True when there are no rules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Parses a rule file.
    ///
    /// # Errors
    /// Returns the first syntax error, with a 1-based line number.
    pub fn parse(text: &str) -> Result<Self, RuleError> {
        let mut rules = Vec::new();
        let mut current: Option<Rule> = None;

        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let (key, rest) = split_key(line);

            match key {
                "rule" => {
                    if current.is_some() {
                        return Err(RuleError::new(line_no, "nested `rule` before `end`"));
                    }
                    if rest.is_empty() {
                        return Err(RuleError::new(line_no, "rule needs a name"));
                    }
                    if !is_snake_case(rest) {
                        return Err(RuleError::new(line_no, "rule name must be snake_case"));
                    }
                    current = Some(Rule {
                        name: rest.to_string(),
                        severity: Severity::Medium,
                        scope: Scope::Any,
                        matches: Vec::new(),
                        forbids: Vec::new(),
                        description: String::new(),
                    });
                }
                "end" => {
                    let rule = current
                        .take()
                        .ok_or_else(|| RuleError::new(line_no, "`end` without `rule`"))?;
                    if rule.matches.is_empty() {
                        return Err(RuleError::new(
                            line_no,
                            "rule has no `match` pattern and would fire on everything",
                        ));
                    }
                    if rules.iter().any(|r: &Rule| r.name == rule.name) {
                        return Err(RuleError::new(line_no, "duplicate rule name"));
                    }
                    rules.push(rule);
                }
                "severity" => {
                    let rule = current
                        .as_mut()
                        .ok_or_else(|| RuleError::new(line_no, "`severity` outside a rule"))?;
                    rule.severity = Severity::parse(rest).ok_or_else(|| {
                        RuleError::new(line_no, "severity must be high, medium or low")
                    })?;
                }
                "scope" => {
                    let rule = current
                        .as_mut()
                        .ok_or_else(|| RuleError::new(line_no, "`scope` outside a rule"))?;
                    rule.scope = Scope::parse(rest)
                        .ok_or_else(|| RuleError::new(line_no, "unknown scope"))?;
                }
                "match" | "forbid" => {
                    let rule = current
                        .as_mut()
                        .ok_or_else(|| RuleError::new(line_no, "pattern outside a rule"))?;
                    let pattern = Pattern::parse(rest)
                        .map_err(|e| RuleError::new(line_no, pattern_message(&e)))?;
                    if key == "match" {
                        rule.matches.push(pattern);
                    } else {
                        rule.forbids.push(pattern);
                    }
                }
                "desc" => {
                    let rule = current
                        .as_mut()
                        .ok_or_else(|| RuleError::new(line_no, "`desc` outside a rule"))?;
                    rule.description = rest.to_string();
                }
                other => {
                    return Err(RuleError::new(
                        line_no,
                        format!("unknown directive `{other}`"),
                    ))
                }
            }
        }

        if let Some(rule) = current {
            return Err(RuleError::new(
                text.lines().count().max(1),
                format!("rule `{}` was not closed with `end`", rule.name),
            ));
        }
        Ok(RuleSet { rules })
    }

    /// Evaluates every rule.
    ///
    /// Presence bitmaps are built once per region and shared by every rule. Building
    /// them inside the rule loop would cost one full pass per rule and defeat the
    /// prefilter entirely, which is what the first version did.
    #[must_use]
    pub fn evaluate(&self, regions: &[Region<'_>]) -> Vec<Hit> {
        let bitmaps: Vec<Option<[u64; 4]>> = regions.iter().map(presence_bitmap).collect();
        self.rules
            .iter()
            .map(|rule| {
                let (value, applicable) = rule.eval(regions, &bitmaps);
                let region = if value.is_true() {
                    find_matching_region(rule, regions)
                } else {
                    None
                };
                Hit {
                    name: rule.name.clone(),
                    severity: rule.severity,
                    value,
                    applicable,
                    region,
                }
            })
            .collect()
    }

    /// Folds rule hits into the two coarse facts the hypothesis layer consumes.
    ///
    /// Returns `(high, medium)`. A rule that could not be decided keeps the
    /// corresponding fact `Unknown`, which is how an untested rule widens the
    /// verdict instead of disappearing from it.
    #[must_use]
    pub fn fold(&self, hits: &[Hit]) -> (Tri, Tri) {
        let mut high = Tri::False;
        let mut medium = Tri::False;
        for hit in hits {
            match hit.severity {
                Severity::High => high = high.or(hit.value),
                Severity::Medium => medium = medium.or(hit.value),
                Severity::Low => {}
            }
        }
        (high, medium)
    }
}

fn find_matching_region(rule: &Rule, regions: &[Region<'_>]) -> Option<String> {
    regions
        .iter()
        .find(|region| {
            rule.scope.matches(region) && rule.matches.iter().all(|p| p.find(region).is_true())
        })
        .map(|region| region.name_lossy().into_owned())
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn split_key(line: &str) -> (&str, &str) {
    match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    }
}

fn is_snake_case(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !text.starts_with(char::is_numeric)
}

fn pattern_message(error: &PatternError) -> String {
    format!("pattern: {error}")
}

/// Why a rule file was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleError {
    /// 1-based line number.
    pub line: usize,
    /// Human explanation.
    pub message: String,
}

impl RuleError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        RuleError {
            line,
            message: message.into(),
        }
    }
}

impl core::fmt::Display for RuleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::{RuleSet, Scope, Severity};
    use t3_core::Tri;
    use t3_format::{Region, RegionKind};

    const SAMPLE: &str = r"
# a comment
rule upx_magic
  severity medium
  scope section:UPX
  match 55 50 58 21
  desc UPX! banner in a packer section
end

rule msvc_prologue
  severity low
  scope section:.text
  match 55 8B EC
  forbid 55 8B EC 83
end
";

    fn regions<'a>(data: &'a [u8], name: &[u8], declared: usize) -> Vec<Region<'a>> {
        vec![Region::new(RegionKind::Section(0), name, 0, data, declared)]
    }

    #[test]
    fn parses_the_sample() {
        let set = RuleSet::parse(SAMPLE).unwrap();
        assert_eq!(set.len(), 2);
        assert_eq!(set.rules()[0].name, "upx_magic");
        assert_eq!(set.rules()[0].severity, Severity::Medium);
        assert_eq!(set.rules()[0].scope, Scope::Section(b"UPX".to_vec()));
        assert_eq!(set.rules()[0].forbids.len(), 0);
        assert_eq!(set.rules()[1].forbids.len(), 1);
    }

    #[test]
    fn reports_missing_end() {
        let err = RuleSet::parse("rule a\n  match 41\n").unwrap_err();
        assert!(err.message.contains("not closed"), "{err}");
    }

    #[test]
    fn rejects_a_rule_without_a_pattern() {
        let err = RuleSet::parse("rule a\n  severity high\nend\n").unwrap_err();
        assert!(err.message.contains("no `match`"), "{err}");
    }

    #[test]
    fn rejects_duplicate_names() {
        let text = "rule a\n match 41\nend\nrule a\n match 42\nend\n";
        assert!(RuleSet::parse(text)
            .unwrap_err()
            .message
            .contains("duplicate"));
    }

    #[test]
    fn unknown_directive_is_an_error() {
        assert!(RuleSet::parse("rule a\n match 41\n nope 1\nend\n").is_err());
    }

    #[test]
    fn an_absent_scope_is_inapplicable_not_unknown() {
        let set = RuleSet::parse(SAMPLE).unwrap();
        // Data does contain UPX!, but there is no UPX section, so the rule has no
        // bearing on this file.
        let data = b"....UPX!....";
        let hits = set.evaluate(&regions(data, b".text", data.len()));
        let upx = hits.iter().find(|h| h.name == "upx_magic").unwrap();
        assert_eq!(upx.value, Tri::False);
        assert!(!upx.applicable, "must be reported as not applicable");
    }

    #[test]
    fn a_hit_is_found_in_its_scope() {
        let set = RuleSet::parse(SAMPLE).unwrap();
        let data = b"....UPX!....";
        let hits = set.evaluate(&regions(data, b"UPX0", data.len()));
        let upx = hits.iter().find(|h| h.name == "upx_magic").unwrap();
        assert_eq!(upx.value, Tri::True);
        assert_eq!(upx.region.as_deref(), Some("UPX0"));
    }

    #[test]
    fn forbid_blocks_a_hit() {
        let set = RuleSet::parse(SAMPLE).unwrap();
        let plain = [0x55u8, 0x8B, 0xEC, 0x00];
        let forbidden = [0x55u8, 0x8B, 0xEC, 0x83];
        let hits_plain = set.evaluate(&regions(&plain, b".text", plain.len()));
        assert_eq!(
            hits_plain
                .iter()
                .find(|h| h.name == "msvc_prologue")
                .unwrap()
                .value,
            Tri::True
        );
        let hits_forbidden = set.evaluate(&regions(&forbidden, b".text", forbidden.len()));
        assert_eq!(
            hits_forbidden
                .iter()
                .find(|h| h.name == "msvc_prologue")
                .unwrap()
                .value,
            Tri::False
        );
    }

    #[test]
    fn truncated_scope_makes_the_rule_unknown() {
        let set = RuleSet::parse(SAMPLE).unwrap();
        // Declared 4096 bytes of a UPX section but only 4 present with no banner.
        let data = [0u8; 4];
        let hits = set.evaluate(&regions(&data, b"UPX0", 4096));
        let upx = hits.iter().find(|h| h.name == "upx_magic").unwrap();
        assert_eq!(
            upx.value,
            Tri::Unknown,
            "present-but-truncated is undecided"
        );
        assert!(upx.applicable);
    }

    #[test]
    fn fold_keeps_unknown_alive() {
        let set = RuleSet::parse(SAMPLE).unwrap();
        let data = b"\x55\x8B\xEC\x00";
        let hits = set.evaluate(&regions(data, b".text", data.len()));
        let (high, medium) = set.fold(&hits);
        // upx_magic's scope is absent, so it contributes nothing: medium is a
        // decidable false rather than a permanent hole in the verdict.
        assert_eq!(high, Tri::False);
        assert_eq!(medium, Tri::False);
    }
}
