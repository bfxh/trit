//! 程序 = 补丁 + 前置/后置行为条件。
//!
//! 这是“权重空间编程”的执行语义：程序要么**整体成功**（前置成立、补丁应用、
//! 后置成立），要么**原样回滚**。权重工件不会停在半改状态，因此可以对同一份
//! 权重反复试程序、批量审计。

use crate::edit::{Cost, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::{Behavior, Oracle};

/// 一个可执行、可回滚的权重编辑程序。
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    name: String,
    before: Option<Behavior>,
    patch: Patch,
    after: Option<Behavior>,
}

/// 一次执行的完整记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// 程序名。
    pub name: String,
    /// 补丁是否被应用过（前置不成立时为 `false`）。
    pub applied: bool,
    /// 后置不成立而回滚。
    pub rolled_back: bool,
    /// 前置条件是否成立。
    pub before_ok: bool,
    /// 应用后的行为是否满足后置条件。
    pub after_ok: bool,
    /// 执行前行为指纹。
    pub before_signature: u64,
    /// 补丁应用后、回滚前的行为指纹。
    pub changed_signature: u64,
    /// 最终行为指纹（回滚后等于 `before_signature`）。
    pub final_signature: u64,
    /// 声明的改动量。
    pub cost: Cost,
}

impl Program {
    /// 新建程序。
    #[must_use]
    pub fn new(name: impl Into<String>, patch: Patch) -> Self {
        Self {
            name: name.into(),
            before: None,
            patch,
            after: None,
        }
    }

    /// 要求执行前的行为必须等于 `behavior`。
    #[must_use]
    pub fn require_before(mut self, behavior: Behavior) -> Self {
        self.before = Some(behavior);
        self
    }

    /// 要求执行后的行为必须等于 `behavior`；不满足则回滚。
    #[must_use]
    pub fn require_after(mut self, behavior: Behavior) -> Self {
        self.after = Some(behavior);
        self
    }

    /// 程序名。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 补丁。
    #[must_use]
    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// 前置行为条件（若有）。
    #[must_use]
    pub fn before(&self) -> Option<&Behavior> {
        self.before.as_ref()
    }

    /// 后置行为条件（若有）。
    #[must_use]
    pub fn after(&self) -> Option<&Behavior> {
        self.after.as_ref()
    }

    /// 执行：前置检查 → 应用补丁 → 后置检查（失败回滚）。
    pub fn run(&self, net: &mut Net, oracle: &Oracle) -> Result<Report> {
        let before = Behavior::of(net, oracle)?;
        let before_signature = before.signature();
        let before_ok = self.before.as_ref().is_none_or(|want| want == &before);
        let cost = self.patch.cost(net);
        if !before_ok {
            return Ok(Report {
                name: self.name.clone(),
                applied: false,
                rolled_back: false,
                before_ok,
                after_ok: false,
                before_signature,
                changed_signature: before_signature,
                final_signature: before_signature,
                cost,
            });
        }

        let journal = self.patch.apply(net)?;
        let changed = Behavior::of(net, oracle)?;
        let changed_signature = changed.signature();
        let after_ok = self.after.as_ref().is_none_or(|want| want == &changed);
        let mut final_signature = changed_signature;
        if !after_ok {
            journal.undo(net);
            final_signature = Behavior::of(net, oracle)?.signature();
        }
        Ok(Report {
            name: self.name.clone(),
            applied: true,
            rolled_back: !after_ok,
            before_ok,
            after_ok,
            before_signature,
            changed_signature,
            final_signature,
            cost,
        })
    }
}

/// 组合脚本：单步、原子序列或按行为选择分支。
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub(crate) kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    Step(Program),
    Sequence {
        name: String,
        atomic: bool,
        steps: Vec<Script>,
    },
    When {
        name: String,
        expected: Behavior,
        then: Box<Script>,
        otherwise: Option<Box<Script>>,
    },
}

/// 脚本执行报告（树形）。
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptReport {
    /// 单步报告。
    Step(Report),
    /// 序列报告。
    Sequence {
        /// 名字。
        name: String,
        /// 失败时是否整体回滚。
        atomic: bool,
        /// 各步报告。
        steps: Vec<ScriptReport>,
        /// 本次是否发生整体回滚。
        rolled_back: bool,
        /// 结束后的行为指纹。
        final_signature: u64,
    },
    /// 条件分支报告。
    When {
        /// 名字。
        name: String,
        /// 条件是否成立（选了 `then`）。
        matched: bool,
        /// 实际执行的子脚本（没有分支时为 `None`）。
        chosen: Option<Box<ScriptReport>>,
        /// 结束后的行为指纹。
        final_signature: u64,
    },
}

