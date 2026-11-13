use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Error, Selection, TaskSpec};

/// 外部副作用的生命周期。`Unknown` 表示「已经可能发出，但结果不明」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// 意图已落账，尚未发送。只有这个状态允许真正发起调用。
    Prepared,
    /// 发送标记已落盘，副作用**可能**已经发生；重启后一律按在途处理。
    Sending,
    /// 调用已被受理，等结果。
    Sent,
    /// 外部已确认发生。
    Confirmed,
    /// 外部已确认未发生（可重试）。
    Refuted,
    /// 已发送或可能已发送，但没有可靠结果。
    Unknown,
}

/// 一个任务会产生的副作用声明。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectSpec {
    /// 台账操作 ID；空表示沿用任务 ID。
    #[serde(default)]
    pub operation: String,
}

impl EffectSpec {
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
        }
    }

    pub fn operation_for<'a>(&'a self, task: &'a str) -> &'a str {
        if self.operation.is_empty() {
            task
        } else {
            &self.operation
        }
    }
}

/// 任务声明的台账操作；未声明时回退到任务 ID（单效果默认）。
pub fn effect_operations(task: &TaskSpec) -> Vec<&str> {
    if task.effects.is_empty() {
        vec![task.task.id.as_str()]
    } else {
        task.effects
            .iter()
            .map(|effect| effect.operation_for(task.task.id.as_str()))
            .collect()
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
                    Effect::Prepared | Effect::Sending | Effect::Sent | Effect::Unknown => {
                        Err(Error::EffectInDoubt {
                            operation: operation.to_owned(),
                        })
                    }
                }
            }
        }
    }

    /// Prepared → Sending。**必须在真正发起外部调用之前落盘**：
    /// 崩溃后看到 Sending 就知道副作用可能已经发生。
    pub fn mark_sending(&mut self, attempt: &Attempt, fence: u64) -> Result<(), Error> {
        self.transition(attempt, fence, Effect::Prepared, Effect::Sending)
    }

    /// Sending → Sent：调用已被受理。
    pub fn mark_sent(&mut self, attempt: &Attempt, fence: u64) -> Result<(), Error> {
        self.transition(attempt, fence, Effect::Sending, Effect::Sent)
    }

    /// 已发送但没拿到可靠结果：进入 `Unknown`，此后必须先对账。
    pub fn mark_unknown(&mut self, attempt: &Attempt) -> Result<(), Error> {
        let record = self.checked(attempt)?;
        match record.state {
            Effect::Prepared | Effect::Sending | Effect::Sent => {
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
        let record = self.checked(attempt)?;
        if !matches!(record.state, Effect::Sending | Effect::Sent) {
            return Err(Error::InvalidTransition {
                operation: attempt.operation.clone(),
                from: format!("{:?}", record.state),
                to: format!("{target:?}"),
            });
        }
        record.state = target;
        Ok(())
    }

    /// 对账：只有 `Sent/Unknown` 需要对账。`Present` 说明效果已发生，`Absent` 说明没发生。
    pub fn reconcile(&mut self, attempt: &Attempt, observed: Reconcile) -> Result<Effect, Error> {
        let record = self.checked(attempt)?;
        match (record.state, observed) {
            (Effect::Sending | Effect::Sent | Effect::Unknown, Reconcile::Present) => {
                record.state = Effect::Confirmed;
                Ok(record.state)
            }
            (Effect::Sending | Effect::Sent | Effect::Unknown, Reconcile::Absent) => {
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

// ---------------------------------------------------------------- 持久化与派发闸门

use std::path::{Path, PathBuf};

/// 台账的 JSON 落盘。写临时文件再改名，避免半截文件被当成有效台账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerStore {
    path: PathBuf,
}

impl LedgerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读台账；文件不存在视为空台账，其余错误照实返回。
    pub fn load(&self) -> Result<Ledger, Error> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ledger::from_json(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::new()),
            Err(error) => Err(self.io(error)),
        }
    }

    /// 原子替换式保存。
    pub fn save(&self, ledger: &Ledger) -> Result<(), Error> {
        let temporary = self.temporary_path();
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| self.io(error))?;
        }
        std::fs::write(&temporary, ledger.to_json()).map_err(|error| self.io(error))?;
        match std::fs::rename(&temporary, &self.path) {
            Ok(()) => Ok(()),
            // Windows 不允许 rename 覆盖已存在文件；先删再改名，窗口期由 tmp 文件兜底。
            Err(_) => {
                let _ = std::fs::remove_file(&self.path);
                std::fs::rename(&temporary, &self.path).map_err(|error| {
                    let _ = std::fs::remove_file(&temporary);
                    self.io(error)
                })
            }
        }
    }

    /// 每次保存都用进程内唯一的中转名。
    ///
    /// 只由目标路径派生（`path.with_extension("tmp")`）时，两个指向同一台账文件的
    /// `LedgerStore` 会写同一个 tmp 再各自 rename：一方可能把另一方的内容搬进自己的
    /// 路径，或第二次 rename 因源文件已消失而报错。调用方仍须自己串行化同一台账
    /// （§1.1 L3 口径不变），这里只保证「不会因同名中转而串内容」。
    fn temporary_path(&self) -> PathBuf {
        static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stem = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ledger".into());
        self.path
            .with_file_name(format!("{stem}.{}.{}.tmp", std::process::id(), seq))
    }

    fn io(&self, error: std::io::Error) -> Error {
        Error::LedgerIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        }
    }
}

