//! `trit-dot bench --len` 的先验后分：三份 `Vec<i32>` + 打包位面都按 len 分配。
//!
//! `MAX_LEN = 1<<24`；测试值 16777217 只超一位，变异放行后也只会分配约 268 MB。

use std::process::Command;

#[test]
fn absurd_len_is_rejected_before_allocating() {
    let out = Command::new(env!("CARGO_BIN_EXE_trit-dot"))
        .args(["bench", "--len", "16777217", "--iters", "1"])
        .output()
        .expect("跑 trit-dot");
    assert_eq!(out.status.code(), Some(1), "越界必须 rc=1");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--len"),
        "实得: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
