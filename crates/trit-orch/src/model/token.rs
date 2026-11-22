use serde::{Deserialize, Serialize};

use crate::Error;
use crate::error::add_tokens;

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
