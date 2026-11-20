//! `--size` 这类方形边长：`n*n*3` 是所有下游分配的根，必须先判上限。

use trit_parts::side::{MAX_SIDE, check};

#[test]
fn the_cap_itself_is_accepted() {
    assert!(check(MAX_SIDE).is_ok(), "上限本身必须合法");
    assert!(check(1).is_ok());
}

#[test]
fn one_above_the_cap_is_rejected() {
    let err = check(MAX_SIDE + 1).expect_err("越界必须 Err");
    assert!(err.contains("4096"), "应报上限，实得: {err}");
}

#[test]
fn zero_is_rejected() {
    // `0` 会让 `n*n` 全空、`seam_ratio` 除零。
    assert!(check(0).is_err());
}

#[test]
fn huge_is_rejected_without_overflow() {
    assert!(check(usize::MAX).is_err());
}

#[test]
fn cap_is_the_documented_value() {
    assert_eq!(MAX_SIDE, 4096);
}
