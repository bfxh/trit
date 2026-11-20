//! `Mlp::new` 的两个外部数：`nhid`（`--hid`）与 `nin * nhid`。
//!
//! `w1: (0..nin * nhid).map(...).collect()` 是裸乘 + 平方级分配；
//! `b1: vec![0.0; nhid]` 直接吃 `nhid`。三条 CLI 链共用这一个构造
//! （`occ --hid`、`texgen --hid`、`texgen2 --hid` 经 `PatchMlp`）。

use trit_parts::diffuse::PatchMlp;
use trit_parts::hidden::MAX_HIDDEN;
use trit_parts::occ::Mlp;

#[test]
fn hid_above_the_cap_is_rejected() {
    assert!(Mlp::new(4, MAX_HIDDEN, 7).is_ok(), "上限本身必须合法");
    let err = Mlp::new(4, MAX_HIDDEN + 1, 7).err().expect("越界必须 Err");
    assert!(err.contains("超过"), "应报越界，实得: {err}");
}

#[test]
fn hid_of_usize_max_is_rejected() {
    let err = Mlp::new(4, usize::MAX, 7)
        .err()
        .expect("usize::MAX 必须 Err");
    assert!(err.contains("超过"), "应报越界，实得: {err}");
}

#[test]
fn zero_hid_is_rejected() {
    assert!(Mlp::new(4, 0, 7).is_err(), "0 宽隐藏层没有意义");
}

#[test]
fn weight_matrix_overflow_is_rejected() {
    // `usize::MAX * 2` 溢出：debug 下 panic，release 下绕成 `usize::MAX - 1`，
    // 随后 `(0..n1).map(...).collect()` 直接 OOM——两种都不该发生。
    let err = Mlp::new(usize::MAX, 2, 7).err().expect("溢出必须 Err");
    assert!(err.contains("溢出"), "应报溢出，实得: {err}");
}

#[test]
fn legal_hid_builds() {
    let m = Mlp::new(6, 16, 3).expect("--hid 16 是默认值，必须合法");
    assert_eq!(m.nin, 6);
    assert_eq!(m.nhid, 16);
}

#[test]
fn patch_mlp_forwards_the_hid_check() {
    // `texgen2 --hid` 走这条路；`PatchMlp::new` 不许把 `Mlp` 的 Err 吞成 panic。
    let err = PatchMlp::new(3, 0, MAX_HIDDEN + 1, 5)
        .err()
        .expect("越界必须 Err");
    assert!(err.contains("超过"), "实得: {err}");
}
