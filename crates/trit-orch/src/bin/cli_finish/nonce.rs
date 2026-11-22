use std::sync::atomic::{AtomicU64, Ordering};

/// 宿主本次调用的挑战。模型可以读到输入，但读不到调用开始后才生成的值，
/// 因此上一轮的真回执无法在本轮重放。
pub(super) fn host_nonce() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "host-{}-{}-{}",
        std::process::id(),
        now.as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}
