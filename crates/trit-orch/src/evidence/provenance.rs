use serde::{Deserialize, Serialize};

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
