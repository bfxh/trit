//! `snn` CLI 的 ticks 前置校验：超大 ticks 不得先分配外部驱动向量。

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-machines") {
        return PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-machines{}", std::env::consts::EXE_SUFFIX))
}

#[test]
fn cli_snn_oversized_ticks_rejected_before_allocating() {
    let out = Command::new(bin())
        .args(["snn", "--size", "4", "--ticks", "100000000", "--drive", "1"])
        .output()
        .expect("failed to run trit-machines");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_ne!(out.status.code().unwrap_or(-1), 0, "stderr={stderr}");
    assert!(stderr.contains("ticks"), "{stderr}");
}
