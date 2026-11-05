use super::*;
use crate::gate::ClaimSpec;
use crate::repo::CommitRequest;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-merge-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn commit(repo: &mut Repo, files: &[(&str, &str)], parent: Option<Hash>, now: u64) -> Hash {
    let map: BTreeMap<String, Vec<u8>> = files
        .iter()
        .map(|(path, body)| (path.to_string(), body.as_bytes().to_vec()))
        .collect();
    repo.stage(&map, now).unwrap();
    let paths: Vec<String> = map.keys().cloned().collect();
    repo.commit(CommitRequest {
        message: format!("c{now}"),
        session: "qoder:merge".to_string(),
        claims: vec![ClaimSpec {
            id: format!("c{now}"),
            owner: "qoder:merge".to_string(),
            paths,
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

fn merged_text(repo: &Repo, outcome: &MergeOutcome, path: &str) -> String {
    let tree = outcome.tree.expect("应无冲突");
    let entries = repo.store().tree_entries(&tree).unwrap();
    let id = entries.get(path).unwrap();
    String::from_utf8(repo.store().get_file(id).unwrap()).unwrap()
}

fn base_five() -> &'static str {
    "a\nb\nc\nd\ne"
}

#[test]
fn non_overlapping_edits_merge_clean() {
    let root = temp("clean");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(&mut repo, &[("f.txt", base_five())], None, 1_000);
    let ours = commit(&mut repo, &[("f.txt", "A\nb\nc\nd\ne")], Some(base), 1_001);
    let theirs = commit(&mut repo, &[("f.txt", "a\nb\nc\nd\nE")], Some(base), 1_002);

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.conflicts.is_empty());
    assert_eq!(
        merged_text(&repo, &outcome, "f.txt"),
        "A\nb\nc\nd\nE".to_string()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn identical_edits_merge_clean() {
    let root = temp("same");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(&mut repo, &[("f.txt", base_five())], None, 1_000);
    let ours = commit(&mut repo, &[("f.txt", "a\nB\nc\nd\ne")], Some(base), 1_001);
    let theirs = commit(&mut repo, &[("f.txt", "a\nB\nc\nd\ne")], Some(base), 1_002);

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.conflicts.is_empty());
    assert_eq!(
        merged_text(&repo, &outcome, "f.txt"),
        "a\nB\nc\nd\ne".to_string()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn formatting_only_difference_is_not_a_conflict() {
    let root = temp("format");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(&mut repo, &[("f.txt", base_five())], None, 1_000);
    // ours 只给第 2 行加了行尾空白；theirs 改第 5 行。
    let ours = commit(
        &mut repo,
        &[("f.txt", "a\nb   \nc\nd\ne")],
        Some(base),
        1_001,
    );
    let theirs = commit(&mut repo, &[("f.txt", "a\nb\nc\nd\nE")], Some(base), 1_002);

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.conflicts.is_empty(), "格式噪音不该变成冲突");
    assert_eq!(
        merged_text(&repo, &outcome, "f.txt"),
        "a\nb\nc\nd\nE".to_string(),
        "只保留真实改动"
    );

    // 两侧都把同一行改成只差行尾空白 ⇒ 归并成一份。
    let ours = commit(
        &mut repo,
        &[("f.txt", "a\nb  \nc\nd\ne")],
        Some(base),
        1_003,
    );
    let theirs = commit(
        &mut repo,
        &[("f.txt", "a\nb\t\nc\nd\ne")],
        Some(base),
        1_004,
    );
    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.conflicts.is_empty());
    assert_eq!(
        merged_text(&repo, &outcome, "f.txt"),
        "a\nb  \nc\nd\ne".to_string(),
        "两侧格式不同但语义相同 ⇒ 取一侧，不报冲突"
    );

    // 同一窗口只差空白，且别处还有真实改动 ⇒ 整文件规范化不等于 ⇒ 靠窗口级归一化归并。
    let ours = commit(
        &mut repo,
        &[("f.txt", "a\nX  \nc\nd\nD")],
        Some(base),
        1_005,
    );
    let theirs = commit(
        &mut repo,
        &[("f.txt", "a\nX\t\nc\nd\ne")],
        Some(base),
        1_006,
    );
    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.conflicts.is_empty(), "窗口级空白差异不该冲突");
    assert_eq!(
        merged_text(&repo, &outcome, "f.txt"),
        "a\nX  \nc\nd\nD".to_string()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn overlapping_edits_become_one_structured_conflict() {
    let root = temp("overlap");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(&mut repo, &[("f.txt", base_five())], None, 1_000);
    let ours = commit(
        &mut repo,
        &[("f.txt", "a\nb\nOURS\nd\ne")],
        Some(base),
        1_001,
    );
    let theirs = commit(
        &mut repo,
        &[("f.txt", "a\nb\nTHEIRS\nd\ne")],
        Some(base),
        1_002,
    );

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.tree.is_none(), "有冲突不给半棵树");
    assert_eq!(outcome.conflicts.len(), 1);
    let conflict = &outcome.conflicts[0];
    assert_eq!(conflict.path, "f.txt");
    assert_eq!(conflict.kind, ConflictKind::Content);
    assert_eq!(conflict.hunks.len(), 1);
    assert_eq!(conflict.hunks[0].base_lines, vec!["c".to_string()]);
    assert_eq!(conflict.hunks[0].ours, vec!["OURS".to_string()]);
    assert_eq!(conflict.hunks[0].theirs, vec!["THEIRS".to_string()]);
    let json = String::from_utf8(conflict.encode()).unwrap();
    assert!(!json.contains("<<<<<<<"), "不能退回冲突标记");
    assert!(json.contains("\"hunks\""));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn conflict_object_is_cheaper_than_reading_both_files() {
    let root = temp("cost");
    let mut repo = Repo::init(&root).unwrap();
    let base_text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let ours_text: String = (0..300)
        .map(|i| {
            if i == 150 {
                "ours changed\n".to_string()
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    let theirs_text: String = (0..300)
        .map(|i| {
            if i == 150 {
                "theirs changed\n".to_string()
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    let base = commit(&mut repo, &[("big.txt", &base_text)], None, 1_000);
    let ours = commit(&mut repo, &[("big.txt", &ours_text)], Some(base), 1_001);
    let theirs = commit(&mut repo, &[("big.txt", &theirs_text)], Some(base), 1_002);

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    let conflict = &outcome.conflicts[0];
    assert!(
        conflict.object_bytes() * 3 < conflict.raw_bytes,
        "冲突对象应远小于两份全文：object={} raw={}",
        conflict.object_bytes(),
        conflict.raw_bytes
    );
    assert_eq!(
        conflict.raw_bytes,
        (base_text.len() + ours_text.len() + theirs_text.len()) as u64
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn binary_files_are_conflicts_not_line_merges() {
    let root = temp("binary");
    let mut repo = Repo::init(&root).unwrap();
    let commit_bytes = |repo: &mut Repo, body: &[u8], parent: Option<Hash>, now: u64| -> Hash {
        let files: BTreeMap<String, Vec<u8>> = [("f.bin".to_string(), body.to_vec())].into();
        repo.stage(&files, now).unwrap();
        repo.commit(CommitRequest {
            message: format!("c{now}"),
            session: "qoder:merge".to_string(),
            claims: vec![ClaimSpec {
                id: format!("c{now}"),
                owner: "qoder:merge".to_string(),
                paths: vec!["f.bin".to_string()],
                acquired_at: 1,
                expires_at: u64::MAX / 2,
            }],
            parent_override: parent,
            now_secs: now,
            ..Default::default()
        })
        .unwrap()
        .id
    };
    let base = commit_bytes(&mut repo, b"a\0b\n", None, 1_000);
    let ours = commit_bytes(&mut repo, b"c\0b\n", Some(base), 1_001);
    let theirs = commit_bytes(&mut repo, b"a\0d\n", Some(base), 1_002);

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].kind, ConflictKind::Binary);
    assert!(outcome.conflicts[0].hunks.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn two_different_additions_are_structured_add_add() {
    let root = temp("add-add");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(&mut repo, &[("keep.txt", "k")], None, 1_000);
    let ours = commit(
        &mut repo,
        &[("keep.txt", "k"), ("f.txt", "from ours")],
        Some(base),
        1_001,
    );
    let theirs = commit(
        &mut repo,
        &[("keep.txt", "k"), ("f.txt", "from theirs")],
        Some(base),
        1_002,
    );
    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].kind, ConflictKind::AddAdd);
    assert!(outcome.tree.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn delete_on_one_side_only_is_clean() {
    let root = temp("delete");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(
        &mut repo,
        &[("f.txt", base_five()), ("keep.txt", "k")],
        None,
        1_000,
    );
    let ours = commit(&mut repo, &[("keep.txt", "k")], Some(base), 1_001);
    let theirs = commit(
        &mut repo,
        &[("f.txt", base_five()), ("keep.txt", "k")],
        Some(base),
        1_002,
    );

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert!(outcome.conflicts.is_empty());
    assert_eq!(outcome.removed_files, 1);
    let entries = repo.store().tree_entries(&outcome.tree.unwrap()).unwrap();
    assert!(!entries.contains_key("f.txt"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn modify_vs_delete_is_structured() {
    let root = temp("delete-modify");
    let mut repo = Repo::init(&root).unwrap();
    let base = commit(
        &mut repo,
        &[("f.txt", base_five()), ("keep.txt", "k")],
        None,
        1_000,
    );
    let ours = commit(&mut repo, &[("keep.txt", "k")], Some(base), 1_001);
    let theirs = commit(
        &mut repo,
        &[("f.txt", "a\nb\nc\nd\nE"), ("keep.txt", "k")],
        Some(base),
        1_002,
    );

    let outcome = merge(&repo, &ours, &theirs, None).unwrap();
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].kind, ConflictKind::DeleteModify);
    assert!(outcome.tree.is_none());
    let _ = std::fs::remove_dir_all(&root);
}
