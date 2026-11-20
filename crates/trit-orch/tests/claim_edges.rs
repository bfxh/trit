//! 认领（claim）的边界：路径覆盖、上限、过期与 fencing。
//!
//! 认领是并发安全的唯一凭据。这里的每个边界都是"看起来没事，实际两个 agent 会同时改同一个文件"。

use trit_orch::{
    Claim, ClaimOutcome, ClaimState, ClaimStore, MAX_CLAIM_PATHS, Ternary, paths_overlap,
};

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|p| (*p).to_string()).collect()
}

/// 认领并解出 `Claim`；竞争（Contested）在这里就是失败，测试要显式看到它。
fn acquire(store: &mut ClaimStore, owner: &str, list: &[&str], now: u64, ttl: u64) -> Claim {
    match store
        .claim(owner, &paths(list), "intent", now, ttl)
        .expect("claim ok")
    {
        ClaimOutcome::Acquired(claim) => claim,
        ClaimOutcome::Contested { contenders, .. } => {
            panic!("期望拿到认领，实际被竞争者拿下：{contenders:?}")
        }
    }
}

#[test]
fn directory_prefix_covers_children_but_not_siblings_with_a_shared_prefix() {
    assert!(
        paths_overlap("crates/a", "crates/a/src/lib.rs"),
        "ORCH_CLAIM_DIR"
    );
    assert!(paths_overlap("crates/a/src/lib.rs", "crates/a"), "对称成立");
    assert!(
        !paths_overlap("crates/a", "crates/ab"),
        "ORCH_CLAIM_PREFIX_TRAP"
    );
    assert!(!paths_overlap("crates/a", "crates/ab/src/lib.rs"));
}

#[test]
fn identical_paths_overlap_with_themselves() {
    assert!(paths_overlap("a/b", "a/b"), "ORCH_CLAIM_SELF");
}

#[test]
fn a_trailing_slash_scope_still_covers_children() {
    // 路径有没有尾斜杠是宿主的事，认领必须两种都认，否则同一目录能开出两张互不冲突的锁。
    assert!(
        paths_overlap("crates/a/", "crates/a/src/lib.rs"),
        "ORCH_CLAIM_TRAILING"
    );
    assert!(
        paths_overlap("crates/a", "crates/a/"),
        "ORCH_CLAIM_TRAILING_EQUAL"
    );
}

#[test]
fn an_empty_scope_covers_nothing_and_cannot_be_claimed() {
    // 空字符串不能被解释成"整棵树"——那是把一次写错变成全仓独占锁。
    // 安全方向是：它不覆盖任何东西，而且认领层直接拒绝它。
    assert!(
        !paths_overlap("", "crates/a/src/lib.rs"),
        "ORCH_CLAIM_EMPTY_SCOPE"
    );
    let error = ClaimStore::new()
        .claim("a", &paths(&[""]), "i", 10, 100)
        .expect_err("blank path refused");
    assert_eq!(error.kind(), "claim_blank_path");
}

#[test]
fn the_first_owner_wins_and_the_second_is_contested() {
    let mut store = ClaimStore::new();
    let claim = acquire(&mut store, "a", &["crates/x"], 10, 100);
    assert_eq!(claim.fencing, 1);

    let outcome = store
        .claim("b", &paths(&["crates/x/src/lib.rs"]), "intent", 10, 100)
        .expect("contested is not an error");
    match outcome {
        ClaimOutcome::Contested { contenders, .. } => {
            assert_eq!(contenders, vec![claim.id.clone()], "ORCH_CLAIM_CONFLICT");
        }
        ClaimOutcome::Acquired(other) => panic!("不该拿到：{}", other.id),
    }
    assert_eq!(store.len(), 1, "竞争失败不能写进表里");
}

#[test]
fn disjoint_paths_are_not_a_conflict() {
    let mut store = ClaimStore::new();
    acquire(&mut store, "a", &["crates/x"], 10, 100);
    acquire(&mut store, "b", &["crates/y"], 10, 100);
    // crates/xy 与 crates/x 不构成前缀冲突（这是"前缀当边界"的老坑）。
    acquire(&mut store, "b", &["crates/xy"], 10, 100);
    assert_eq!(store.len(), 3, "ORCH_CLAIM_SIBLING");
}

#[test]
fn the_same_owner_merging_paths_bumps_fencing() {
    // 同一个 owner 扩大范围不是冲突，但必须换 fence：旧的写入凭据立刻失效。
    let mut store = ClaimStore::new();
    let first = acquire(&mut store, "a", &["crates/x"], 10, 100);
    let merged = acquire(&mut store, "a", &["crates/x/src/lib.rs"], 10, 100);
    assert_eq!(merged.id, first.id, "同一 owner 合并成一条");
    assert_eq!(
        merged.fencing,
        first.fencing + 1,
        "ORCH_CLAIM_MERGE_FENCING"
    );
    assert_eq!(merged.paths.len(), 2);
}

#[test]
fn an_expired_claim_is_expired_not_free() {
    // 过期不等于释放：别人可能还在跑。必须是 Unknown，逼调用方重新 claim。
    let mut store = ClaimStore::new();
    acquire(&mut store, "a", &["crates/x"], 10, 20);
    assert_eq!(
        store
            .state_of_paths(&paths(&["crates/x"]), 35)
            .expect("state"),
        ClaimState::Expired,
        "ORCH_CLAIM_EXPIRED"
    );
    assert_eq!(
        store.gate(&paths(&["crates/x"]), "b", 35).expect("gate"),
        Ternary::Unknown,
        "ORCH_CLAIM_EXPIRED_GATE"
    );
}

