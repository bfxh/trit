//! 认领条数硬上限：防止无界输入把认领表撑爆。

use trit_orch::{ClaimStore, Error, MAX_CLAIMS};

#[test]
fn claim_store_has_a_hard_count_limit() {
    let mut store = ClaimStore::new();
    for index in 0..MAX_CLAIMS {
        store
            .claim("owner", &[format!("file-{index}")], "edit", 0, 1_000)
            .unwrap();
    }
    assert_eq!(
        store.claim("owner", &["one-more".into()], "edit", 0, 1_000),
        Err(Error::ClaimTooMany {
            count: MAX_CLAIMS,
            max: MAX_CLAIMS,
        }),
        "ORCH_CLAIM_LIMIT"
    );
}
