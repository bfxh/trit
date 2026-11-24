//! Adam `q` 符号掩码的逐位对拍，重点覆盖 `w/s` 的 ±0.5 边界。

fn around(v: f32, k: i32) -> f32 {
    f32::from_bits((v.to_bits() as i64 + k as i64) as u32)
}

#[test]
fn q_sign_masks_match_scalar_near_threshold() {
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
        let (p, n) = super::q_sign::masks(
            wv,
            _mm256_set1_ps(smax),
            _mm256_set1_ps(h),
            _mm256_set1_ps(-h),
            _mm256_set1_ps((h * (1.0 / 4096.0)).max(f32::MIN_POSITIVE)),
        );
        let pm = _mm256_movemask_ps(p);
        let nm = _mm256_movemask_ps(n);
        for (j, &wj) in w.iter().enumerate() {
            let q = (wj / smax).round().clamp(-1.0, 1.0);
            assert_eq!(
                pm & (1 << j) != 0,
                q > 0.0,
                "{tag} positive lane {j}: w={wj} s={s}"
            );
            assert_eq!(
                nm & (1 << j) != 0,
                q < 0.0,
                "{tag} negative lane {j}: w={wj} s={s}"
            );
        }
    }
}