impl Ledger {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ledger is always serializable")
    }

    pub fn from_json(text: &str) -> Result<Self, Error> {
        serde_json::from_str(text).map_err(|error| Error::LedgerParse {
            message: error.to_string(),
        })
    }
}

/// 重启后的分类：只按**落盘事实**，不看内存里没保存的进度。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    /// 从未发出，可安全发起。
    pub ready: Vec<String>,
    /// 可能已经发出，必须先对账。
    pub in_doubt: Vec<String>,
    /// 已确认未发生，可重试。
    pub retryable: Vec<String>,
    /// 已确认发生。
    pub done: Vec<String>,
}

impl Ledger {
    /// 重启后的恢复分类；顺序按 operation ID 升序，保证可复现。
    pub fn recover(&self) -> Recovery {
        let mut recovery = Recovery::default();
        for (operation, record) in &self.records {
            match record.state {
                Effect::Prepared => recovery.ready.push(operation.clone()),
                Effect::Sending | Effect::Sent | Effect::Unknown => {
                    recovery.in_doubt.push(operation.clone());
                }
                Effect::Refuted => recovery.retryable.push(operation.clone()),
                Effect::Confirmed => recovery.done.push(operation.clone()),
            }
        }
        recovery
    }
}

/// 为一个任务的每个声明操作原子地落 `Prepared` 意图。
///
/// 先用同一个 fence 预检全部操作；任一操作不可重试、fence 过期或操作重复，
/// 都不写入任何意图，避免留下“半个任务已准备”的状态。
pub fn prepare_effects(
    ledger: &mut Ledger,
    spec: &TaskSpec,
    fence: u64,
) -> Result<Vec<Attempt>, Error> {
    let operations = effect_operations(spec);
    let mut seen = BTreeSet::new();
    for operation in &operations {
        if !seen.insert(*operation) {
            return Err(Error::DuplicateEffectOperation {
                task: spec.task.id.clone(),
                operation: (*operation).to_owned(),
            });
        }
        match ledger.state(operation) {
            None | Some(Effect::Refuted) => {}
            Some(Effect::Confirmed) => {
                return Err(Error::EffectAlreadyApplied {
                    operation: (*operation).to_owned(),
                });
            }
            Some(_) => {
                return Err(Error::EffectInDoubt {
                    operation: (*operation).to_owned(),
                });
            }
        }
        if let Some(attempt) = ledger.attempt(operation)
            && fence < attempt.fence
        {
            return Err(Error::StaleFence {
                operation: (*operation).to_owned(),
                given: fence,
                current: attempt.fence,
            });
        }
    }
    let mut attempts = Vec::with_capacity(operations.len());
    for operation in operations {
        attempts.push(ledger.prepare(&spec.task.id, spec.epoch, operation, fence)?);
    }
    Ok(attempts)
}

/// 派发闸门：每个被选中的任务，其声明的每个操作都必须已有**同 epoch、Prepared** 的意图。
///
/// 约定：未声明 `effects` 的任务沿用任务 ID 作为唯一 operation ID。
pub fn dispatch_ready(
    selection: &Selection,
    tasks: &[TaskSpec],
    ledger: &Ledger,
) -> Result<Vec<Attempt>, Error> {
    let mut ready = Vec::new();
    for id in &selection.selected {
        let spec = tasks
            .binary_search_by(|spec| spec.task.id.as_str().cmp(id))
            .ok()
            .map(|position| &tasks[position])
            .ok_or_else(|| Error::NotPrepared {
                operation: id.clone(),
                reason: "任务不在计划里".into(),
            })?;
        for operation in effect_operations(spec) {
            let attempt = ledger
                .attempt(operation)
                .ok_or_else(|| Error::NotPrepared {
                    operation: operation.to_owned(),
                    reason: "没有 prepare 记录".into(),
                })?;
            match ledger.state(operation) {
                Some(Effect::Prepared) => {}
                other => {
                    return Err(Error::NotPrepared {
                        operation: operation.to_owned(),
                        reason: format!("状态是 {other:?}，不是 Prepared"),
                    });
                }
            }
            if attempt.epoch != spec.epoch {
                return Err(Error::PreparedEpochMismatch {
                    operation: operation.to_owned(),
                    prepared: attempt.epoch,
                    expected: spec.epoch,
                });
            }
            ready.push(attempt.clone());
        }
    }
    Ok(ready)
}
