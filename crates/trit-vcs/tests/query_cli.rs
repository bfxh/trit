//! 命令行端到端：按标签/证据过滤提交，接 techmap 反查条目与测试命令。

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
    let dir = std::env::temp_dir().join(format!("trit-vcs-query-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn where_tags_map_commit_to_tech_entries() {
    let root = temp("flow");
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(run(&repo, &["init"]).status.success());
    fs::write(repo.join("a.txt"), b"hello").unwrap();
    assert!(run(&repo, &["add", "a.txt"]).status.success());
    let committed = run(
        &repo,
        &[
            "commit",
            "-m",
            "add demo",
            "--owner",
            "qoder:it",
            "--allow-unclaimed",
            "--tag",
            "demo",
            "--tag",
            "vcs",
            "--evidence",
            "PR #900",
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));

    let logged = run(&repo, &["log", "--tag", "demo", "--json"]);
    assert!(logged.status.success(), "{}", stderr(&logged));
    let rows: serde_json::Value = serde_json::from_str(&stdout(&logged)).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let id = rows[0]["id"].as_str().unwrap().to_string();

    let none = run(&repo, &["log", "--tag", "gpu", "--json"]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout(&none)).unwrap(),
        serde_json::json!([])
    );

    let fixture = root.join("techmap.json");
    fs::write(
        &fixture,
        r#"{"entries":[{"id":"vcs-demo","title":"demo tech","tags":["demo"],
            "paths":["a.txt"],
            "tests":["crates/trit-vcs/tests/query_cli.rs::where_tags_map_commit_to_tech_entries"],
            "status":"landed"}]}"#,
    )
    .unwrap();
    let fixture_text = fixture.to_string_lossy().to_string();

    let where_hit = run(&repo, &["where", "a.txt", "--techmap", &fixture_text]);
    assert!(where_hit.status.success(), "{}", stderr(&where_hit));
    assert!(stdout(&where_hit).contains("vcs-demo"));

    let where_miss = run(
        &repo,
        &["where", "no-such-tech", "--techmap", &fixture_text],
    );
    assert!(!where_miss.status.success(), "没有命中应判失败");

    let tests = run(&repo, &["tests", "demo", "--techmap", &fixture_text]);
    assert!(tests.status.success(), "{}", stderr(&tests));
    assert_eq!(
        stdout(&tests).trim(),
        "cargo test -p trit-vcs where_tags_map_commit_to_tech_entries"
    );

    let explained = run(
        &repo,
        &["explain", &id, "--techmap", &fixture_text, "--json"],
    );
    assert!(explained.status.success(), "{}", stderr(&explained));
    let value: serde_json::Value = serde_json::from_str(&stdout(&explained)).unwrap();
    assert_eq!(value["tags"][1], "vcs");
    assert_eq!(value["entries"][0]["id"], "vcs-demo");
    assert_eq!(
        value["tests"][0],
        "cargo test -p trit-vcs where_tags_map_commit_to_tech_entries"
    );
    let _ = fs::remove_dir_all(&root);
}
