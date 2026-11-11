use crate::findings::{RuleOutcome, ScanReport};
use crate::snapshot::WorldSnapshot;

pub trait StructuralRule {
    fn id(&self) -> &'static str;

    fn version(&self) -> u32 {
        1
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome;
}

pub fn run_rules(rules: &[Box<dyn StructuralRule>], snapshot: &WorldSnapshot) -> ScanReport {
    let canonical = snapshot.clone().canonicalized();
    let snapshot_hash = canonical.stable_hash();
    let outcomes = rules.iter().map(|rule| rule.check(&canonical)).collect();
    ScanReport {
        snapshot_hash,
        outcomes,
    }
}
