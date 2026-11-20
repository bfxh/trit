//! `--depth-planes` 是位数（级数 `3^n - 1`），不是平面条数：越限的位数必须在
//! 入口被拒，底层的 `3^n` 与头部通道数不能静默绕回。

use trit_parts::planes::{MAX_PLANES, check, head_cout, levels, pow3};

#[test]
fn planes_above_the_cap_are_rejected() {
    assert!(check(MAX_PLANES).is_ok(), "上限本身必须合法");
    assert!(check(0).is_ok(), "0 表示不建深度头，必须合法");
    assert!(
        check(MAX_PLANES + 1).is_err(),
        "上限 +1 必须被拒，否则 3^13 级会在下游裸乘"
    );
    assert!(
        check(usize::MAX).is_err(),
        "usize::MAX 必须被拒，否则 `as u32` 截断后再乘 3 会绕回"
    );
}

#[test]
fn levels_of_small_n_are_pinned() {
    assert_eq!(levels(0), 0, "0 位没有可分的档");
    assert_eq!(levels(1), 2);
    assert_eq!(levels(2), 8);
    assert_eq!(levels(3), 26);
}

#[test]
fn levels_never_approach_usize_max_within_the_cap() {
    for n in 0..=MAX_PLANES {
        let l = levels(n);
        assert!(
            l < usize::MAX / 4,
            "3^{n} - 1 级不应接近 usize::MAX，实得 {l}"
        );
        assert_eq!(l, pow3(n).saturating_sub(1));
    }
}

#[test]
fn head_cout_does_not_wrap_at_the_old_overflow_point() {
    // 修复前：`n * 3` 在 u32 里绕成 u32::MAX，补到 4 的倍数那步再加 1 回到 0，
    // 深度头静默变成 0 通道——不报错不崩，只是训练出一个没有深度输出的网。
    let n = 1_431_655_765usize; // 0x5555_5555，n*3 = u32::MAX
    let c = head_cout(n);
    assert!(c > 0, "输出通道绕成了 {c}");
    assert!(c >= n.saturating_mul(3), "至少 n*3 个通道，实得 {c}");
}

#[test]
fn head_cout_is_a_multiple_of_four_within_the_cap() {
    for n in 1..=MAX_PLANES {
        let c = head_cout(n);
        assert!(c > 0, "n={n} 的输出通道是 {c}");
        assert!(
            c.is_multiple_of(4),
            "n={n} cout={c} 不是 4 的倍数，vec4 内核会读越界"
        );
    }
}

#[test]
fn pow3_matches_plain_power_within_the_cap() {
    for e in 0..=MAX_PLANES {
        assert_eq!(pow3(e), 3usize.pow(e as u32), "3^{e} 对不上");
    }
}

#[test]
fn depth_planes_rejects_the_level_before_touching_io() {
    // 越界的位数必须在读 `depth/*.npy` 之前就拒：目录是瞎编的，
    // 报的必须是位数超限，而不是「读不到文件」。
    let err = trit_parts::data::depth_planes(
        std::path::Path::new("/nonexistent/trit-planes-scene"),
        std::path::Path::new("/nonexistent/trit-planes-frame.png"),
        1,
        MAX_PLANES + 1,
        0.15,
        0.65,
    )
    .unwrap_err();
    assert!(err.contains("位数"), "应报位数超限，实得: {err}");
}

#[test]
fn levels_saturate_instead_of_panicking_above_the_cap() {
    // 位数越限时底层也不能裸 pow：`3usize.pow(64)` 在 debug 里 panic、release 里绕回
    // 一个「看起来合理」的级数。饱和后必然远大于上限内的 3^12。
    let l = levels(64);
    assert!(l >= 3usize.pow(MAX_PLANES as u32), "3^64 应饱和，实得 {l}");
}
