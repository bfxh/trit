use crate::error::add_tokens;
use crate::{
    BlockReason, Error, Limits, Observation, Phase, Resources, TaskSpec, Ternary, TokenUsage,
};

#[derive(Default)]
pub(crate) struct Budget {
    pub observed: TokenUsage,
    pub reserved_tokens: u64,
    pub resources: Resources,
    pub workers: usize,
    observed_total: u64,
}

impl Budget {
    pub fn from_snapshot(
        tasks: &[TaskSpec],
        observations: &[&Observation],
        limits: &Limits,
    ) -> Result<Self, Error> {
        let mut budget = Self::default();
        for (spec, observation) in tasks.iter().zip(observations) {
            let id = spec.task.id.as_str();
            let used = observation.usage.total_for(Some(id))?;
            let remaining =
                spec.max_tokens
                    .checked_sub(used)
                    .ok_or_else(|| Error::TaskTokenLimitExceeded {
                        task: id.to_owned(),
                        used,
                        max_tokens: spec.max_tokens,
                    })?;
            budget.observed = budget.observed.checked_add(observation.usage, id)?;
            if observation.phase == Phase::InFlight {
                budget.reserve(spec, remaining)?;
            }
        }
        budget.observed_total = budget.observed.total()?;
        budget.validate(limits)?;
        Ok(budget)
    }

    fn validate(&self, limits: &Limits) -> Result<(), Error> {
        if self.workers > limits.workers {
            return Err(Error::WorkerLimitExceeded {
                occupied: self.workers,
                workers: limits.workers,
            });
        }
        if self.resources.cpu > limits.capacity.cpu
            || self.resources.memory > limits.capacity.memory
        {
            return Err(self.resource_limit_error(limits));
        }
        if add_tokens(self.observed_total, self.reserved_tokens, None)? > limits.tokens {
            return Err(self.token_limit_error(limits));
        }
        Ok(())
    }

    pub fn reserve(&mut self, spec: &TaskSpec, tokens: u64) -> Result<(), Error> {
        let resources = self
            .resources
            .checked_add(spec.requirements)
            .ok_or_else(|| Error::ResourceOverflow {
                task: spec.task.id.clone(),
                reserved: self.resources,
                additional: spec.requirements,
            })?;
        let reserved_tokens = add_tokens(self.reserved_tokens, tokens, Some(&spec.task.id))?;
        let workers = self
            .workers
            .checked_add(1)
            .ok_or_else(|| Error::WorkerOverflow {
                task: spec.task.id.clone(),
                occupied: self.workers,
            })?;
        self.resources = resources;
        self.reserved_tokens = reserved_tokens;
        self.workers = workers;
        Ok(())
    }

    pub fn gate(
        &self,
        spec: &TaskSpec,
        limits: &Limits,
        reasons: &mut Vec<BlockReason>,
    ) -> Result<Ternary, Error> {
        let (available, tokens) = self.available(limits)?;
        let mut gate = Ternary::True;
        if spec.requirements.cpu > limits.capacity.cpu
            || spec.requirements.memory > limits.capacity.memory
        {
            reasons.push(BlockReason::ResourceCapacity {
                requirements: spec.requirements,
                capacity: limits.capacity,
            });
            gate = gate & Ternary::False;
        } else if spec.requirements.cpu > available.cpu
            || spec.requirements.memory > available.memory
        {
            reasons.push(BlockReason::Resources {
                requirements: spec.requirements,
                available,
            });
            gate = gate & Ternary::Unknown;
        }
        if self.workers >= limits.workers {
            reasons.push(BlockReason::Workers {
                occupied: self.workers,
                limit: limits.workers,
            });
            gate = gate & Ternary::Unknown;
        }
        if spec.max_tokens > limits.tokens {
            reasons.push(BlockReason::TokenCapacity {
                required: spec.max_tokens,
                limit: limits.tokens,
            });
            gate = gate & Ternary::False;
        } else if spec.max_tokens > tokens {
            reasons.push(BlockReason::Tokens {
                required: spec.max_tokens,
                available: tokens,
            });
            gate = gate & Ternary::Unknown;
        }
        Ok(gate)
    }

    fn available(&self, limits: &Limits) -> Result<(Resources, u64), Error> {
        let cpu = limits
            .capacity
            .cpu
            .checked_sub(self.resources.cpu)
            .ok_or_else(|| self.resource_limit_error(limits))?;
        let memory = limits
            .capacity
            .memory
            .checked_sub(self.resources.memory)
            .ok_or_else(|| self.resource_limit_error(limits))?;
        let tokens = limits
            .tokens
            .checked_sub(self.observed_total)
            .and_then(|remaining| remaining.checked_sub(self.reserved_tokens))
            .ok_or_else(|| self.token_limit_error(limits))?;
        Ok((Resources { cpu, memory }, tokens))
    }

    fn resource_limit_error(&self, limits: &Limits) -> Error {
        Error::ResourceLimitExceeded {
            reserved: self.resources,
            capacity: limits.capacity,
        }
    }

    fn token_limit_error(&self, limits: &Limits) -> Error {
        Error::TokenLimitExceeded {
            observed: self.observed_total,
            reserved: self.reserved_tokens,
            limit: limits.tokens,
        }
    }
}
