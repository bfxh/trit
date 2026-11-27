//! `--verify-c` 的路径边界：`--out` 用相对路径时，驱动器不能重复拼目录。
#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-direct-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

#[test]
fn verify_c_accepts_a_relative_out_dir() {
    let dir = scratch("verify-relative");
    fs::write(dir.join("v.tri"), "(a & b) | (a & c)\n").expect("写源文件");
    let output = Command::new(bin())
        .current_dir(&dir)
        .args(["v.tri", "--out", "v-out", "--verify-c"])
        .output()
        .expect("运行 trit-build");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let summary: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout 是 JSON");
    assert_eq!(summary["verified"]["c"]["rows"], 27, "{summary}");
    let _ = fs::remove_dir_all(&dir);
}
