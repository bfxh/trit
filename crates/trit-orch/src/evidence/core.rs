use serde::{Deserialize, Serialize};

use crate::{Ternary, TokenUsage};

#[path = "acceptance_digest.rs"]
pub(crate) mod acceptance_digest;
#[path = "digest.rs"]
mod digest;
#[path = "exit.rs"]
mod exit;
#[path = "plan.rs"]
mod plan;
#[path = "receipt.rs"]
mod receipt;
#[path = "receipt_ctor.rs"]
mod receipt_ctor;
#[path = "receipt_plan.rs"]
mod receipt_plan;
#[path = "task_digest.rs"]
mod task_digest;

pub use digest::test_files_digest;
pub use exit::Exit;
pub use plan::ReceiptPlan;
#[path = "provenance.rs"]
mod provenance;
pub use provenance::ReceiptProvenance;
pub use task_digest::task_spec_digest;

/// 一次执行的回执。旧版本把断言计数当作受信字段；现在只有
/// [`Receipt::host_measured`] 构造的回执能过默认完成闸。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub epoch: u64,
    pub exit: Exit,
    pub assertions_passed: u32,
    pub assertions_failed: u32,
    /// 来源；缺省按最不可信的 `SelfReported` 解析（旧 JSON 不会静默升级为可信）。
    #[serde(default)]
    pub provenance: ReceiptProvenance,
    /// 宿主为本次测量生成的挑战/去重标识。空串表示未绑定。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nonce: String,
    /// 宿主实际执行的 argv（审计用；不是 shell 字符串）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// 宿主对本次执行所属 observation 的授权判定；`None` 表示未提供，默认闸拒绝。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<Ternary>,
    /// 宿主对输入新鲜度的判定；`None` 表示未提供，默认闸拒绝。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Ternary>,
    /// 宿主记录的 token 用量；`None` 表示未提供，默认闸拒绝。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// 宿主对测试工件计算的摘要；`None` 表示本次没有绑定测试文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_digest: Option<String>,
    /// 宿主钉住的计划身份；`None` 表示未绑定，默认完成闸拒绝。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan: Option<ReceiptPlan>,
}

/// 验收规格：至少要通过多少条断言才算覆盖命题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpec {
    pub min_assertions: u32,
}
