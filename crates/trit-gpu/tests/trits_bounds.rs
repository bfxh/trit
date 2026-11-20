//! `trit-gpu bench --trits` 的规模上限：位平面与 GPU 缓冲都在分配前拒绝。
//!
//! 默认构建（无 gpu feature）也有 `bench` 桩，所以检查必须放在 `main` 的解析后，
//! 不能只写在 `#[cfg(feature = "gpu")]` 里。

use std::process::Command;

#[test]
fn absurd_trits_is_rejected_before_allocating() {
    let out = Command::new(env!("CARGO_BIN_EXE_trit-gpu"))
        .args(["bench", "--trits", "16777217"])
        .output()
        .expect("跑 trit-gpu");
    assert_eq!(out.status.code(), Some(1), "越界必须失败");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--trits"),
        "实得: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
