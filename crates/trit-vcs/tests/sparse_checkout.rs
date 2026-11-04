//! 命令行端到端：稀疏检出（只物化一个目录）与范围内/外的 status。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-vcs")
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(root)
        .output()
        .expect("启动 trit-vcs 失败")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-sparse-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn sparse_checkout_materializes_one_directory_only() {
    let root = temp("flow");
    assert!(run(&root, &["init"]).status.success());
    for d in 0..3 {
        let dir = root.join(format!("gen/d{d}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..3 {
            fs::write(dir.join(format!("f{f}.txt")), format!("d{d} f{f}\n")).unwrap();
        }
    }
    let added = run(&root, &["add", "gen"]);
    assert!(added.status.success(), "{}", stderr(&added));
    let committed = run(
        &root,
        &[
            "commit",
            "-m",
            "base",
            "--owner",
            "qoder:it",
            "--allow-unclaimed",
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));
    let rows: serde_json::Value =
        serde_json::from_str(&stdout(&run(&root, &["log", "--json"]))).unwrap();
    let id = rows[0]["id"].as_str().unwrap().to_string();

    fs::remove_dir_all(root.join("gen")).unwrap();
    let sparse = run(&root, &["checkout", &id, "--prefix", "gen/d1"]);
    assert!(sparse.status.success(), "{}", stderr(&sparse));
    assert!(
        stdout(&sparse).contains("跳过 6"),
        "应报告跳过范围外文件：{}",
        stdout(&sparse)
    );
    assert!(root.join("gen/d1/f0.txt").is_file());
    assert!(!root.join("gen/d0").exists(), "范围外不该被物化");
    assert!(!root.join("gen/d2").exists());

    let scoped = run(&root, &["status", "--prefix", "gen/d1", "--json"]);
    assert!(scoped.status.success(), "{}", stderr(&scoped));
    let scoped_value = serde_json::from_str::<serde_json::Value>(&stdout(&scoped)).unwrap();
    assert_eq!(scoped_value["changes"], serde_json::json!([]));
    assert_eq!(scoped_value["prefix"], "gen/d1");
    let whole = run(&root, &["status", "--json"]);
    let whole_value = serde_json::from_str::<serde_json::Value>(&stdout(&whole)).unwrap();
    assert_eq!(
        whole_value["changes"].as_array().unwrap().len(),
        6,
        "范围外缺失如实报告"
    );
    let _ = fs::remove_dir_all(&root);
}
