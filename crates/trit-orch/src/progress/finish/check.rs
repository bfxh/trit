use crate::{Error, Receipt, Snapshot, TaskSpec, task_spec_digest};

/// 回执必须钉住当前计划：revision 与任务定义摘要逐项相等。
///
/// 这三种不符都归到 `ReceiptObservationMismatch`，用 `field` 区分
/// `plan` / `revision` / `task_digest`，不新增错误类型。
pub(super) fn plan(
    task: &str,
    receipt: &Receipt,
    snapshot: &Snapshot,
    spec: &TaskSpec,
) -> Result<(), Error> {
    let Some(plan) = receipt.plan.as_ref() else {
        return Err(Error::ReceiptObservationMismatch {
            task: task.to_owned(),
            field: "plan",
        });
    };
    if plan.revision != snapshot.revision {
        return Err(Error::ReceiptObservationMismatch {
            task: task.to_owned(),
            field: "revision",
        });
    }
    if plan.task_digest != task_spec_digest(spec)? {
        return Err(Error::ReceiptObservationMismatch {
            task: task.to_owned(),
            field: "task_digest",
        });
    }
    Ok(())
}

/// 回执里的 observation 证明必须存在且与快照一致；缺失按拒绝处理。
pub(super) fn observation<T: PartialEq + Copy>(
    task: &str,
    field: &'static str,
    claimed: Option<T>,
    actual: T,
) -> Result<(), Error> {
    match claimed {
        None => Err(Error::ReceiptObservationMissing {
            task: task.to_owned(),
            field,
        }),
        Some(value) if value == actual => Ok(()),
        Some(_) => Err(Error::ReceiptObservationMismatch {
            task: task.to_owned(),
            field,
        }),
    }
}
