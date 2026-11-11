use std::fmt;

use crate::snapshot::{EntityId, EntitySnapshot, WorldSnapshot};

#[derive(Clone, Debug, PartialEq)]
pub enum Mutation {
    SetPosition {
        id: EntityId,
        position: [f64; 3],
    },
    SetRotation {
        id: EntityId,
        rotation: [f64; 4],
    },
    SetScale {
        id: EntityId,
        scale: [f64; 3],
    },
    SetParent {
        id: EntityId,
        parent: Option<EntityId>,
    },
    SetEntityId {
        id: EntityId,
        new_id: EntityId,
    },
    InjectEntity {
        entity: EntitySnapshot,
    },
    RemoveEntity {
        id: EntityId,
    },
    SetResourceGeneration {
        key: String,
        generation: u64,
    },
}

impl Mutation {
    pub fn nan_position(id: EntityId) -> Self {
        Self::SetPosition {
            id,
            position: [f64::NAN, 0.0, 0.0],
        }
    }

    pub fn zero_scale(id: EntityId) -> Self {
        Self::SetScale {
            id,
            scale: [1.0, 0.0, 1.0],
        }
    }

    pub fn invalid_rotation(id: EntityId) -> Self {
        Self::SetRotation {
            id,
            rotation: [0.0, 0.0, 0.0, 0.0],
        }
    }

    pub fn dangling_parent(id: EntityId, parent: EntityId) -> Self {
        Self::SetParent {
            id,
            parent: Some(parent),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::SetPosition { id, position } => {
                format!("set position of entity {} to {:?}", id, position)
            }
            Self::SetRotation { id, rotation } => {
                format!("set rotation of entity {} to {:?}", id, rotation)
            }
            Self::SetScale { id, scale } => {
                format!("set scale of entity {} to {:?}", id, scale)
            }
            Self::SetParent { id, parent } => {
                format!("set parent of entity {} to {:?}", id, parent)
            }
            Self::SetEntityId { id, new_id } => {
                format!("set entity id {} to {}", id, new_id)
            }
            Self::InjectEntity { entity } => {
                format!("inject entity with id {}", entity.id)
            }
            Self::RemoveEntity { id } => format!("remove entity {}", id),
            Self::SetResourceGeneration { key, generation } => {
                format!("set resource {} generation to {}", key, generation)
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MutationPlan {
    pub seed: u64,
    pub mutations: Vec<Mutation>,
}

impl MutationPlan {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            mutations: Vec::new(),
        }
    }

    pub fn push(&mut self, mutation: Mutation) -> &mut Self {
        self.mutations.push(mutation);
        self
    }

    pub fn len(&self) -> usize {
        self.mutations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty()
    }

    pub fn apply_to<M: Mutator>(
        &self,
        target: &mut M,
    ) -> Result<Vec<MutationReceipt>, MutationError> {
        let mut receipts = Vec::with_capacity(self.mutations.len());
        for (index, mutation) in self.mutations.iter().enumerate() {
            let receipt = target.apply(mutation)?;
            receipts.push(MutationReceipt {
                sequence: index as u64 + 1,
                mutation: receipt.mutation,
            });
        }
        Ok(receipts)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationReceipt {
    pub sequence: u64,
    pub mutation: Mutation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationError {
    EntityNotFound(EntityId),
    ResourceNotFound(String),
    Internal(String),
}

impl fmt::Display for MutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityNotFound(id) => write!(f, "entity {} not found", id),
            Self::ResourceNotFound(key) => write!(f, "resource {} not found", key),
            Self::Internal(message) => write!(f, "mutation failed: {}", message),
        }
    }
}

impl std::error::Error for MutationError {}

pub trait Mutator {
    fn snapshot(&self) -> WorldSnapshot;

    fn load_snapshot(&mut self, snapshot: WorldSnapshot);

    fn apply(&mut self, mutation: &Mutation) -> Result<MutationReceipt, MutationError>;
}
