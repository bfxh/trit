use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::add_tokens;
use crate::{Error, Resources, Task, Ternary};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub task: Task,
    pub requirements: Resources,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub workers: usize,
    pub capacity: Resources,
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Pending,
    InFlight,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckResult {
    Passed,
    Refuted,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReport {
    pub revision: u64,
    pub checks: u32,
    pub result: CheckResult,
}

impl CheckReport {
    pub fn verdict(&self, expected_revision: u64) -> Ternary {
        if self.checks == 0 || self.revision != expected_revision {
            return Ternary::Unknown;
        }
        match self.result {
            CheckResult::Passed => Ternary::True,
            CheckResult::Refuted => Ternary::False,
            CheckResult::Inconclusive => Ternary::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub fresh: u64,
    pub cached: u64,
    pub output: u64,
}

impl TokenUsage {
    pub fn total(&self) -> Result<u64, Error> {
        self.total_for(None)
    }

    pub(crate) fn total_for(&self, task: Option<&str>) -> Result<u64, Error> {
        add_tokens(
            add_tokens(self.fresh, self.cached, task)?,
            self.output,
            task,
        )
    }

    pub(crate) fn checked_add(self, other: Self, task: &str) -> Result<Self, Error> {
        Ok(Self {
            fresh: add_tokens(self.fresh, other.fresh, Some(task))?,
            cached: add_tokens(self.cached, other.cached, Some(task))?,
            output: add_tokens(self.output, other.output, Some(task))?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub phase: Phase,
    pub authorization: Ternary,
    pub freshness: Ternary,
    pub evidence: Option<CheckReport>,
    pub usage: TokenUsage,
}

impl Default for Observation {
    fn default() -> Self {
        Self {
            phase: Phase::Pending,
            authorization: Ternary::Unknown,
            freshness: Ternary::Unknown,
            evidence: None,
            usage: TokenUsage::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub observations: BTreeMap<String, Observation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockReason {
    Dependency {
        task: String,
        required: Ternary,
        verdict: Ternary,
    },
    Authorization {
        verdict: Ternary,
    },
    Freshness {
        verdict: Ternary,
    },
    ResourceCapacity {
        requirements: Resources,
        capacity: Resources,
    },
    Resources {
        requirements: Resources,
        available: Resources,
    },
    Workers {
        occupied: usize,
        limit: usize,
    },
    TokenCapacity {
        required: u64,
        limit: u64,
    },
    Tokens {
        required: u64,
        available: u64,
    },
    InFlight,
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub task: String,
    pub admission: Ternary,
    pub reasons: Vec<BlockReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub revision: u64,
    pub selected: Vec<String>,
    pub decisions: Vec<Decision>,
    pub observed_tokens: TokenUsage,
    pub reserved_tokens: u64,
    pub reserved_resources: Resources,
    pub occupied_workers: usize,
}
