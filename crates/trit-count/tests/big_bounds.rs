//! `--big` 的 max_n 上限：DPLL/BDD 按 n 放大，超过 40 直接拒绝（对齐 `--fuzz-cnf`）。
//! 用 `--cases 0` 让合法路径也快速返回，测试只关心退出码与文案。

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_trit-count"))
        .args(args)
        .output()
        .expect("跑 trit-count")
}

#[test]
fn big_above_the_cap_is_rejected() {
    let out = run(&["--big", "41", "--cases", "0"]);
    assert_eq!(out.status.code(), Some(2), "越界必须 rc=2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("上限"), "实得: {stderr}");
}

#[test]
fn big_at_the_cap_still_runs() {
    let out = run(&["--big", "40", "--cases", "0"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "40 必须放行: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
