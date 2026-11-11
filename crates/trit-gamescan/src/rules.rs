use std::collections::{BTreeMap, BTreeSet};

use crate::findings::{Finding, RuleOutcome, Severity};
use crate::penetration::PenetrationOracle;
use crate::rule::StructuralRule;
use crate::snapshot::{EntityId, WorldSnapshot};

pub struct NoDuplicateIds;

impl StructuralRule for NoDuplicateIds {
    fn id(&self) -> &'static str {
        "structure.no_duplicate_ids"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let mut seen = BTreeSet::new();
        let mut findings = Vec::new();
        for entity in &snapshot.entities {
            if !seen.insert(entity.id) {
                findings.push(
                    Finding::fail(
                        self.id(),
                        Severity::High,
                        format!("duplicate entity id {}", entity.id),
                        format!("entity id {} appears more than once", entity.id),
                    )
                    .with_target(entity.id),
                );
            }
        }
        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

pub struct ParentsExist;

impl StructuralRule for ParentsExist {
    fn id(&self) -> &'static str {
        "structure.parents_exist"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let ids = snapshot.entity_ids();
        let mut findings = Vec::new();
        for entity in &snapshot.entities {
            if let Some(parent) = entity.parent
                && !ids.contains(&parent)
            {
                findings.push(
                    Finding::fail(
                        self.id(),
                        Severity::High,
                        format!("entity {} references missing parent {}", entity.id, parent),
                        format!("parent id {} not present", parent),
                    )
                    .with_target(entity.id),
                );
            }
        }
        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

pub struct NoHierarchyCycles;

impl StructuralRule for NoHierarchyCycles {
    fn id(&self) -> &'static str {
        "structure.no_hierarchy_cycles"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        if snapshot.entity_count() != snapshot.entity_ids().len() {
            return RuleOutcome::unknown(
                self.id(),
                vec![Finding::unknown(
                    self.id(),
                    Severity::Medium,
                    "hierarchy cycle check is inconclusive with duplicate ids",
                    "duplicate entity ids prevent a unique parent map",
                )],
            );
        }

        let mut parent_map: BTreeMap<EntityId, Option<EntityId>> = BTreeMap::new();
        for entity in &snapshot.entities {
            parent_map.insert(entity.id, entity.parent);
        }

        let mut findings = Vec::new();
        for start in parent_map.keys().copied() {
            let mut seen = BTreeSet::new();
            let mut current = Some(start);
            while let Some(id) = current {
                if !seen.insert(id) {
                    findings.push(
                        Finding::fail(
                            self.id(),
                            Severity::Critical,
                            format!("hierarchy cycle reaches entity {}", id),
                            format!("parent chain starting at {} revisits {}", start, id),
                        )
                        .with_target(start),
                    );
                    break;
                }
                current = parent_map.get(&id).copied().flatten();
            }
        }

        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

pub struct TransformFinite;

impl StructuralRule for TransformFinite {
    fn id(&self) -> &'static str {
        "physics.transform_finite"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let mut findings = Vec::new();
        for entity in &snapshot.entities {
            push_non_finite(
                &mut findings,
                self.id(),
                entity.id,
                "position",
                &entity.transform.position,
            );
            push_non_finite(
                &mut findings,
                self.id(),
                entity.id,
                "rotation",
                &entity.transform.rotation,
            );
            push_non_finite(
                &mut findings,
                self.id(),
                entity.id,
                "scale",
                &entity.transform.scale,
            );
        }
        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

pub struct ScaleNonZeroFinite;

impl StructuralRule for ScaleNonZeroFinite {
    fn id(&self) -> &'static str {
        "physics.scale_nonzero_finite"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let mut findings = Vec::new();
        for entity in &snapshot.entities {
            for (axis, value) in entity.transform.scale.iter().enumerate() {
                if !value.is_finite() || value.abs() <= f64::EPSILON {
                    findings.push(
                        Finding::fail(
                            self.id(),
                            Severity::High,
                            format!("entity {} has invalid scale on axis {}", entity.id, axis),
                            format!("scale[{}] = {}", axis, value),
                        )
                        .with_target(entity.id),
                    );
                }
            }
        }
        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

pub struct RotationNormalized {
    pub tolerance: f64,
}

impl Default for RotationNormalized {
    fn default() -> Self {
        Self { tolerance: 1.0e-3 }
    }
}

impl StructuralRule for RotationNormalized {
    fn id(&self) -> &'static str {
        "physics.rotation_normalized"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let mut findings = Vec::new();
        for entity in &snapshot.entities {
            let rotation = entity.transform.rotation;
            let norm_squared = rotation.iter().map(|value| value * value).sum::<f64>();
            let norm = norm_squared.sqrt();
            if !norm.is_finite() || norm <= f64::EPSILON || (norm - 1.0).abs() > self.tolerance {
                findings.push(
                    Finding::fail(
                        self.id(),
                        Severity::Medium,
                        format!("entity {} has non-normalized rotation", entity.id),
                        format!("quaternion norm = {}", norm),
                    )
                    .with_target(entity.id),
                );
            }
        }
        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

pub struct ResourceKeysUnique;

impl StructuralRule for ResourceKeysUnique {
    fn id(&self) -> &'static str {
        "assets.resource_keys_unique"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let mut seen = BTreeSet::new();
        let mut findings = Vec::new();
        for resource in &snapshot.resources {
            if !seen.insert(resource.key.as_str()) {
                findings.push(Finding::fail(
                    self.id(),
                    Severity::Medium,
                    format!("duplicate resource key {}", resource.key),
                    format!("resource key {} appears more than once", resource.key),
                ));
            }
        }
        if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::fail(self.id(), findings)
        }
    }
}

fn push_non_finite(
    findings: &mut Vec<Finding>,
    rule_id: &'static str,
    entity: EntityId,
    field: &str,
    values: &[f64],
) {
    for (axis, value) in values.iter().enumerate() {
        if !value.is_finite() {
            findings.push(
                Finding::fail(
                    rule_id,
                    Severity::High,
                    format!(
                        "entity {} has non-finite transform.{}[{}]",
                        entity, field, axis
                    ),
                    format!("{}[{}] = {}", field, axis, value),
                )
                .with_target(entity),
            );
        }
    }
}

pub fn default_structural_rules() -> Vec<Box<dyn StructuralRule>> {
    vec![
        Box::new(NoDuplicateIds),
        Box::new(ParentsExist),
        Box::new(NoHierarchyCycles),
        Box::new(TransformFinite),
        Box::new(ScaleNonZeroFinite),
        Box::new(RotationNormalized::default()),
        Box::new(ResourceKeysUnique),
        Box::new(PenetrationOracle::default()),
    ]
}
