use std::collections::BTreeMap;

use trit_orch::{Error, Receipt};

use super::super::measure;
use crate::cli_inputs::FinishInput;

/// 宿主逐条执行验收命令，把真实进程测出的回执写进映射。
///
/// `revision`、`TaskSpec` 与 `min_assertions` 全部从请求读出，执行器
/// 无法影响 plan 身份。
pub(super) fn host_measure(
    request: &FinishInput,
    receipts: &mut BTreeMap<String, Receipt>,
    nonce: &str,
) -> Result<(), Error> {
    for execution in &request.executions {
        let task_spec = request
            .tasks
            .iter()
            .find(|spec| spec.task.id == execution.task)
            .ok_or_else(|| Error::ReceiptUnknownTask {
                task: execution.task.clone(),
            })?;
        let receipt = measure::run_execution(
            execution,
            request.snapshot.revision,
            task_spec,
            request.min_assertions,
            nonce,
        )?;
        receipts.insert(execution.task.clone(), receipt);
    }
    Ok(())
}
