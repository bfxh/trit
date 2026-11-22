//! 可选的可信写盘时间：`with_stamped_time` 让 store 在封存前把 `recorded_at`
//! 覆盖成宿主真实时间，而不是采信调用方填的值。
//!
//! 默认关闭以保持旧事件语义与可复现测试；需要审计可信时间线时显式开启。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::TraceEvent;

use super::TraceEventStore;

impl TraceEventStore {
    /// 开启可信写盘时间：append 时用宿主当前时间覆盖 `recorded_at`。
    pub fn with_stamped_time(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            ecc: None,
            strict: false,
            anchor: None,
            stamp: true,
        }
    }

    pub(super) fn stamp_event(&self, event: &mut TraceEvent) {
        if self.stamp {
            event.recorded_at = now_secs();
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
