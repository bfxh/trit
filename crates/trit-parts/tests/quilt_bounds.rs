//! `tex::quilt` 的输出尺寸：`out_w×out_h×3` 是 `out` / `placed` 两个 Vec 的根。
//!
//! 这是 pub API：`--size` / `--quilt-size` 之外还有别的调用方，守卫必须在函数里。

use trit_parts::tex::quilt;

fn src(n: usize) -> Vec<f32> {
    vec![0.5; n * n * 3]
}

#[test]
fn small_output_still_works() {
    let out = quilt(&src(8), 8, 8, 16, 16, 4, 1, 1, false, 0, false, false);
    assert_eq!(out.len(), 16 * 16 * 3, "输出应是 out_w×out_h×3");
}

#[test]
#[should_panic(expected = "w×h")]
fn huge_output_canvas_is_rejected_before_allocating() {
    let _ = quilt(&src(8), 8, 8, 8192, 8193, 4, 1, 1, false, 0, false, false);
}

#[test]
#[should_panic(expected = "patch")]
fn zero_output_canvas_is_rejected() {
    // `0` 在更早的 patch 断言就被挡住（`out_w >= patch`），到不了像素上限那一层。
    let _ = quilt(&src(8), 8, 8, 0, 16, 4, 1, 1, false, 0, false, false);
}
