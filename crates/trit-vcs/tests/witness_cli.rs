//! 命令行端到端：见证 → 提交 → 准入，以及改一个字节就被拒。

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
    let dir = std::env::temp_dir().join(format!("trit-vcs-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn witness_file(root: &Path) -> PathBuf {
    let dir = root.join(".tritvcs").join("witness");
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "应该只有一份见证：{entries:?}");
    entries.pop().unwrap()
}

#[test]
fn witness_commit_admit_roundtrip_and_tamper_refusal() {
    let root = temp("roundtrip");
    assert!(run(&root, &["init"]).status.success());
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    let added = run(&root, &["add", "a.txt"]);
    assert!(added.status.success(), "{}", stderr(&added));

    let witnessed = run(&root, &["witness", "--", "cargo", "--version"]);
    assert!(
        witnessed.status.success(),
        "witness 失败：{}",
        stderr(&witnessed)
    );
    let path = witness_file(&root);
    let path_text = path.to_string_lossy().to_string();

    let committed = run(
        &root,
        &[
            "commit",
            "-m",
            "witnessed",
            "--owner",
            "qoder:it",
            "--allow-unclaimed",
            "--witness",
            &path_text,
            "--goal",
            "见证往返",
            "--accept",
            "改一字节被拒",
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));

    let logged = run(&root, &["log", "--json"]);
    assert!(logged.status.success(), "{}", stderr(&logged));
    let rows: serde_json::Value = serde_json::from_str(&stdout(&logged)).unwrap();
    let id = rows[0]["id"].as_str().unwrap().to_string();
    assert!(
        rows[0]["witness"].is_string(),
        "提交里应记录见证摘要：{}",
        rows[0]
    );
    assert_eq!(rows[0]["intent"]["goal"], "见证往返");
    assert_eq!(rows[0]["intent"]["accept"][0], "改一字节被拒");
    assert!(
        rows[0]["intent"]["base"].is_null(),
        "第一条提交没有父树 ⇒ base=null：{}",
        rows[0]["intent"]
    );

    let admitted = run(&root, &["admit", &id, "--witness", &path_text]);
    assert!(admitted.status.success(), "{}", stderr(&admitted));

    // 只改见证文件里的一个字节 ⇒ 准入必须拒
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes
        .windows(9)
        .position(|window| window == b"trit-vcs/")
        .expect("见证里应有 tool 字段");
    bytes[at] = b'X';
    fs::write(&path, bytes).unwrap();
    let refused = run(&root, &["admit", &id, "--witness", &path_text]);
    assert!(!refused.status.success(), "改一个字节必须拒");
    assert!(
        stderr(&refused).contains("改动"),
        "错误里要说明是见证被改动：{}",
        stderr(&refused)
    );
    let _ = fs::remove_dir_all(&root);
}
