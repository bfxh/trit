use super::{intr_small, tail_visible};
use crate::splat::ScreenGaussian;

fn sg(u: f32, sigma: f32) -> ScreenGaussian {
    ScreenGaussian {
        u,
        v: 12.0,
        inv_a: 1.0 / (sigma * sigma),
        inv_b: 0.0,
        inv_c: 1.0 / (sigma * sigma),
        alpha: 1.0,
        depth: 5.0,
        color: [1.0, 0.0, 0.0],
    }
}

#[test]
fn offscreen_tiny_gaussian_is_culled() {
    assert!(!tail_visible(&sg(1000.0, 1.0), &intr_small()));
}

#[test]
fn offscreen_giant_with_visible_tail_is_kept() {
    assert!(tail_visible(&sg(1000.0, 1000.0), &intr_small()));
}

#[test]
fn on_screen_giant_is_kept() {
    assert!(tail_visible(&sg(16.0, 1000.0), &intr_small()));
}
