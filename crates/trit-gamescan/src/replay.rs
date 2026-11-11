use crate::chaos::{Mutation, MutationError, MutationReceipt, Mutator};
use crate::snapshot::{ResourceSnapshot, WorldSnapshot};

#[derive(Clone, Debug)]
pub struct ReplayWorld {
    snapshot: WorldSnapshot,
}

impl ReplayWorld {
    pub fn new(snapshot: WorldSnapshot) -> Self {
        Self {
            snapshot: snapshot.canonicalized(),
        }
    }

    pub fn world(&self) -> &WorldSnapshot {
        &self.snapshot
    }
}

impl Mutator for ReplayWorld {
    fn snapshot(&self) -> WorldSnapshot {
        self.snapshot.clone().canonicalized()
    }

    fn load_snapshot(&mut self, snapshot: WorldSnapshot) {
        self.snapshot = snapshot.canonicalized();
    }

    fn apply(&mut self, mutation: &Mutation) -> Result<MutationReceipt, MutationError> {
        match mutation {
            Mutation::SetPosition { id, position } => {
                let entity = self
                    .snapshot
                    .find_mut(*id)
                    .ok_or(MutationError::EntityNotFound(*id))?;
                entity.transform.position = *position;
            }
            Mutation::SetRotation { id, rotation } => {
                let entity = self
                    .snapshot
                    .find_mut(*id)
                    .ok_or(MutationError::EntityNotFound(*id))?;
                entity.transform.rotation = *rotation;
            }
            Mutation::SetScale { id, scale } => {
                let entity = self
                    .snapshot
                    .find_mut(*id)
                    .ok_or(MutationError::EntityNotFound(*id))?;
                entity.transform.scale = *scale;
            }
            Mutation::SetParent { id, parent } => {
                let entity = self
                    .snapshot
                    .find_mut(*id)
                    .ok_or(MutationError::EntityNotFound(*id))?;
                entity.parent = *parent;
            }
            Mutation::SetEntityId { id, new_id } => {
                let entity = self
                    .snapshot
                    .find_mut(*id)
                    .ok_or(MutationError::EntityNotFound(*id))?;
                entity.id = *new_id;
            }
            Mutation::InjectEntity { entity } => {
                self.snapshot.entities.push(entity.clone());
            }
            Mutation::RemoveEntity { id } => {
                let index = self
                    .snapshot
                    .entities
                    .iter()
                    .position(|entity| entity.id == *id)
                    .ok_or(MutationError::EntityNotFound(*id))?;
                self.snapshot.entities.remove(index);
            }
            Mutation::SetResourceGeneration { key, generation } => {
                if let Some(resource) = self
                    .snapshot
                    .resources
                    .iter_mut()
                    .find(|resource| resource.key == *key)
                {
                    resource.generation = *generation;
                } else {
                    self.snapshot.resources.push(ResourceSnapshot {
                        key: key.clone(),
                        generation: *generation,
                    });
                }
            }
        }

        self.snapshot.canonicalize();
        Ok(MutationReceipt {
            sequence: 0,
            mutation: mutation.clone(),
        })
    }
}
