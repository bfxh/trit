//! Adam `q` 符号掩码的逐位对拍，重点覆盖 `w/s` 的 ±0.5 边界。

fn around(v: f32, k: i32) -> f32 {
    f32::from_bits((v.to_bits() as i64 + k as i64) as u32)
}

#[test]
fn q_sign_masks_match_exact_near_threshold() {
    if !std::arch::is_x86_feature_detected!("avx2") {
        return;
    }
    for s in [0.01f32, 0.5, 1.0, f32::MIN_POSITIVE * 4.0, 0.02, 3.0] {
        let smax = s.max(f32::MIN_POSITIVE);
        let h = 0.5 * smax;
        let mut w = [0.0f32; 8];
        for k in -8i32..=8 {
            for (j, slot) in w.iter_mut().enumerate() {
                *slot = around(h, k + j as i32 - 4);
            }
            assert_masks(&w, s, "positive");
            for (j, slot) in w.iter_mut().enumerate() {
                *slot = around(-h, k + j as i32 - 4);
            }
            assert_masks(&w, s, "negative");
        }
        let mut state = 0x1234_5678u64;
        for _ in 0..64 {
            for slot in &mut w {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *slot = ((state % 4001) as f32 - 2000.0) / 500.0;
            }
            assert_masks(&w, s, "random");
        }
    }
}

fn assert_masks(w: &[f32; 8], s: f32, tag: &str) {
    use std::arch::x86_64::*;
    unsafe {
        let wv = _mm256_loadu_ps(w.as_ptr());
        let smax = s.max(f32::MIN_POSITIVE);
        let h = 0.5 * smax;
        let sv = _mm256_set1_ps(smax);
        let (p, n) = super::q_sign::masks(
            wv,
            sv,
            _mm256_set1_ps(h),
            _mm256_set1_ps(-h),
            _mm256_set1_ps((h * (1.0 / 4096.0)).max(f32::MIN_POSITIVE)),
        );
        // 旧量化是 `trunc(scaled + 0.5)`，加法本身会舍入，不能用 `round()` 当标量预期。
        let (ep, en) = super::q_sign::exact(wv, sv);
        assert_eq!(
            _mm256_movemask_ps(p),
            _mm256_movemask_ps(ep),
            "{tag} positive mask w={w:?} s={s}"
        );
        assert_eq!(
            _mm256_movemask_ps(n),
            _mm256_movemask_ps(en),
            "{tag} negative mask w={w:?} s={s}"
        );
    }
}
