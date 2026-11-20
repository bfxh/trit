//! `trit-codec stats/plan --trits` 的规模上限：`Vec<Ternary>` 按 trits 建（每项 1 字节）。
//!
//! 两处解析共用 `trits_arg`；`16777217` 只超 `MAX_TRITS = 1<<24` 一位。

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_trit-codec"))
        .args(args)
        .output()
        .expect("跑 trit-codec")
}

#[test]
fn stats_rejects_absurd_trits() {
    let out = run(&["stats", "--trits", "16777217"]);
    assert_eq!(out.status.code(), Some(1), "越界必须失败");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--trits"),
        "实得: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn plan_rejects_absurd_trits() {
    let out = run(&["plan", "--trits", "16777217"]);
    assert_eq!(out.status.code(), Some(1), "越界必须失败");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--trits"),
        "实得: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
