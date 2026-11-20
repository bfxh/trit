//! `res` 的平方：`u32` 里算会在 65536 处绕回 0 —— 分配出 0 个像素、结果全错，却不报错。
//!
//! 所有入口都必须先加宽到 `usize` 再乘，并由 `dims` 判 `MAX_PIXELS`。

use trit_parts::dims;

#[test]
fn the_wrapping_square_is_rejected() {
    // 65536² = 2^32 ≡ 0 (mod 2^32)：这正是 u32 里算出「0 个像素」的那个值
    assert!(dims::check(65536, 65536).is_err());
}

#[test]
#[should_panic(expected = "w×h 必须在")]
fn the_wrapping_square_panics_before_allocating() {
    let _ = dims::pixels(65536, 65536);
}

#[test]
fn the_biggest_square_that_still_fits() {
    assert_eq!(dims::pixels(8192, 8192), dims::MAX_PIXELS);
    assert!(dims::check(8193, 8193).is_err());
}

#[test]
#[should_panic(expected = "w×h 必须在")]
fn resizing_an_image_to_a_huge_square_panics() {
    // 入口：外部图片 → 目标边长。没有上限时这一行是一次 12 TB 的分配。
    let _ = trit_parts::data::resize_rgb(1, 1, &[0.0, 0.0, 0.0], 1 << 20);
}
