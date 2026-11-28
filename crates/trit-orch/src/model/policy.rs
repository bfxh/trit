use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

use crate::{EvidenceSpec, Resources, Task};

/// 一个任务的完整定义。`epoch` 是该定义自己的版本号：补丁替换定义时递增，
/// 旧 epoch 的执行与验收结果因此自动失效，与全局计划 revision 解耦。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub task: Task,
    pub requirements: Resources,
    pub max_tokens: u64,
    /// 版本与验收策略；`serde(flatten)` 保持旧 JSON 的 `epoch` 顶层字段。
    #[serde(flatten)]
    pub policy: TaskPolicy,
}

/// 任务自己的版本与验收下限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPolicy {
    #[serde(default)]
    pub epoch: u64,
    /// 该任务至少要有的断言数；全局 `EvidenceSpec` 只能抬高，不能压低。
    #[serde(default = "default_min_assertions")]
    pub min_assertions: u32,
}

pub(crate) const fn default_min_assertions() -> u32 {
    1
}

impl Default for TaskPolicy {
    fn default() -> Self {
        Self {
            epoch: 0,
            min_assertions: default_min_assertions(),
        }
    }
}

impl TaskPolicy {
    /// 与全局下限取较大者；调用方只能抬高任务自己的验收下限。
    pub(crate) fn evidence(&self, global: u32) -> EvidenceSpec {
        EvidenceSpec {
            min_assertions: self.min_assertions.max(global),
        }
    }
}

impl Deref for TaskSpec {
    type Target = TaskPolicy;

    fn deref(&self) -> &Self::Target {
        &self.policy
    }
}

impl DerefMut for TaskSpec {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.policy
    }
}