#[test]
fn an_owned_claim_gates_to_false_for_everyone_else() {
    let mut store = ClaimStore::new();
    acquire(&mut store, "a", &["crates/x"], 10, 100);
    assert_eq!(
        store.gate(&paths(&["crates/x"]), "b", 10).expect("gate"),
        Ternary::False,
        "ORCH_CLAIM_GATE_FALSE"
    );
    // 持有者自己必须能过，否则续期时会把自己拦在门外。
    assert_eq!(
        store.gate(&paths(&["crates/x"]), "a", 10).expect("gate"),
        Ternary::True,
        "ORCH_CLAIM_GATE_SELF"
    );
}

#[test]
fn free_paths_gate_to_unknown_not_true() {
    // 没人认领 = 还没有凭据，不是"已经批准"。真批准要走过 claim 才算。
    let store = ClaimStore::new();
    assert_eq!(
        store.gate(&paths(&["crates/x"]), "a", 10).expect("gate"),
        Ternary::Unknown,
        "ORCH_CLAIM_GATE_FREE"
    );
    assert_eq!(
        store
            .state_of_paths(&paths(&["crates/x"]), 10)
            .expect("state"),
        ClaimState::Free
    );
}

#[test]
fn a_released_claim_frees_the_paths_for_the_next_owner() {
    let mut store = ClaimStore::new();
    let claim = acquire(&mut store, "a", &["crates/x"], 10, 100);
    let released = store
        .release(&claim.id, "a", claim.fencing)
        .expect("release");
    assert_eq!(released.id, claim.id);
    // 释放是删除记录，不是把到期时间改到过去：状态回到 Free。
    assert_eq!(
        store
            .state_of_paths(&paths(&["crates/x"]), 11)
            .expect("state"),
        ClaimState::Free,
        "ORCH_CLAIM_RELEASE_FREE"
    );
    // 但 Free 仍不是 True：没有凭据不能算已批准。
    assert_eq!(
        store.gate(&paths(&["crates/x"]), "b", 11).expect("gate"),
        Ternary::Unknown,
        "ORCH_CLAIM_RELEASE_GATE"
    );
}

#[test]
fn releasing_with_a_wrong_owner_is_refused() {
    let mut store = ClaimStore::new();
    let claim = acquire(&mut store, "a", &["crates/x"], 10, 100);
    let error = store
        .release(&claim.id, "b", claim.fencing)
        .expect_err("not the owner");
    assert_eq!(error.kind(), "claim_not_owner", "ORCH_CLAIM_OWNER");
}

#[test]
fn renewing_bumps_fencing_so_late_writers_lose() {
    let mut store = ClaimStore::new();
    let claim = acquire(&mut store, "a", &["crates/x"], 10, 100);
    let renewed = store
        .renew(&claim.id, "a", claim.fencing, 10, 200)
        .expect("renew");
    assert_eq!(renewed.fencing, claim.fencing + 1, "ORCH_CLAIM_FENCING");
    let error = store
        .release(&claim.id, "a", claim.fencing)
        .expect_err("old fence is stale");
    assert_eq!(error.kind(), "stale_fence", "ORCH_CLAIM_STALE_FENCE");
}

#[test]
fn too_many_paths_are_refused_at_the_documented_bound() {
    let mut store = ClaimStore::new();
    let many: Vec<String> = (0..=MAX_CLAIM_PATHS).map(|i| format!("p/{i}")).collect();
    let error = store.claim("a", &many, "i", 10, 100).expect_err("too many");
    assert_eq!(error.kind(), "claim_too_many_paths", "ORCH_CLAIM_MAX");

    let exactly: Vec<String> = (0..MAX_CLAIM_PATHS).map(|i| format!("p/{i}")).collect();
    store
        .claim("a", &exactly, "i", 10, 100)
        .expect("ORCH_CLAIM_MAX_BOUNDARY");
}

#[test]
fn a_blank_owner_is_refused() {
    // 没有 owner 的锁无法释放，也无法追责。
    let error = ClaimStore::new()
        .claim("   ", &paths(&["crates/x"]), "i", 10, 100)
        .expect_err("blank owner");
    assert_eq!(error.kind(), "claim_blank_owner", "ORCH_CLAIM_BLANK_OWNER");
}

#[test]
fn a_zero_ttl_is_refused() {
    // ttl=0 的锁在落盘的同一刻就过期，等于没有锁。
    let error = ClaimStore::new()
        .claim("a", &paths(&["crates/x"]), "i", 10, 0)
        .expect_err("zero ttl");
    assert_eq!(error.kind(), "claim_zero_ttl", "ORCH_CLAIM_ZERO_TTL");
}

#[test]
fn restoring_a_store_rejects_blank_ids_and_keeps_valid_ones() {
    let claim = Claim {
        id: "c1".into(),
        owner: "a".into(),
        paths: paths(&["crates/x"]),
        intent: "i".into(),
        fencing: 1,
        acquired_at: 10,
        expires_at: 100,
    };
    let restored = ClaimStore::from_claims(vec![claim.clone()]).expect("restore");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored.get("c1"), Some(&claim), "ORCH_CLAIM_RESTORE");

    let mut blank = claim.clone();
    blank.id = "  ".into();
    let error = ClaimStore::from_claims(vec![blank]).expect_err("blank id");
    assert_eq!(error.kind(), "claim_id_blank", "ORCH_CLAIM_RESTORE_BLANK");
}
