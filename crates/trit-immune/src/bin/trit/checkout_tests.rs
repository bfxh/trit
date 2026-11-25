//! `checkout` 视图构造回归（审计 F39-1 / #619）。

use super::*;
use std::fs;

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trit-checkout-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn s(v: impl Into<String>) -> String {
    v.into()
}

#[test]
fn checkout_view_does_not_duplicate_root() {
    let dir = tmpdir("root");
    let ledger = dir.join(DEFAULT_LEDGER);
    run(&[s("init"), s(dir.to_string_lossy().to_string())]).unwrap();
    run(&[
        s("branch"),
        s("登录模块"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();

    let j = Journal::open_as(&ledger, 0).unwrap();
    let dag = j.dag().clone();
    drop(j.into_dag().unwrap());

    // tip=0：账本仅根节点，视图必须只有 1 个节点；根仍是根，不被错挂成合成根的子节点。
    let v0 = build_checkout_view(&dag, 0).unwrap();
    assert_eq!(v0.nodes().len(), 1, "视图不得多出合成根节点");
    assert!(v0.nodes()[0].parent.is_none());
    assert_eq!(v0.nodes()[0].title, dag.nodes()[0].title);
    assert_eq!(v0.nodes()[0].verdict, dag.nodes()[0].verdict);

    // tip=1：视图 2 节点；#1 的父是 #0，编号与账本 1:1 对齐（不再整体后移）。
    let v1 = build_checkout_view(&dag, 1).unwrap();
    assert_eq!(v1.nodes().len(), 2);
    assert_eq!(v1.nodes()[1].parent, Some(NodeId(0)));
    assert_eq!(v1.nodes()[1].title, dag.nodes()[1].title);
    assert_eq!(v1.nodes()[1].verdict, dag.nodes()[1].verdict);
}

#[test]
fn checkout_view_preserves_history_and_non_ascii_title() {
    let dir = tmpdir("hist");
    let ledger = dir.join(DEFAULT_LEDGER);
    run(&[s("init"), s(dir.to_string_lossy().to_string())]).unwrap();
    run(&[
        s("branch"),
        s("分支-安全模块"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[
        s("commit"),
        s("-m"),
        s("回归通过"),
        s("-v"),
        s("+1"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();

    let j = Journal::open_as(&ledger, 0).unwrap();
    let dag = j.dag().clone();
    drop(j.into_dag().unwrap());

    let v = build_checkout_view(&dag, 1).unwrap();
    let child = &v.nodes()[1];
    assert_eq!(child.title, dag.nodes()[1].title);
    assert!(child.title.contains("分支-安全模块"));
    assert!(child.history.iter().any(|e| e.note.contains("回归通过")));
    assert_eq!(child.verdict, Trit::Hardened);
}
