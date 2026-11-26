use crate::{Attempt, Effect, Error, Ledger, Selection, TaskSpec};
/// 派发闸门：每个被选中的任务都必须已有**同 epoch、Prepared** 的意图。
/// 约定：单效果任务用任务 ID 作为 operation ID；选中列表按 id 去重（保留首个、顺序稳定），重复 id 不透传为重复 Attempt；重复只来自外部构造的 Selection。
pub fn dispatch_ready(
    selection: &Selection,
    tasks: &[TaskSpec],
    ledger: &Ledger,
) -> Result<Vec<Attempt>, Error> {
    let mut ready = Vec::with_capacity(selection.selected.len());
    for id in &selection.selected {
        if ready.iter().any(|a: &Attempt| a.operation == *id) {
            continue;
        }
        let spec = tasks
            .binary_search_by(|spec| spec.task.id.as_str().cmp(id))
            .ok()
            .map(|position| &tasks[position])
            .ok_or_else(|| Error::NotPrepared {
                operation: id.clone(),
                reason: "任务不在计划里".into(),
            })?;
        let attempt = ledger.attempt(id).ok_or_else(|| Error::NotPrepared {
            operation: id.clone(),
            reason: "没有 prepare 记录".into(),
        })?;
        match ledger.state(id) {
            Some(Effect::Prepared) => {}
            other => {
                return Err(Error::NotPrepared {
                    operation: id.clone(),
                    reason: format!("状态是 {other:?}，不是 Prepared"),
                });
            }
        }
        if attempt.epoch != spec.epoch {
            return Err(Error::PreparedEpochMismatch {
                operation: id.clone(),
                prepared: attempt.epoch,
                expected: spec.epoch,
            });
        }
        ready.push(attempt.clone());
    }
    Ok(ready)
}
