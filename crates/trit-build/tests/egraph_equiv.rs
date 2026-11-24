//! 端到端回归：含 `≡`/`⊕` 的表达式在默认饱和下必须能构建，
//! 且真值表与关掉饱和（`--egraph-rounds 0`）逐字节一致。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const SOURCE: &str = "((a => b) <=> (!a | b)) ^ c";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-egraph-{}-{}-{}",
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

fn build(dir: &std::path::Path, name: &str, extra: &[&str]) -> (String, i32) {
    let path = dir.join(format!("{name}.tri"));
    fs::write(&path, format!("# regression\n{SOURCE}\n")).expect("写源文件");
    let out = dir.join(format!("{name}-out"));
    let mut args = vec![
        path.to_str().unwrap().to_string(),
        "--out".to_string(),
        out.to_str().unwrap().to_string(),
    ];
    args.extend(extra.iter().map(|item| item.to_string()));
    let output = Command::new(bin())
        .args(&args)
        .output()
        .expect("运行 trit-build");
    (
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn saturation_keeps_the_truth_table_of_equiv_xor_chain() {
    let dir = scratch("equiv-xor");
    let (stderr, code) = build(&dir, "saturated", &[]);
    assert_eq!(code, 0, "默认饱和必须能构建：{stderr}");
    let (stderr, code) = build(&dir, "raw", &["--egraph-rounds", "0"]);
    assert_eq!(code, 0, "关掉饱和必须能构建：{stderr}");

    let saturated = fs::read(dir.join("saturated-out/truth.csv")).expect("读 truth.csv");
    let raw = fs::read(dir.join("raw-out/truth.csv")).expect("读 truth.csv");
    assert_eq!(saturated, raw, "饱和后的真值表必须与不饱和逐字节一致");

    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.join("saturated-out/build.json")).expect("读 build.json"),
    )
    .expect("build.json 应是合法 JSON");
    assert_eq!(
        manifest["verified"]["equivalent"], true,
        "manifest={manifest}"
    );
    assert_eq!(
        manifest["verified"]["egraph"]["saturated"], true,
        "manifest={manifest}"
    );
}
