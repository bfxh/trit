use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::hash::Fnv64;

pub type EntityId = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    pub position: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntitySnapshot {
    pub id: EntityId,
    pub parent: Option<EntityId>,
    pub transform: Transform,
    pub tags: BTreeSet<String>,
}

impl EntitySnapshot {
    pub fn new(id: EntityId) -> Self {
        Self {
            id,
            parent: None,
            transform: Transform::default(),
            tags: BTreeSet::new(),
        }
    }

    pub fn with_parent(mut self, parent: EntityId) -> Self {
        self.parent = Some(parent);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSnapshot {
    pub key: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColliderShape {
    Sphere { radius: f64 },
    Capsule { half_height: f64, radius: f64 },
    Box { half_extents: [f64; 3] },
}

impl ColliderShape {
    pub fn tag(&self) -> u8 {
        match self {
            Self::Sphere { .. } => 0,
            Self::Capsule { .. } => 1,
            Self::Box { .. } => 2,
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            Self::Sphere { radius } => radius.is_finite() && *radius > f64::EPSILON,
            Self::Capsule {
                half_height,
                radius,
            } => {
                half_height.is_finite()
                    && *half_height >= 0.0
                    && radius.is_finite()
                    && *radius > f64::EPSILON
            }
            Self::Box { half_extents } => half_extents
                .iter()
                .all(|value| value.is_finite() && *value > f64::EPSILON),
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Sphere { .. } => "sphere",
            Self::Capsule { .. } => "capsule",
            Self::Box { .. } => "box",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColliderSnapshot {
    pub entity: EntityId,
    pub shape: ColliderShape,
    pub local_offset: [f64; 3],
    pub layer: u32,
    pub mask: u32,
    pub is_trigger: bool,
    pub enabled: bool,
}

impl ColliderSnapshot {
    pub fn new(entity: EntityId, shape: ColliderShape) -> Self {
        Self {
            entity,
            shape,
            local_offset: [0.0; 3],
            layer: 0,
            mask: u32::MAX,
            is_trigger: false,
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldSnapshot {
    pub adapter: String,
    pub tick: u64,
    pub entities: Vec<EntitySnapshot>,
    pub resources: Vec<ResourceSnapshot>,
    pub colliders: Vec<ColliderSnapshot>,
}

impl WorldSnapshot {
    pub fn new(adapter: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            tick: 0,
            entities: Vec::new(),
            resources: Vec::new(),
            colliders: Vec::new(),
        }
    }

    pub fn canonicalize(&mut self) {
        self.entities.sort_by(entity_cmp);
        self.resources.sort_by(|a, b| {
            a.key
                .cmp(&b.key)
                .then_with(|| a.generation.cmp(&b.generation))
        });
        self.colliders.sort_by(collider_cmp);
    }

    pub fn canonicalized(mut self) -> Self {
        self.canonicalize();
        self
    }

    pub fn stable_hash(&self) -> u64 {
        self.clone().canonicalized().hash_canonical()
    }

    fn hash_canonical(&self) -> u64 {
        let mut hash = Fnv64::new();
        hash.write_str(&self.adapter);
        hash.write_u64(self.tick);
        hash.write_u64(self.entities.len() as u64);
        for entity in &self.entities {
            hash.write_u64(entity.id);
            match entity.parent {
                Some(parent) => {
                    hash.write(&[1]);
                    hash.write_u64(parent);
                }
                None => hash.write(&[0]),
            }
            for value in entity.transform.position {
                hash.write_f64(value);
            }
            for value in entity.transform.rotation {
                hash.write_f64(value);
            }
            for value in entity.transform.scale {
                hash.write_f64(value);
            }
            hash.write_u64(entity.tags.len() as u64);
            for tag in &entity.tags {
                hash.write_str(tag);
            }
        }
        hash.write_u64(self.resources.len() as u64);
        for resource in &self.resources {
            hash.write_str(&resource.key);
            hash.write_u64(resource.generation);
        }
        hash.write_u64(self.colliders.len() as u64);
        for collider in &self.colliders {
            hash.write_u64(collider.entity);
            hash.write_u64(u64::from(collider.layer));
            hash.write_u64(u64::from(collider.mask));
            hash.write(&[u8::from(collider.is_trigger), u8::from(collider.enabled)]);
            for value in collider.local_offset {
                hash.write_f64(value);
            }
            hash.write(&[collider.shape.tag()]);
            match collider.shape {
                ColliderShape::Sphere { radius } => hash.write_f64(radius),
                ColliderShape::Capsule {
                    half_height,
                    radius,
                } => {
                    hash.write_f64(half_height);
                    hash.write_f64(radius);
                }
                ColliderShape::Box { half_extents } => {
                    for value in half_extents {
                        hash.write_f64(value);
                    }
                }
            }
        }
        hash.finish()
    }

    pub fn entity_ids(&self) -> BTreeSet<EntityId> {
        self.entities.iter().map(|entity| entity.id).collect()
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    pub fn collider_count(&self) -> usize {
        self.colliders.len()
    }

    pub fn find(&self, id: EntityId) -> Option<&EntitySnapshot> {
        self.entities.iter().find(|entity| entity.id == id)
    }

    pub fn find_mut(&mut self, id: EntityId) -> Option<&mut EntitySnapshot> {
        self.entities.iter_mut().find(|entity| entity.id == id)
    }

    pub fn find_colliders(&self, id: EntityId) -> impl Iterator<Item = &ColliderSnapshot> {
        self.colliders
            .iter()
            .filter(move |collider| collider.entity == id)
    }
}

fn entity_cmp(a: &EntitySnapshot, b: &EntitySnapshot) -> Ordering {
    a.id.cmp(&b.id)
        .then_with(|| a.parent.cmp(&b.parent))
        .then_with(|| cmp3(&a.transform.position, &b.transform.position))
        .then_with(|| cmp4(&a.transform.rotation, &b.transform.rotation))
        .then_with(|| cmp3(&a.transform.scale, &b.transform.scale))
        .then_with(|| a.tags.iter().cmp(b.tags.iter()))
}

fn collider_cmp(a: &ColliderSnapshot, b: &ColliderSnapshot) -> Ordering {
    a.entity
        .cmp(&b.entity)
        .then_with(|| a.layer.cmp(&b.layer))
        .then_with(|| a.mask.cmp(&b.mask))
        .then_with(|| a.is_trigger.cmp(&b.is_trigger))
        .then_with(|| a.enabled.cmp(&b.enabled))
        .then_with(|| cmp3(&a.local_offset, &b.local_offset))
        .then_with(|| shape_cmp(&a.shape, &b.shape))
}

fn shape_cmp(a: &ColliderShape, b: &ColliderShape) -> Ordering {
    let tag_order = a.tag().cmp(&b.tag());
    if tag_order != Ordering::Equal {
        return tag_order;
    }
    match (a, b) {
        (
            ColliderShape::Sphere { radius: a_radius },
            ColliderShape::Sphere { radius: b_radius },
        ) => a_radius.total_cmp(b_radius),
        (
            ColliderShape::Capsule {
                half_height: a_half,
                radius: a_radius,
            },
            ColliderShape::Capsule {
                half_height: b_half,
                radius: b_radius,
            },
        ) => a_half
            .total_cmp(b_half)
            .then_with(|| a_radius.total_cmp(b_radius)),
        (
            ColliderShape::Box {
                half_extents: a_half,
            },
            ColliderShape::Box {
                half_extents: b_half,
            },
        ) => cmp3(a_half, b_half),
        _ => Ordering::Equal,
    }
}

fn cmp3(a: &[f64; 3], b: &[f64; 3]) -> Ordering {
    a[0].total_cmp(&b[0])
        .then_with(|| a[1].total_cmp(&b[1]))
        .then_with(|| a[2].total_cmp(&b[2]))
}

fn cmp4(a: &[f64; 4], b: &[f64; 4]) -> Ordering {
    a[0].total_cmp(&b[0])
        .then_with(|| a[1].total_cmp(&b[1]))
        .then_with(|| a[2].total_cmp(&b[2]))
        .then_with(|| a[3].total_cmp(&b[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_order(reverse: bool) -> WorldSnapshot {
        let mut snapshot = WorldSnapshot::new("replay");
        snapshot.tick = 7;
        let mut a = EntitySnapshot::new(1);
        a.tags.insert("static".into());
        let mut b = EntitySnapshot::new(2);
        b.tags.insert("dynamic".into());
        if reverse {
            snapshot.entities = vec![b, a];
        } else {
            snapshot.entities = vec![a, b];
        }
        snapshot.resources = vec![
            ResourceSnapshot {
                key: "z".into(),
                generation: 2,
            },
            ResourceSnapshot {
                key: "a".into(),
                generation: 1,
            },
        ];
        let collider_a = ColliderSnapshot::new(1, ColliderShape::Sphere { radius: 0.5 });
        let collider_b = ColliderSnapshot::new(
            2,
            ColliderShape::Box {
                half_extents: [1.0, 2.0, 3.0],
            },
        );
        if reverse {
            snapshot.colliders = vec![collider_b, collider_a];
        } else {
            snapshot.colliders = vec![collider_a, collider_b];
        }
        snapshot
    }

    #[test]
    fn canonical_hash_is_order_independent() {
        assert_eq!(
            snapshot_with_order(false).stable_hash(),
            snapshot_with_order(true).stable_hash()
        );
    }

    #[test]
    fn canonicalization_sorts_entities_and_resources() {
        let snapshot = snapshot_with_order(true).canonicalized();
        assert_eq!(snapshot.entities[0].id, 1);
        assert_eq!(snapshot.entities[1].id, 2);
        assert_eq!(snapshot.resources[0].key, "a");
        assert_eq!(snapshot.resources[1].key, "z");
        assert_eq!(snapshot.colliders[0].entity, 1);
        assert_eq!(snapshot.colliders[1].entity, 2);
    }
}
