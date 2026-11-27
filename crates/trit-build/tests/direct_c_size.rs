//! `direct.c` 字节数回归门：只设上界，优化缩小时同步下调。
//!
//! 样例覆盖单用链、扇出、取反、否定对偶、11 变量、深链与全常量；预算来自当前 CLI 产物，任何变大都要先解释；生成物用 `/*K3 变量表*/` 单行头、无空行、无缩进函数体、`n<N?0:` 守卫、短参数名 `v/n`、单表 `T[]` 与 `3*l+r+4` 索引。
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-size-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}
fn build(dir: &Path, name: &str, source: &str) -> usize {
    let source_path = dir.join(format!("{name}.tri"));
    let out = dir.join(format!("{name}-out"));
    fs::write(&source_path, format!("{source}\n")).expect("写源文件");
    let output = Command::new(bin())
        .args([
            source_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--backend",
            "direct",
        ])
        .output()
        .expect("运行 trit-build");
    assert!(
        output.status.success(),
        "{source}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let c = fs::read_to_string(out.join("direct.c")).expect("读 direct.c");
    assert!(!c.contains("T_NOT"), "取反不应再发表：{source}\n{c}");
    fs::metadata(out.join("direct.c")).unwrap().len() as usize
}
#[test]
fn direct_c_bytes_stay_within_budget() {
    let fixtures = [
        ("(a & b) | c", 195),
        ("((a & b) | c) & (d | e)", 224),
        ("((a => b) <=> (!a | b)) ^ c", 235),
        (
            "(((a & b) | (c & d)) & ((e | f) ^ (g & h))) | !(i & (j | k))",
            334,
        ),
        ("a & (b | (c & (d | (e & f))))", 238),
        ("!a", 98),
        ("a & b", 161),
        ("a <=> 0", 98),
        ("(!a) => (!b)", 157),
        ("(!a) <=> (!b)", 158),
        ("(!(a & b)) => (!(c | d))", 210),
        ("1 & 0", 87),
    ];
    let dir = scratch();
    for (index, (source, budget)) in fixtures.into_iter().enumerate() {
        let bytes = build(&dir, &format!("e{}", index + 1), source);
        assert!(
            bytes <= budget,
            "direct.c 变大：{source} 生成 {bytes} B，超过预算 {budget} B"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
