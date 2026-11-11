//! 按 rustc 版本决定是否编译 AVX512 路径。
//!
//! `stdarch_x86_avx512` 在 1.88 仍是 nightly 特性（MSRV 上是硬编译错误），
//! 在 1.89+ 才稳定。这里读 `rustc --version` 发出 `has_avx512` cfg：
//! 1.88 构建自动走 POPCNT/标量路径，1.89+ 才编译 AVX512 派发。
//! 运行期是否真的走 AVX512 由 `is_x86_feature_detected!` 决定（本机无 AVX512 时自动降级）。

use std::process::Command;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_avx512)");
    println!("cargo:rerun-if-changed=build.rs");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let Ok(output) = Command::new(rustc).arg("--version").output() else {
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace().nth(1).unwrap_or("0.0").split('.');
    let major: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    if (major, minor) >= (1, 89) {
        println!("cargo:rustc-cfg=has_avx512");
    }
}
