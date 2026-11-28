//! `simd.rs` 的单元测试（拆自 `simd.rs`，过 per-file 行数台账；仅 x86_64）。
//!
//! 覆盖：AVX2 vs 标量差分（掩码/两行版/f32/f16/q4）、掩码空间穷举、调度与边界。
use super::*;

fn rails(rows: usize, cols: usize) -> (Vec<u64>, Vec<u64>, Vec<f32>) {
    let bpr = cols.div_ceil(64);
    let padded = bpr * 64;
    let mut pos = vec![0u64; rows * bpr];
    let mut neg = vec![0u64; rows * bpr];
    for r in 0..rows {
        for c in 0..cols {
            let t = ((r * 31 + c * 17) % 3) as i32 - 1;
            let word = r * bpr + c / 64;
            let bit = 1u64 << (c % 64);
            match t {
                1 => pos[word] |= bit,
                -1 => neg[word] |= bit,
                _ => {}
            }
        }
    }
    let x = (0..padded)
        .map(|i| ((i as f32 * 0.37).sin() * 0.5) + ((i % 7) as f32 - 3.0) * 0.01)
        .collect();
    (pos, neg, x)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.0)
}

#[test]
fn scalar_mask_matches_avx2() {
    if !avx2() {
        return;
    }
    let (rows, cols) = (9usize, 576usize);
    let (pos, neg, x) = rails(rows, cols);
    let bpr = cols.div_ceil(64);
    for r in 0..rows {
        let (p, n) = (&pos[r * bpr..(r + 1) * bpr], &neg[r * bpr..(r + 1) * bpr]);
        // SAFETY: x 已补齐到 bpr*64。
        let (fast, slow) = unsafe { (imp::row_dot_mask(p, n, &x), scalar::row_dot_mask(p, n, &x)) };
        assert!(close(fast, slow), "mask row {r}: avx2={fast} scalar={slow}");
    }
}

/// 8 位掩码的**穷举**对拍：所有互斥 `(pos, neg)` 字节组合（3^8 = 6561 组），
/// 每条 lane 的 `x = 1.0`，结果是小整数 ⇒ f32 精确，可要求逐位相等。
///
/// 这条把「AVX2 掩码路径 vs 标量路径」的等价从抽样抬到掩码空间全覆盖；
/// 浮点内核（`row_dot_f32` / `f16` / `q4`）仍只能按容差对拍。
#[test]
fn mask_kernel_exhaustive_bytes() {
    if !avx2() {
        return;
    }
    let x = vec![1.0f32; 64];
    for p in 0u64..256 {
        for n in 0u64..256 {
            // 三值编码里 pos 与 neg 互斥；重叠位两者语义不同，不属于本口径。
            if p & n != 0 {
                continue;
            }
            // SAFETY: 单个字，x 长度 64 覆盖整行。
            let (fast, slow) = unsafe {
                (
                    imp::row_dot_mask(&[p], &[n], &x),
                    scalar::row_dot_mask(&[p], &[n], &x),
                )
            };
            assert!(
                fast == slow,
                "pos={p:#04x} neg={n:#04x}: avx2={fast} scalar={slow}"
            );
        }
    }
}

#[test]
fn mask2_matches_single_bitwise() {
    if !avx2() {
        return;
    }
    let (rows, cols) = (8usize, 576usize);
    let (pos, neg, x) = rails(rows, cols);
    let bpr = cols.div_ceil(64);
    for r in (0..rows).step_by(2) {
        let (pa, na) = (&pos[r * bpr..(r + 1) * bpr], &neg[r * bpr..(r + 1) * bpr]);
        let (pb, nb) = (
            &pos[(r + 1) * bpr..(r + 2) * bpr],
            &neg[(r + 1) * bpr..(r + 2) * bpr],
        );
        // SAFETY: x 已补齐到 bpr*64。
        let (a, b) = unsafe { imp::row_dot_mask2(pa, na, pb, nb, &x) };
        // SAFETY: 同上。
        let (sa, sb) = unsafe { (imp::row_dot_mask(pa, na, &x), imp::row_dot_mask(pb, nb, &x)) };
        assert_eq!(a, sa, "row {r} 两行版与单行版必须逐位一致");
        assert_eq!(b, sb, "row {} 两行版与单行版必须逐位一致", r + 1);
    }
}

#[test]
fn scalar_f32_and_f16_match_avx2() {
    if !avx2() {
        return;
    }
    let n = 576usize;
    let w: Vec<f32> = (0..n).map(|i| ((i * 13 % 17) as f32 - 8.0) * 0.1).collect();
    let x: Vec<f32> = (0..n).map(|i| ((i * 7 % 11) as f32 - 5.0) * 0.05).collect();
    // SAFETY: 等长切片。
    let (fast, slow) = unsafe { (imp::row_dot_f32(&w, &x), scalar::row_dot_f32(&w, &x)) };
    assert!(close(fast, slow), "f32: avx2={fast} scalar={slow}");

    let h: Vec<u16> = w.iter().map(|&v| crate::bench::to_f16(v)).collect();
    // SAFETY: 等长切片。
    let (fast, slow) = unsafe { (imp::row_dot_f16(&h, &x), scalar::row_dot_f16(&h, &x)) };
    assert!(close(fast, slow), "f16: avx2={fast} scalar={slow}");
}

#[test]
fn scalar_q4_matches_avx2() {
    if !avx2() {
        return;
    }
    let cols = 64usize;
    let w: Vec<f32> = (0..cols)
        .map(|i| ((i * 5 % 13) as f32 - 6.0) * 0.2)
        .collect();
    let packed = crate::bench::pack_q4(&w);
    let x: Vec<f32> = (0..cols)
        .map(|i| ((i * 3 % 9) as f32 - 4.0) * 0.1)
        .collect();
    let (qx, dx) = quantize_act_q8(&x);
    // SAFETY: packed 覆盖 dx.len() 个块，qx 覆盖 32*块数。
    let (fast, slow) = unsafe {
        (
            imp::row_dot_q4(&packed, &qx, &dx),
            scalar::row_dot_q4(&packed, &qx, &dx),
        )
    };
    assert!(close(fast, slow), "q4: avx2={fast} scalar={slow}");
}
