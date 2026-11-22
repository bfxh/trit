//! Windows 认领路径大小写折叠。
//!
//! Windows 文件系统不区分大小写；`src/Foo.rs` 与 `src/foo.rs` 是同一个文件。
//! 认领表如果按字节比较，两个执行器可以各拿一条“不同路径”的认领同时写同一
//! 文件，单写者保证失效。Linux 上不跑这组（大小写敏感是正确语义）。

#![cfg(windows)]

use trit_orch::{ClaimOutcome, ClaimStore, Ternary};

fn claim(store: &mut ClaimStore, owner: &str, path: &str) -> ClaimOutcome {
    store
        .claim(owner, &[path.to_owned()], "edit", 0, 100)
        .expect("claim")
}

#[test]
fn windows_claim_paths_are_case_insensitive() {
    let mut store = ClaimStore::new();
    assert!(matches!(
        claim(&mut store, "alice", "src/Foo.rs"),
        ClaimOutcome::Acquired(_)
    ));

    // 另一个 owner 用不同大小写认领同一文件：门禁必须判 False，认领必须冲突。
    assert_eq!(
        store
            .gate(&["src/foo.rs".to_owned()], "bob", 0)
            .expect("gate"),
        Ternary::False
    );
    assert!(matches!(
        claim(&mut store, "bob", "src/foo.rs"),
        ClaimOutcome::Contested { .. }
    ));

    // 同一 owner 的不同大小写认领合并，而不是并存两条。
    assert!(matches!(
        claim(&mut store, "alice", "SRC/Foo.rs"),
        ClaimOutcome::Acquired(_)
    ));
    assert_eq!(store.len(), 1, "同一文件只应有一条认领");
}
