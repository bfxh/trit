//! `snn --ticks` 的先验后分：`vec![drive; ticks]` 在 `simulate_with_drive` 之前就分配。
//!
//! `MAX_TICKS` 的闸原来在 `run()` 里，CLI 先按 ticks 建外部驱动序列——顺序反了。

use std::process::Command;

#[test]
fn absurd_ticks_is_rejected_before_allocating() {
    let out = Command::new(env!("CARGO_BIN_EXE_trit-machines"))
        .args(["snn", "--ticks", "1000001", "--drive", "1", "--size", "4"])
        .output()
        .expect("跑 trit-machines");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "越界必须非零退出");
    assert!(
        stderr.contains("--ticks"),
        "应在 CLI 分配前拒绝，实得: {stderr}"
    );
}
