//! `--backend direct|both` 的端到端判据：产物形状、与查表后端逐字节对拍、
//! 缓存键、Verilog 边界，以及 `--verify-c` 的 cc 编译对拍（Unix）。

use std::fs;
use std::path::{Path, PathBuf};
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

fn build(dir: &Path, name: &str, source: &str, extra: &[&str]) -> (String, String, i32) {
    let path = dir.join(format!("{name}.tri"));
    fs::write(&path, format!("{source}\n")).expect("写源文件");
    run(dir, name, &path, extra)
}

fn run(dir: &Path, name: &str, source: &Path, extra: &[&str]) -> (String, String, i32) {
    let out = dir.join(format!("{name}-out"));
    let mut args = vec![
        source.to_str().unwrap().to_string(),
        "--out".to_string(),
        out.to_str().unwrap().to_string(),
    ];
    args.extend(extra.iter().map(|item| item.to_string()));
    let output = Command::new(bin())
        .args(&args)
        .output()
        .expect("运行 trit-build");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

fn manifest(dir: &Path, name: &str) -> serde_json::Value {
    let text =
        fs::read_to_string(dir.join(format!("{name}-out/build.json"))).expect("读 build.json");
    serde_json::from_str(&text).expect("build.json 应是合法 JSON")
}

#[test]
fn direct_backend_writes_linear_c_and_skips_table() {
    let dir = scratch("direct");
    let (stdout, stderr, code) = build(
        &dir,
        "d",
        "((a => b) <=> (!a | b)) ^ c",
        &["--backend", "direct"],
    );
    assert_eq!(code, 0, "{stderr}");
    let summary: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout 是 JSON");
    assert_eq!(summary["options"]["backend"], "direct", "{summary}");
    assert!(summary["outputs"].get("direct.c").is_some(), "{summary}");
    assert!(summary["outputs"].get("lookup.c").is_none(), "{summary}");

    let source = fs::read_to_string(dir.join("d-out/direct.c")).expect("读 direct.c");
    assert!(
        source.contains("int8_t trit_direct(const int8_t *vars, unsigned nvars)"),
        "{source}"
    );
    assert!(
        !source.contains("TRIT_TABLE"),
        "直算不该带 3^n 表：{source}"
    );
    assert!(
        !dir.join("d-out/lookup.c").exists(),
        "direct 后端不该写 lookup.c"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn table_backend_remains_the_default() {
    let dir = scratch("default");
    let (_, stderr, code) = build(&dir, "t", "a & b | !c", &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(dir.join("t-out/lookup.c").exists());
    assert!(!dir.join("t-out/direct.c").exists());
    assert_eq!(manifest(&dir, "t")["options"]["backend"], "table");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn both_backends_agree_on_truth_table_and_blob() {
    let dir = scratch("both");
    let source = "(a ^ b) & (c | !d) | (a <=> d)";
    let (_, stderr, code) = build(&dir, "table", source, &["--backend", "table"]);
    assert_eq!(code, 0, "{stderr}");
    let (_, stderr, code) = build(&dir, "both", source, &["--backend", "both"]);
    assert_eq!(code, 0, "{stderr}");

    for name in ["truth.csv", "truth.tritbin"] {
        assert_eq!(
            fs::read(dir.join(format!("table-out/{name}"))).unwrap(),
            fs::read(dir.join(format!("both-out/{name}"))).unwrap(),
            "{name} 必须与后端无关"
        );
    }
    assert!(dir.join("both-out/lookup.c").exists());
    assert!(dir.join("both-out/direct.c").exists());
    assert_eq!(manifest(&dir, "both")["options"]["backend"], "both");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn backend_is_part_of_the_cache_key() {
    let dir = scratch("cache");
    let source = "a & b";
    let (first, _, _) = build(&dir, "cache", source, &[]);
    assert!(first.contains("\"cached\":false"), "{first}");
    let (second, _, _) = build(&dir, "cache", source, &[]);
    assert!(
        second.contains("\"cached\":true"),
        "同参数第二次应命中缓存：{second}"
    );
    let (third, _, _) = build(&dir, "cache", source, &["--backend", "direct"]);
    assert!(
        third.contains("\"cached\":false"),
        "换后端必须重建：{third}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn direct_backend_rejects_verilog_sources() {
    let dir = scratch("verilog");
    let path = dir.join("design.v");
    fs::write(
        &path,
        "module m(input a, input b, output y);\n  assign y = a & b;\nendmodule\n",
    )
    .expect("写 Verilog");
    let (_, stderr, code) = run(&dir, "v", &path, &["--backend", "direct"]);
    assert_ne!(code, 0, "Verilog + direct 必须报错，而不是静默少产物");
    assert!(
        stderr.contains("只支持 .tri") && stderr.contains("direct"),
        "错误信息要说清边界：{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn verify_c_compiles_and_matches_every_row() {
    let dir = scratch("verify");
    let (stdout, stderr, code) = build(
        &dir,
        "v",
        "((a => b) <=> (!a | b)) ^ c",
        &["--backend", "both", "--verify-c"],
    );
    assert_eq!(code, 0, "{stderr}");
    let summary: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout 是 JSON");
    let result = summary["verified"]["c"]["result"]
        .as_str()
        .expect("verified.c.result");
    assert!(result.starts_with("ok rows=27"), "{summary}");
    let _ = fs::remove_dir_all(&dir);
}
