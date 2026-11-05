//! 命令行端到端：导出 → 部分导入 → 截断后续传。

use std::fs;
use std::path::{Path, PathBuf};
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

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-tx-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn export_partial_import_and_resume() {
    let root = temp("flow");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    assert!(run(&src, &["init"]).status.success());
    for d in 0..12 {
        for f in 0..2 {
            let dir = src.join(format!("gen/d{d:03}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("f{f}.txt")),
                format!("dir {d} file {f}\n").repeat(8),
            )
            .unwrap();
        }
    }
    let added = run(&src, &["add", "gen"]);
    assert!(added.status.success(), "{}", stderr(&added));
    let committed = run(
        &src,
        &[
            "commit",
            "-m",
            "tx",
            "--owner",
            "qoder:it",
            "--allow-unclaimed",
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));

    let logged = run(&src, &["log", "--json"]);
    assert!(logged.status.success(), "{}", stderr(&logged));
    let rows: serde_json::Value = serde_json::from_str(&stdout(&logged)).unwrap();
    let tree = rows[0]["tree"].as_str().unwrap().to_string();

    let full = root.join("full.tvbd");
    let full_text = full.to_string_lossy().to_string();
    let exported = run(&src, &["export", &tree, "--out", &full_text]);
    assert!(exported.status.success(), "{}", stderr(&exported));

    let partial = root.join("part.tvbd");
    let partial_text = partial.to_string_lossy().to_string();
    let exported = run(
        &src,
        &[
            "export",
            &tree,
            "--out",
            &partial_text,
            "--prefix",
            "gen/d000",
        ],
    );
    assert!(exported.status.success(), "{}", stderr(&exported));
    assert!(
        fs::metadata(&partial).unwrap().len() * 10 < fs::metadata(&full).unwrap().len(),
        "单目录包应小于全量 1/10"
    );

    let dst = root.join("dst");
    fs::create_dir_all(&dst).unwrap();
    assert!(run(&dst, &["init"]).status.success());
    let dst_text = dst.to_string_lossy().to_string();

    // 截断 → 失败；再导全包 → 续传成功；第三次 → 全部跳过
    let bytes = fs::read(&full).unwrap();
    fs::write(&partial, &bytes[..bytes.len() / 2]).unwrap();
    let truncated = run(&dst, &["import", &partial_text, "--repo", &dst_text]);
    assert!(!truncated.status.success(), "截断的包必须判失败");

    let resumed = run(&dst, &["import", &full_text, "--repo", &dst_text]);
    assert!(resumed.status.success(), "{}", stderr(&resumed));
    assert!(
        stdout(&resumed).contains("跳过已有"),
        "续传应报告跳过数：{}",
        stdout(&resumed)
    );

    let repeated = run(&dst, &["import", &full_text, "--repo", &dst_text]);
    assert!(repeated.status.success(), "{}", stderr(&repeated));
    assert!(
        stdout(&repeated).contains("收到 0 个对象"),
        "重复导入不应再写：{}",
        stdout(&repeated)
    );
    let _ = fs::remove_dir_all(&root);
}
