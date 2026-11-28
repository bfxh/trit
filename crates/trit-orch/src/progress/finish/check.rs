use crate::{Error, Receipt, Snapshot, TaskSpec, acceptance_digest};

/// 回执必须钉住当前计划：revision 与验收身份摘要逐项相等；不符按
/// `field=plan|revision|acceptance_digest` 归到 `ReceiptObservationMismatch`。
pub(super) fn plan(
    task: &str,
    receipt: &Receipt,
    snapshot: &Snapshot,
    spec: &TaskSpec,
    min_assertions: u32,
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
    if plan.acceptance_digest != acceptance_digest(spec, min_assertions)? {
        return Err(Error::ReceiptObservationMismatch {
            task: task.to_owned(),
            field: "acceptance_digest",
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
