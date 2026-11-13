//! 审计时间戳：写入/提取 Unix 毫秒（不参与哈希链，旧日志无 `ts` 仍可验链）。

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// 当前 Unix 毫秒。
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// 给一条记录 JSON 加 `ts`。
pub fn attach(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.insert("ts".to_string(), serde_json::json!(now_ms()));
    }
}

/// 按行提取 `ts`，空行忽略。
#[must_use]
pub fn extract(text: &str) -> Vec<Option<u64>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| value.get("ts").and_then(Value::as_u64))
        })
        .collect()
}
