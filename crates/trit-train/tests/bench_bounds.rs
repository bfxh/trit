//! `train-bench` 的尺寸上限：`rows×cols` 个 f32 与 `mtrits` 百万级 trit 都在分配前拒绝。
//!
//! `--rows 9000` 已超 `MAX_ROWS=8192`；用 `--cols 8` 让变异放行后的合法路径也能秒回。

use std::process::Command;

#[test]
fn absurd_rows_is_rejected_before_allocating() {
    let out = Command::new(env!("CARGO_BIN_EXE_train-bench"))
        .args([
            "--rows",
            "9000",
            "--cols",
            "8",
            "--rounds",
            "1",
            "--threads",
            "1",
            "--ste-steps",
            "0",
        ])
        .output()
        .expect("跑 train-bench");
    assert_eq!(out.status.code(), Some(2), "越界必须 rc=2");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--rows"),
        "实得: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
