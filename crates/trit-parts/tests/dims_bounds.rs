//! `w×h` 像素数上限：`initial_noise` / `predict_x0` / 条件 one-hot 图的公共根。
//!
//! 这些入口都是 pub API（`ddim_sample*`、`Denoiser::predict_x0`、`ddim_sample_eps`），
//! `w`/`h` 由调用方给出，以前一路裸乘进分配器。

use trit_parts::diffuse::{Denoiser, PatchMlp, Schedule, ddim_sample_opts};
use trit_parts::dims::{MAX_PIXELS, check};

#[test]
fn the_cap_itself_is_accepted() {
    assert!(check(1, 1).is_ok());
    assert!(check(MAX_PIXELS, 1).is_ok(), "上限本身必须合法");
    assert!(check(1, MAX_PIXELS).is_ok(), "行列可互换");
}

#[test]
fn one_above_the_cap_is_rejected() {
    let err = check(MAX_PIXELS + 1, 1).expect_err("越界必须 Err");
    assert!(err.contains("w×h"), "应报 w×h，实得: {err}");
}

#[test]
fn zero_pixels_is_rejected() {
    assert!(check(0, 0).is_err());
    // 0 行会让训练里取样的 `w*h - 1` 下溢。
    assert!(check(0, 7).is_err(), "0 行必须拒");
}

#[test]
fn overflow_is_rejected_before_multiplying() {
    // 乘法绕回成一个小数后，所有上限判断都会放行。
    assert!(check(usize::MAX / 2, 3).is_err(), "w*h 溢出必须拒");
}

#[test]
#[should_panic(expected = "w×h")]
fn sampling_a_huge_canvas_panics_before_allocating() {
    let d = PatchMlp::new(3, 0, 8, 1).unwrap();
    let _ = ddim_sample_opts(&d, &Schedule::cosine(8), None, 1 << 20, 1 << 20, 7, true);
}

#[test]
#[should_panic(expected = "w×h")]
fn predict_x0_on_a_huge_canvas_panics() {
    let d = PatchMlp::new(3, 0, 8, 1).unwrap();
    let _ = d.predict_x0(&[0.5], 0.5, None, 1 << 20, 1 << 20);
}

#[test]
#[should_panic(expected = "w×h")]
fn train_step_on_a_huge_canvas_panics() {
    let mut d = PatchMlp::new(3, 0, 8, 1).unwrap();
    let ab = Schedule::cosine(8).abar();
    let mut rng = 1u64;
    let _ = d.train_step(&[0.5], &ab, None, 1 << 20, 1 << 20, 0.1, &mut rng);
}

#[test]
#[should_panic(expected = "w×h")]
fn eps_sampler_on_a_huge_canvas_panics() {
    let net = trit_parts::tconv::Net::new(2, &[4], 1).unwrap();
    let ab = Schedule::cosine(4).abar();
    let _ = trit_parts::tconv::ddim_sample_eps(&net, &ab, 1 << 20, 1 << 20, 3, 1.0);
}
