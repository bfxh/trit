use crate::digest;
use crate::{Error, TaskSpec, task_spec_digest};

/// 宿主计算验收身份摘要：任务定义摘要 + 本次采用的最低断言数。
///
/// plan 只钉任务定义还不够：同一份定义可以先用严阈值测量、再换成宽阈值
/// 完成。把阈值一起折进摘要，换阈值就会和回执里的 `acceptance_digest`
/// 对不上，完成闸按 `acceptance_digest` 不符拒绝。
pub fn acceptance_digest(task: &TaskSpec, min_assertions: u32) -> Result<String, Error> {
    let task_digest = task_spec_digest(task)?;
    let mut input = Vec::with_capacity(task_digest.len() + 5);
    input.extend_from_slice(task_digest.as_bytes());
    input.push(b':');
    input.extend_from_slice(&min_assertions.to_le_bytes());
    Ok(digest::hex(&digest::sha256(&input)))
}
