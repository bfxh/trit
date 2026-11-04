//! 命令行端到端：trit → Git 导出，再从 Git 导入另一个 trit 仓。

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
    let dir = std::env::temp_dir().join(format!("trit-vcs-gitcli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn export_and_import_through_the_cli() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("跳过：本机没有 git");
        return;
    }
    let root = temp("flow");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    assert!(run(&src, &["init"]).status.success());
    fs::write(src.join("a.txt"), b"hello").unwrap();
    assert!(run(&src, &["add", "a.txt"]).status.success());
    let committed = run(
        &src,
        &[
            "commit",
            "-m",
            "镜像一版",
            "--owner",
            "qoder:it",
            "--allow-unclaimed",
            "--tag",
            "vcs",
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));

    let git_dir = root.join("git");
    let git_text = git_dir.to_string_lossy().to_string();
    let map = root.join("map.json");
    let map_text = map.to_string_lossy().to_string();
    let exported = run(&src, &["git-export", &git_text, "--map", &map_text]);
    assert!(exported.status.success(), "{}", stderr(&exported));

    let log = Command::new("git")
        .args(["log", "--format=%s%n%b"])
        .current_dir(&git_dir)
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&log.stdout);
    assert!(log_text.contains("镜像一版"), "{log_text}");
    assert!(
        log_text.contains("Trit-Tags: vcs"),
        "导出提交应带 trit 尾注：{log_text}"
    );

    let dst = root.join("dst");
    fs::create_dir_all(&dst).unwrap();
    assert!(run(&dst, &["init"]).status.success());
    let dst_text = dst.to_string_lossy().to_string();
    let imported = run(
        &dst,
        &[
            "git-import",
            &git_text,
            "--repo",
            &dst_text,
            "--map",
            &map_text,
        ],
    );
    assert!(imported.status.success(), "{}", stderr(&imported));

    let logged = run(&dst, &["log", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&logged)).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["message"], "镜像一版");
    assert_eq!(rows[0]["tags"][0], "git-import");
    assert!(map.is_file(), "映射表应落盘");
    let _ = fs::remove_dir_all(&root);
}
