use crate::digest;

use super::TraceEvent;

impl TraceEvent {
    /// 规范化 payload 字节：`payload_hash` / `prev_hash` 自身不参与计算。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut copy = self.clone();
        copy.payload_hash.clear();
        copy.prev_hash.clear();
        serde_json::to_vec(&copy).expect("trace event is always serializable")
    }

    /// 当前内容对应的 SHA-256 十六进制摘要。
    pub fn computed_payload_hash(&self) -> String {
        digest::hex(&digest::sha256(&self.canonical_bytes()))
    }

    /// 链接到前一条并封存本事件。`prev_hash` 为空表示创世。
    ///
    /// 封存后任何字段（包括 payload）再被改动，`computed_payload_hash` 都会
    /// 与 `payload_hash` 不一致；`TraceEventStore::load` 与 `check_trace` 都会
    /// 把它判红（fail-closed）。
    pub fn seal(&mut self, prev_hash: &str) {
        self.prev_hash = prev_hash.to_owned();
        self.payload_hash = self.computed_payload_hash();
    }
}
