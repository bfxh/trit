//! `TernaryMemory::new` 的维数硬顶：`dim²` 个 i32 在分配前就该拒绝。
//!
//! `--dim` 是 CLI 参数，`new` 也是 pub API；`dim=100000` 是 37.25 GiB。

use trit_attn::{MAX_DIM, TernaryMemory};

#[test]
fn max_dim_is_legal() {
    assert_eq!(TernaryMemory::new(MAX_DIM).dim(), MAX_DIM);
    assert_eq!(TernaryMemory::new(1).dim(), 1);
}

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_dim_panics_before_allocating() {
    let _ = TernaryMemory::new(MAX_DIM + 1);
}

#[test]
#[should_panic(expected = "超过上限")]
fn zero_dim_panics() {
    let _ = TernaryMemory::new(0);
}
