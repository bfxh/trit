//! `--patch` 与 `PatchMlp::new`。
//!
//! `nin = patch² + cond + 1` 随后进 `Mlp::new` 的 `nin×nhid`。`patch²` 是裸乘，
//! 而 `Mlp` 那边的 `checked_mul` 只挡 usize 绕回——`--patch 65536` 给出 4.3e9 个特征，
//! 乘法干干净净地过，然后按 550 GB 去分配。

use trit_parts::diffuse::{MAX_PATCH, PatchMlp, check_patch};

#[test]
fn patch_above_the_cap_is_rejected() {
    assert!(
        PatchMlp::new(MAX_PATCH, 0, 8, 1).is_ok(),
        "上限本身必须合法"
    );
    let err = PatchMlp::new(MAX_PATCH + 2, 0, 8, 1)
        .err()
        .expect("越界必须 Err");
    assert!(err.contains("奇数"), "应报边长越界，实得: {err}");
}

#[test]
fn huge_patch_is_rejected_before_the_multiply() {
    // 65537 是奇数，只能在「上限」这一层被挡住：65537² 不溢出 usize，旧路径会一路算到 4.3e9 个特征再去分配。
    let err = check_patch(65537, 0).expect_err("65537 必须被拒");
    assert!(err.contains("奇数"), "实得: {err}");
}

#[test]
fn even_patch_is_rejected() {
    // 偶数 patch：`features` 的 `(2r+1)²` 比 `patch²` 多，多出来的被 `take(nin)` 静默丢掉。
    let err = PatchMlp::new(4, 0, 8, 1).err().expect("偶数必须 Err");
    assert!(err.contains("奇数"), "实得: {err}");
}

#[test]
fn zero_patch_is_rejected() {
    assert!(PatchMlp::new(0, 0, 8, 1).is_err(), "0 边长会得到空 patch");
}

#[test]
fn feature_dim_is_patch_square_plus_cond_plus_one() {
    assert_eq!(check_patch(3, 0), Ok(10));
    assert_eq!(check_patch(7, 2), Ok(52));
    let d = PatchMlp::new(3, 0, 8, 1).unwrap();
    assert_eq!(d.mlp.nin, 10);
}

#[test]
fn cap_is_the_documented_value() {
    assert_eq!(MAX_PATCH, 15);
}
