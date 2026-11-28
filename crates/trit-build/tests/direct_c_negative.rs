#![cfg(unix)]
//! 负数 `n` 的 C 回归：守卫必须先返回 `X`，不能读 `v`。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-direct-negative-{}-{}",
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

fn build(dir: &Path) {
    let source = dir.join("neg.tri");
    fs::write(&source, "a & b\n").expect("写源文件");
    let out = dir.join("neg-out");
    let status = Command::new(bin())
        .args([
            source.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--backend",
            "direct",
        ])
        .status()
        .expect("运行 trit-build");
    assert!(status.success(), "trit-build 失败");
}

#[test]
fn negative_n_returns_x_without_reading_input() {
    let dir = scratch();
    build(&dir);
    let driver = dir.join("driver.c");
    fs::write(
        &driver,
        "#include <stdio.h>\n#include \"neg-out/direct.c\"\n\
         int main(void) { signed char r = f(NULL, -1); printf(\"%d\\n\", (int)r); return r == 0 ? 0 : 1; }\n",
    )
    .expect("写驱动");
    let compile = Command::new("cc")
        .args([
            "-std=c99", "-Wall", "-Wextra", "-Werror", "driver.c", "-o", "driver",
        ])
        .current_dir(&dir)
        .output()
        .expect("跑 cc");
    assert!(
        compile.status.success(),
        "编译失败：{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(dir.join("driver")).output().expect("跑驱动");
    assert!(run.status.success(), "负数 n 没有返回 X");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "0");
    let _ = fs::remove_dir_all(&dir);
}
