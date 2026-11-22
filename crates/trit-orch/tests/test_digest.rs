//! 测试工件摘要：路径排序稳定、内容变化敏感、路径逃逸拒绝。

use trit_orch::test_files_digest;

fn rel_path(tag: &str, name: &str) -> String {
    format!("target/test-digest-{tag}-{}/{}", std::process::id(), name)
}

#[test]
fn digest_binds_paths_and_content_and_rejects_escape() {
    let first = rel_path("bind", "a.txt");
    let second = rel_path("bind", "b.txt");
    let dir = std::path::Path::new(&first).parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&first, b"one").unwrap();
    std::fs::write(&second, b"two").unwrap();

    let forward = test_files_digest(&[first.clone(), second.clone()]).unwrap();
    let reverse = test_files_digest(&[second.clone(), first.clone()]).unwrap();
    assert_eq!(forward, reverse, "路径排序必须稳定");

    std::fs::write(&first, b"changed").unwrap();
    let changed = test_files_digest(&[first.clone(), second.clone()]).unwrap();
    assert_ne!(forward, changed, "内容变化必须改变摘要");

    assert!(
        test_files_digest(&["../escape".into()]).is_err(),
        "路径逃逸必须拒绝"
    );
    let _ = std::fs::remove_dir_all(dir);
}
