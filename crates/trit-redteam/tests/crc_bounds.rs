//! CRC-3 扫描的规模闸：`walk` 的枚举是 `C(n, w)·3^w`，n/w 大一点就组合爆炸。
//!
//! `trit-redteam` 是纯 bin，测试走 CLI：`--trits 257` 必须由 `run_crc` 的预检拒掉，
//! 错误文案带 `trits`（变异放行后会掉进 `scan` 的 assert，文案不含 `trits`）。

use std::process::Command;

#[test]
fn absurd_trits_is_rejected_before_scanning() {
    let out = Command::new(env!("CARGO_BIN_EXE_trit-redteam"))
        .args(["crc", "--trits", "257"])
        .output()
        .expect("跑 trit-redteam");
    assert!(!out.status.success(), "越界必须非零退出");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("trits"),
        "实得: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
