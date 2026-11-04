//! 命令行端到端：结构化争用对象（无冲突标记）与干净合并。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use trit_vcs::Hash;
use trit_vcs::gate::ClaimSpec;
use trit_vcs::repo::{CommitRequest, Repo};

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
    let dir =
        std::env::temp_dir().join(format!("trit-vcs-merge-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn commit(repo: &mut Repo, body: &str, parent: Option<Hash>, now: u64) -> Hash {
    let files: BTreeMap<String, Vec<u8>> = [("f.txt".to_string(), body.as_bytes().to_vec())].into();
    repo.stage(&files, now).unwrap();
    repo.commit(CommitRequest {
        message: format!("c{now}"),
        session: "qoder:merge-cli".to_string(),
        claims: vec![ClaimSpec {
            id: format!("c{now}"),
            owner: "qoder:merge-cli".to_string(),
            paths: vec!["f.txt".to_string()],
            acquired_at: 1,
            expires_at: u64::MAX / 2,
        }],
        parent_override: parent,
        now_secs: now,
        ..Default::default()
    })
    .unwrap()
    .id
}

#[test]
fn structured_conflict_has_no_markers_and_clean_merge_reports_tree() {
    let root = temp("flow");
    let repo_root = root.join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    let mut repo = Repo::init(&repo_root).unwrap();

    let big = |edit: &str| -> String {
        (0..300)
            .map(|i| {
                if i == 150 {
                    format!("{edit}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect()
    };
    let base_text = big("line 150");
    let base = commit(&mut repo, &base_text, None, 1_000);
    let ours = commit(&mut repo, &big("ours changed"), Some(base), 1_001);
    let theirs = commit(&mut repo, &big("theirs changed"), Some(base), 1_002);

    let repo_text = repo_root.to_string_lossy().to_string();
    let output = run(
        &repo_root,
        &[
            "merge",
            &ours.to_hex(),
            &theirs.to_hex(),
            "--json",
            "--repo",
            &repo_text,
        ],
    );
    assert!(!output.status.success(), "有冲突应判失败");
    let text = stdout(&output);
    assert!(!text.contains("<<<<<<<"), "不能退回冲突标记：{text}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let conflict = &value["conflicts"][0];
    assert_eq!(conflict["kind"], "content");
    assert_eq!(conflict["hunks"].as_array().unwrap().len(), 1);
    let object_bytes = conflict["object_bytes"].as_u64().unwrap();
    let raw_bytes = conflict["raw_bytes"].as_u64().unwrap();
    assert!(
        object_bytes * 3 < raw_bytes,
        "冲突对象应远小于全文：object={object_bytes} raw={raw_bytes}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn non_overlapping_edits_merge_clean_via_cli() {
    let root = temp("clean");
    let repo_root = root.join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    let mut repo = Repo::init(&repo_root).unwrap();
    let base = commit(&mut repo, "a\nb\nc\nd\ne", None, 1_000);
    let ours = commit(&mut repo, "A\nb\nc\nd\ne", Some(base), 1_001);
    let theirs = commit(&mut repo, "a\nb\nc\nd\nE", Some(base), 1_002);

    let repo_text = repo_root.to_string_lossy().to_string();
    let output = run(
        &repo_root,
        &[
            "merge",
            &ours.to_hex(),
            &theirs.to_hex(),
            "--repo",
            &repo_text,
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("合并干净"),
        "应报告干净合并：{}",
        stdout(&output)
    );
    let _ = std::fs::remove_dir_all(&root);
}
