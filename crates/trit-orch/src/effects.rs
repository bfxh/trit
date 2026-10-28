use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;

/// 外部副作用的生命周期。`Unknown` 表示「已经可能发出，但结果不明」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// 意图已落账，尚未发送。
    Prepared,
    /// 已发送，等结果。
    Sent,
    /// 外部已确认发生。
    Confirmed,
    /// 外部已确认未发生（可重试）。
    Refuted,
    /// 已发送或可能已发送，但没有可靠结果。
    Unknown,
}

/// 一次尝试的身份：谁在什么 epoch、用什么 fence 发起了哪个操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub operation: String,
    pub task: String,
    pub epoch: u64,
    pub attempt: u64,
    pub fence: u64,
}

impl Attempt {
    pub fn state_of(&self, ledger: &Ledger) -> Option<Effect> {
        ledger.state(&self.operation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    attempt: Attempt,
    state: Effect,
}

/// 外部是否真的发生了这次效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconcile {
    Present,
    Absent,
}

/// 副作用台账。规则：**结果不明不许盲重试**，先对账；fence 单调递增，旧执行器不能提交。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    records: BTreeMap<String, Record>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self, operation: &str) -> Option<Effect> {
        self.records.get(operation).map(|record| record.state)
    }

    pub fn attempt(&self, operation: &str) -> Option<&Attempt> {
        self.records.get(operation).map(|record| &record.attempt)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 是否允许为该操作发起新尝试。
    ///
    /// 只有「从未发起」或「已确认未发生（Refuted）」可以；`Prepared/Sent/Unknown/Confirmed` 都不行。
    pub fn may_retry(&self, operation: &str) -> bool {
        matches!(self.state(operation), None | Some(Effect::Refuted))
    }

    /// 落一条发送意图。已存在且未终结的操作必须先对账，不能靠再 prepare 绕过。
    pub fn prepare(
        &mut self,
        task: &str,
        epoch: u64,
        operation: &str,
        fence: u64,
    ) -> Result<Attempt, Error> {
        match self.records.get(operation) {
            None => {
                let attempt = Attempt {
                    operation: operation.to_owned(),
                    task: task.to_owned(),
                    epoch,
                    attempt: 1,
                    fence,
                };
                self.records.insert(
                    operation.to_owned(),
                    Record {
                        attempt: attempt.clone(),
                        state: Effect::Prepared,
                    },
                );
                Ok(attempt)
            }
            Some(record) => {
                fence_guard(operation, record, fence)?;
                match record.state {
                    Effect::Refuted => {
                        let attempt = Attempt {
                            operation: operation.to_owned(),
                            task: task.to_owned(),
                            epoch,
                            attempt: record.attempt.attempt.saturating_add(1),
                            fence,
                        };
                        self.records.insert(
                            operation.to_owned(),
                            Record {
                                attempt: attempt.clone(),
                                state: Effect::Prepared,
                            },
                        );
                        Ok(attempt)
                    }
                    Effect::Confirmed => Err(Error::EffectAlreadyApplied {
                        operation: operation.to_owned(),
                    }),
                    Effect::Prepared | Effect::Sent | Effect::Unknown => {
                        Err(Error::EffectInDoubt {
                            operation: operation.to_owned(),
                        })
                    }
                }
            }
        }
    }

    /// Prepared → Sent。
    pub fn mark_sent(&mut self, attempt: &Attempt, fence: u64) -> Result<(), Error> {
        self.transition(attempt, fence, Effect::Prepared, Effect::Sent)
    }

    /// 已发送但没拿到可靠结果：进入 `Unknown`，此后必须先对账。
    pub fn mark_unknown(&mut self, attempt: &Attempt) -> Result<(), Error> {
        let record = self.checked(attempt)?;
        match record.state {
            Effect::Prepared | Effect::Sent => {
                record.state = Effect::Unknown;
                Ok(())
            }
            other => Err(Error::InvalidTransition {
                operation: attempt.operation.clone(),
                from: format!("{other:?}"),
                to: "Unknown".into(),
            }),
        }
    }

    /// 拿到确定结果：Sent → Confirmed/Refuted。
    pub fn record_result(&mut self, attempt: &Attempt, happened: bool) -> Result<(), Error> {
        let target = if happened {
            Effect::Confirmed
        } else {
            Effect::Refuted
        };
        self.transition(attempt, attempt.fence, Effect::Sent, target)
    }

    /// 对账：只有 `Sent/Unknown` 需要对账。`Present` 说明效果已发生，`Absent` 说明没发生。
    pub fn reconcile(&mut self, attempt: &Attempt, observed: Reconcile) -> Result<Effect, Error> {
        let record = self.checked(attempt)?;
        match (record.state, observed) {
            (Effect::Sent | Effect::Unknown, Reconcile::Present) => {
                record.state = Effect::Confirmed;
                Ok(record.state)
            }
            (Effect::Sent | Effect::Unknown, Reconcile::Absent) => {
                record.state = Effect::Refuted;
                Ok(record.state)
            }
            (other, _) => Err(Error::InvalidTransition {
                operation: attempt.operation.clone(),
                from: format!("{other:?}"),
                to: format!("reconcile {observed:?}"),
            }),
        }
    }

    fn checked(&mut self, attempt: &Attempt) -> Result<&mut Record, Error> {
        let record =
            self.records
                .get_mut(&attempt.operation)
                .ok_or_else(|| Error::UnknownOperation {
                    operation: attempt.operation.clone(),
                })?;
        if record.attempt.attempt != attempt.attempt {
            return Err(Error::AttemptMismatch {
                operation: attempt.operation.clone(),
                given: attempt.attempt,
                current: record.attempt.attempt,
            });
        }
        Ok(record)
    }

    fn transition(
        &mut self,
        attempt: &Attempt,
        fence: u64,
        from: Effect,
        to: Effect,
    ) -> Result<(), Error> {
        let record = self.checked(attempt)?;
        fence_guard(&attempt.operation, record, fence)?;
        if record.state != from {
            return Err(Error::InvalidTransition {
                operation: attempt.operation.clone(),
                from: format!("{:?}", record.state),
                to: format!("{to:?}"),
            });
        }
        record.state = to;
        Ok(())
    }
}

/// 旧 fence 的执行器不能提交任何状态变化。
fn fence_guard(operation: &str, record: &Record, fence: u64) -> Result<(), Error> {
    if fence < record.attempt.fence {
        return Err(Error::StaleFence {
            operation: operation.to_owned(),
            given: fence,
            current: record.attempt.fence,
        });
    }
    Ok(())
}
