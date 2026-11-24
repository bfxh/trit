//! Adam 步的 `q` 符号掩码：量化值只用于 `dX`/`qdot` 的符号，先做阈值比较，
//! 只有落在阈值附近的 lane 才回退到原来的除法量化，保持逐位结果。
#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

/// 返回 `(q > 0, q < 0)` 的全 1 掩码。
///
/// `half_s`/`neg_half_s`/`margin` 由调用方按行预先算好。
///
/// # Safety
/// 需要 AVX2；`wv`/`sv` 是同一 8-lane 块。
#[inline]
#[target_feature(enable = "avx2")]
pub(super) unsafe fn masks(
    wv: __m256,
    sv: __m256,
    half_s: __m256,
    neg_half_s: __m256,
    margin: __m256,
) -> (__m256, __m256) {
    let sign = _mm256_set1_ps(-0.0);
    let abs_w = _mm256_andnot_ps(sign, wv);
    let diff = _mm256_andnot_ps(sign, _mm256_sub_ps(abs_w, half_s));
    let near = _mm256_or_ps(
        _mm256_cmp_ps(diff, margin, _CMP_LE_OQ),
        _mm256_cmp_ps(wv, wv, _CMP_UNORD_Q),
    );
    if _mm256_movemask_ps(near) == 0 {
        return (
            _mm256_cmp_ps(wv, half_s, _CMP_GE_OQ),
            _mm256_cmp_ps(wv, neg_half_s, _CMP_LE_OQ),
        );
    }
    exact(wv, sv)
}

/// 阈值附近：复刻原量化（除法 + 半远离零 + clamp + 整数比较）。
///
/// # Safety
/// 需要 AVX2；调用方保证输入是有限的 8-lane 向量。
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn exact(wv: __m256, sv: __m256) -> (__m256, __m256) {
    let half = _mm256_set1_ps(0.5);
    let one = _mm256_set1_ps(1.0);
    let neg_one = _mm256_set1_ps(-1.0);
    let sign = _mm256_set1_ps(-0.0);
    let izero = _mm256_setzero_si256();
    let scaled = _mm256_div_ps(wv, sv);
    let signed_half = _mm256_or_ps(_mm256_and_ps(scaled, sign), half);
    let rounded = _mm256_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(_mm256_add_ps(
        scaled,
        signed_half,
    ));
    let clamped = _mm256_min_ps(one, _mm256_max_ps(neg_one, rounded));
    let qi = _mm256_cvtps_epi32(clamped);
    (
        _mm256_castsi256_ps(_mm256_cmpgt_epi32(qi, izero)),
        _mm256_castsi256_ps(_mm256_cmpgt_epi32(izero, qi)),
    )
}
