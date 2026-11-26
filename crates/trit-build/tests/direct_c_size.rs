//! `direct.c` 字节数回归门：只设上界，优化缩小时同步下调。
//!
//! 样例覆盖单用链、扇出、取反、否定对偶、11 变量与深链；预算来自当前 CLI 产物，任何变大都要先解释；索引 3*l+r+4 每次少 14 B。
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
        ("(a & b) | c", 517),
        ("((a & b) | c) & (d | e)", 558),
        ("((a => b) <=> (!a | b)) ^ c", 622),
        (
            "(((a & b) | (c & d)) & ((e | f) ^ (g & h))) | !(i & (j | k))",
            753,
        ),
        ("a & (b | (c & (d | (e & f))))", 579),
        ("!a", 347),
        ("a & b", 434),
        ("a <=> 0", 347),
        ("(!a) => (!b)", 431),
        ("(!a) <=> (!b)", 435),
        ("(!(a & b)) => (!(c | d))", 540),
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
