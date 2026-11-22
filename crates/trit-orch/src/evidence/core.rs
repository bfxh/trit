use serde::{Deserialize, Serialize};

use crate::{Ternary, TokenUsage};

#[path = "digest.rs"]
mod digest;
#[path = "receipt.rs"]
mod receipt;

pub use digest::test_files_digest;

/// 一次执行的结束方式。`Signalled`/`TimedOut`/`SpawnFailed` 都不是断言失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exit {
    Completed { code: i32 },
    Signalled,
    TimedOut,
    SpawnFailed,
}

/// 回执的来源。可信根只可能在宿主：执行器（及其背后的模型）永远可以自报，
/// 所以 `SelfReported` 在默认完成闸里一律拒绝。
///
/// - `HostMeasured`：宿主自己 spawn 执行器、自己读退出状态、自己解析输出里的
///   断言计数。执行器没有任何字段可以填写。
/// - `SelfReported`：调用方/执行器把数字写进 JSON。它只保留给「外部已经有独立
///   信任锚（例如宿主亲自验过签）」的显式场景，默认完成闸不采信。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptProvenance {
    HostMeasured,
    #[default]
    SelfReported,
}

/// 一次执行的回执。
///
/// 旧版本把 `assertions_passed/failed` 当作「受信 executor 返回」的字段，但同一份
/// 输入 JSON 既装 snapshot 又装 receipts 时，模型可以同时伪造两者。现在的结构
/// 把来源显式化：只有 [`Receipt::host_measured`] 构造的回执能过默认完成闸。
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
}

/// 验收规格：至少要通过多少条断言才算覆盖命题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpec {
    pub min_assertions: u32,
}
