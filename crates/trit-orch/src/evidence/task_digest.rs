use crate::digest;
use crate::{Error, TaskSpec};

/// 宿主计算任务定义摘要：对 `TaskSpec` 的规范 JSON 做 SHA-256。
///
/// 它固定「这次验收的是哪一份任务定义」；`TaskSpec` 任一字段（依赖、资源、
/// token 上限、epoch）变化都会换摘要。JSON 序列化失败按执行错误处理，不返回
/// 空摘要——空串会让「未绑定」伪装成「绑定过」。
pub fn task_spec_digest(task: &TaskSpec) -> Result<String, Error> {
    let bytes = serde_json::to_vec(task).map_err(|error| Error::ReceiptExecution {
        task: task.task.id.clone(),
        message: format!("task digest: {error}"),
    })?;
    Ok(digest::hex(&digest::sha256(&bytes)))
}
