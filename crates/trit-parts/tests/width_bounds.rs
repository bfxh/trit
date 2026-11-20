//! `--width` 直接乘进通道数，通道数再乘进 `kk * kk * cin * cout` 定权重个数。
//!
//! `scaled_channels` 里的 `f32 as u32` 是饱和转换：`f32::INFINITY`（或任何让
//! `c × width` 超过 `u32::MAX` 的值）先取到 `u32::MAX`，`div_ceil(4) * 4` 再绕回
//! 0，`.max(4)` 把结果塌成 4 —— 不报错、不崩，只是整张网变成 4 通道。
//!
//! 校验放在 `width::check` 而不是 `Net::random_w` 里，是因为 `net` 整块挂在
//! `features = ["gpu"]` 下：写在那里的话，不装 GPU 的 CI 一次都跑不到。

use trit_parts::width::{MAX_WIDTH, check, scaled_channels};

/// 修前没有 `check`：`INFINITY` 一路走到 `scaled_channels` 变成 4。
#[test]
fn infinite_width_is_rejected_instead_of_collapsing_to_four_channels() {
    let e = check(f32::INFINITY).unwrap_err();
    assert!(e.contains("不在"), "{e}");
}

/// `NaN as u32` 是 0，同样落到 `.max(4)`。
#[test]
fn nan_width_is_rejected() {
    assert!(check(f32::NAN).is_err());
}

/// 负数 `as u32` 饱和成 0。
#[test]
fn negative_width_is_rejected() {
    assert!(check(-1.0).is_err());
}

/// `0.0` 让每一层都退化成 4 通道，等于换了一个网络。
#[test]
fn zero_width_is_rejected() {
    assert!(check(0.0).is_err());
}

/// 刚过上限：`4.5` 会按 576 通道分配（`9 × 576 × 576` 的权重 × 13 层）。
#[test]
fn width_above_the_cap_is_rejected() {
    let e = check(MAX_WIDTH + 0.5).unwrap_err();
    assert!(e.contains("MAX") || e.contains("4"), "{e}");
}

/// 正例：合法区间内的缩放结果要真的是 4 的倍数，且 `1.0` 是原宽度。
#[test]
fn legal_widths_scale_as_documented() {
    assert!(check(1.0).is_ok());
    assert!(check(MAX_WIDTH).is_ok());
    assert_eq!(scaled_channels(32, 1.0), 32);
    assert_eq!(scaled_channels(128, 1.0), 128);
    assert_eq!(scaled_channels(128, MAX_WIDTH), 512);
    assert_eq!(scaled_channels(32, 1.1), 36);
    for c in [32u32, 64, 128] {
        for w in [1.0f32, 1.5, 2.0, MAX_WIDTH] {
            assert_eq!(scaled_channels(c, w) % 4, 0, "c={c} w={w}");
        }
    }
}
