use std::collections::BTreeMap;

use super::super::measure;
use crate::cli_inputs::FinishInput;
use trit_orch::{Error, Receipt};
/// 宿主逐条执行验收命令；revision、TaskSpec 与有效阈值都从请求读出，执行器无法影响。
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
            task_spec
                .policy
                .min_assertions
                .max(request.min_assertions.unwrap_or(0)),
            nonce,
        )?;
        receipts.insert(execution.task.clone(), receipt);
    }
    Ok(())
}