impl ScriptReport {
    /// 结束后的行为指纹。
    #[must_use]
    pub fn final_signature(&self) -> u64 {
        match self {
            Self::Step(report) => report.final_signature,
            Self::Sequence {
                final_signature, ..
            }
            | Self::When {
                final_signature, ..
            } => *final_signature,
        }
    }

    /// 是否失败（单步未应用/回滚，或原子序列回滚，或选中分支失败）。
    #[must_use]
    pub fn failed(&self) -> bool {
        match self {
            Self::Step(report) => !report.applied || report.rolled_back,
            Self::Sequence { rolled_back, .. } => *rolled_back,
            Self::When { chosen, .. } => chosen.as_ref().is_some_and(|report| report.failed()),
        }
    }
}

impl Script {
    /// 单步脚本。
    #[must_use]
    pub fn step(program: Program) -> Self {
        Self {
            kind: Kind::Step(program),
        }
    }

    /// 原子序列：任一步失败/回滚，整段恢复原样。
    #[must_use]
    pub fn sequence(name: impl Into<String>, steps: Vec<Self>) -> Self {
        Self {
            kind: Kind::Sequence {
                name: name.into(),
                atomic: true,
                steps,
            },
        }
    }

    /// 设置序列失败时是否整体回滚（对非序列无影响）。
    #[must_use]
    pub fn atomic(mut self, atomic: bool) -> Self {
        if let Kind::Sequence {
            atomic: current, ..
        } = &mut self.kind
        {
            *current = atomic;
        }
        self
    }

    /// 条件分支：当前行为等于 `expected` 时执行 `then`，否则执行 `otherwise`（可选）。
    #[must_use]
    pub fn when(
        name: impl Into<String>,
        expected: Behavior,
        then: Self,
        otherwise: Option<Self>,
    ) -> Self {
        Self {
            kind: Kind::When {
                name: name.into(),
                expected,
                then: Box::new(then),
                otherwise: otherwise.map(Box::new),
            },
        }
    }

    /// 名字（单步用程序名）。
    #[must_use]
    pub fn name(&self) -> &str {
        match &self.kind {
            Kind::Step(program) => program.name(),
            Kind::Sequence { name, .. } | Kind::When { name, .. } => name,
        }
    }

    /// 执行脚本；序列按 `atomic` 决定失败后是否回滚已执行的步骤。
    pub fn run(&self, net: &mut Net, oracle: &Oracle) -> Result<ScriptReport> {
        match &self.kind {
            Kind::Step(program) => Ok(ScriptReport::Step(program.run(net, oracle)?)),
            Kind::Sequence {
                name,
                atomic,
                steps,
            } => {
                let snapshot = atomic.then(|| net.clone());
                let mut reports = Vec::with_capacity(steps.len());
                let mut failed = false;
                for step in steps {
                    let report = step.run(net, oracle)?;
                    failed = report.failed();
                    reports.push(report);
                    if failed {
                        break;
                    }
                }
                let rolled_back = failed && *atomic;
                if let (true, Some(snapshot)) = (rolled_back, snapshot) {
                    *net = snapshot;
                }
                Ok(ScriptReport::Sequence {
                    name: name.clone(),
                    atomic: *atomic,
                    steps: reports,
                    rolled_back,
                    final_signature: Behavior::of(net, oracle)?.signature(),
                })
            }
            Kind::When {
                name,
                expected,
                then,
                otherwise,
            } => {
                let matched = &Behavior::of(net, oracle)? == expected;
                let chosen = if matched {
                    Some(Box::new(then.run(net, oracle)?))
                } else if let Some(otherwise) = otherwise {
                    Some(Box::new(otherwise.run(net, oracle)?))
                } else {
                    None
                };
                Ok(ScriptReport::When {
                    name: name.clone(),
                    matched,
                    chosen,
                    final_signature: Behavior::of(net, oracle)?.signature(),
                })
            }
        }
    }
}
