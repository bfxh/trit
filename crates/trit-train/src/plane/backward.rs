//! 双导轨权重矩阵与反向 `dX = Wᵀdy` 的位平面内核。
//!
//! 布局与 [`super::Planes`] 一致：每行每 64 个 trit 一个 `u64`，`pos` 位为
//! `+1`、`neg` 位为 `-1`，两位皆 0 为 `0`。反向不做三进制算术加法，也不做
//! 浮点乘法：对每一行取 `coeff = dy[r] * scale[r]`，按 `pos` / `neg` 掩码把
//! `coeff` 加减到 `dX` 的 64 个 lane 上。
//!
//! 这条路径只消费双导轨位平面，所以和 [`super::Planes`] 的 ALU 边界一致：
//! 它加速的是「按位选符号 + f32 累加」，不是三进制进位加法。
use super::TRITS_PER_WORD;

/// 行主序的三值权重矩阵，同时保留双导轨位平面与展开后的 `q`。
///
/// `q` 让调用方更新 latent/scale 时不用再从位平面逐位解包；位平面只服务
/// 点积与反向的按字内核。`bytes()` 只算两条导轨，不含 `q` 缓存。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaneMatrix {
    pub(super) rows: usize,
    pub(super) cols: usize,
    pub(super) bpr: usize,
    pub(super) q: Vec<i8>,
    pub(super) pos: Vec<u64>,
    pub(super) neg: Vec<u64>,
}

impl PlaneMatrix {
    /// 从行主序 `-1/0/+1` 构造；`q.len()` 必须是 `rows * cols`。
    pub fn from_quantized(q: Vec<i8>, rows: usize, cols: usize) -> Self {
        assert_eq!(q.len(), rows * cols, "q 长度必须是 rows * cols");
        assert!(q.iter().all(|v| (-1..=1).contains(v)), "q 必须是 -1/0/+1");
        let bpr = cols.div_ceil(TRITS_PER_WORD).max(1);
        let mut pos = vec![0u64; rows * bpr];
        let mut neg = vec![0u64; rows * bpr];
        for r in 0..rows {
            for c in 0..cols {
                let word = r * bpr + c / TRITS_PER_WORD;
                let bit = 1u64 << (c % TRITS_PER_WORD);
                match q[r * cols + c] {
                    1 => pos[word] |= bit,
                    -1 => neg[word] |= bit,
                    _ => {}
                }
            }
        }
        Self {
            rows,
            cols,
            bpr,
            q,
            pos,
            neg,
        }
    }

    /// 从按行给出的三值构造；每行长度必须等于 `cols`。
    pub fn from_rows(rows: &[Vec<i8>], cols: usize) -> Self {
        let mut q = Vec::with_capacity(rows.len() * cols);
        for (r, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), cols, "第 {r} 行长度不是 {cols}");
            q.extend_from_slice(row);
        }
        Self::from_quantized(q, rows.len(), cols)
    }

    /// 行数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数。
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 每行多少个 64 位字。
    pub fn blocks_per_row(&self) -> usize {
        self.bpr
    }

    /// 双导轨位平面的字节数（不含 `q` 缓存）。
    pub fn bytes(&self) -> usize {
        (self.pos.len() + self.neg.len()) * 8
    }

    /// 展开后的三值权重（行主序）。
    pub fn q(&self) -> &[i8] {
        &self.q
    }

    /// 单行点积 `Σ q[r,c] * x[c]`（标量参考）。
    pub fn row_dot_scalar(&self, r: usize, x: &[f32]) -> f32 {
        assert!(r < self.rows, "行号越界");
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        let pos = &self.pos[r * self.bpr..(r + 1) * self.bpr];
        let neg = &self.neg[r * self.bpr..(r + 1) * self.bpr];
        let mut acc = 0.0f32;
        for (c, &xc) in x.iter().enumerate() {
            let word = c / TRITS_PER_WORD;
            let bit = 1u64 << (c % TRITS_PER_WORD);
            if pos[word] & bit != 0 {
                acc += xc;
            } else if neg[word] & bit != 0 {
                acc -= xc;
            }
        }
        acc
    }

    /// 单行点积的自动内核：AVX2 掩码 + 标量尾。
    pub fn row_dot(&self, r: usize, x: &[f32]) -> f32 {
        assert!(r < self.rows, "行号越界");
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        #[cfg(target_arch = "x86_64")]
        if self.cols >= 8 && has_avx2() {
            let pos = &self.pos[r * self.bpr..(r + 1) * self.bpr];
            let neg = &self.neg[r * self.bpr..(r + 1) * self.bpr];
            // SAFETY: 行切片长度都是 bpr；x 长度已断言为 cols。
            return unsafe { simd::row_dot(pos, neg, x, self.cols) };
        }
        self.row_dot_scalar(r, x)
    }

    /// 反向：`out[c] = Σ_r dy[r] * scale[r] * q[r,c]`。
    ///
    /// 自动选择 AVX2 掩码内核；非 x86_64 或列数不足 8 时走标量参考。
    pub fn input_grad(&self, dy: &[f32], scale: &[f32], out: &mut [f32]) {
        assert_eq!(dy.len(), self.rows, "dy 长度必须是 rows");
        assert_eq!(scale.len(), self.rows, "scale 长度必须是 rows");
        assert_eq!(out.len(), self.cols, "输出长度必须是 cols");
        #[cfg(target_arch = "x86_64")]
        if self.cols >= 8 && has_avx2() {
            // SAFETY: 矩阵与 dy/scale 的长度已断言；内核按 rows/cols 切片。
            let got = unsafe {
                simd::input_grad(
                    &self.pos, &self.neg, self.bpr, self.rows, self.cols, dy, scale,
                )
            };
            out.copy_from_slice(&got);
            return;
        }
        self.input_grad_scalar(dy, scale, out);
    }

    /// 标量参考：逐行、逐列按位选符号后累加 `coeff`。
    pub fn input_grad_scalar(&self, dy: &[f32], scale: &[f32], out: &mut [f32]) {
        assert_eq!(dy.len(), self.rows, "dy 长度必须是 rows");
        assert_eq!(scale.len(), self.rows, "scale 长度必须是 rows");
        assert_eq!(out.len(), self.cols, "输出长度必须是 cols");
        out.fill(0.0);
        for r in 0..self.rows {
            let coeff = dy[r] * scale[r];
            let pos = &self.pos[r * self.bpr..(r + 1) * self.bpr];
            let neg = &self.neg[r * self.bpr..(r + 1) * self.bpr];
            for (c, slot) in out.iter_mut().enumerate() {
                let word = c / TRITS_PER_WORD;
                let bit = 1u64 << (c % TRITS_PER_WORD);
                if pos[word] & bit != 0 {
                    *slot += coeff;
                } else if neg[word] & bit != 0 {
                    *slot -= coeff;
                }
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub(super) fn has_avx2() -> bool {
    std::arch::is_x86_feature_detected!("avx2")
}

#[cfg(not(target_arch = "x86_64"))]
pub(super) fn has_avx2() -> bool {
    false
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

    const fn build_mask_lut() -> [[i32; 8]; 256] {
        let mut table = [[0i32; 8]; 256];
        let mut b = 0usize;
        while b < 256 {
            let mut k = 0usize;
            while k < 8 {
                table[b][k] = if (b >> k) & 1 == 1 { -1 } else { 0 };
                k += 1;
            }
            b += 1;
        }
        table
    }

    static MASK_LUT: [[i32; 8]; 256] = build_mask_lut();

    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn hsum(v: __m256) -> f32 {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps(v, 1);
        let s = _mm_add_ps(lo, hi);
        let s = _mm_hadd_ps(s, s);
        let s = _mm_hadd_ps(s, s);
        _mm_cvtss_f32(s)
    }

    /// 单行点积：每 8 个 trit 取一个字节的 `pos`/`neg` 掩码，8 个 lane 并行加减。
    ///
    /// # Safety
    /// 需要 AVX2；`x.len() == cols`，`pos`/`neg` 是同一行的 `cols.div_ceil(64)` 个
    /// 64 位字。
    #[target_feature(enable = "avx2")]
    pub unsafe fn row_dot(pos: &[u64], neg: &[u64], x: &[f32], cols: usize) -> f32 {
        let full = cols / 8;
        let mut acc = [_mm256_setzero_ps(); 4];
        for j in 0..full {
            let w = j / 8;
            let k = j % 8;
            let base = j * 8;
            let pb = ((pos[w] >> (k * 8)) & 0xFF) as usize;
            let nb = ((neg[w] >> (k * 8)) & 0xFF) as usize;
            // SAFETY: base + 8 <= full * 8 <= cols。
            let xv = _mm256_loadu_ps(x.as_ptr().add(base));
            let mp =
                _mm256_castsi256_ps(_mm256_loadu_si256(MASK_LUT[pb].as_ptr() as *const __m256i));
            let mn =
                _mm256_castsi256_ps(_mm256_loadu_si256(MASK_LUT[nb].as_ptr() as *const __m256i));
            let idx = j & 3;
            acc[idx] = _mm256_add_ps(acc[idx], _mm256_and_ps(xv, mp));
            acc[idx] = _mm256_sub_ps(acc[idx], _mm256_and_ps(xv, mn));
        }
        let mut sum = hsum(_mm256_add_ps(
            _mm256_add_ps(acc[0], acc[1]),
            _mm256_add_ps(acc[2], acc[3]),
        ));
        for (c, &xc) in x.iter().enumerate().skip(full * 8) {
            let word = c / 64;
            let bit = 1u64 << (c % 64);
            if pos[word] & bit != 0 {
                sum += xc;
            } else if neg[word] & bit != 0 {
                sum -= xc;
            }
        }
        sum
    }

    /// 反向 `dX`：4 个行累加器打散依赖链，最后合并。
    ///
    /// # Safety
    /// 需要 AVX2；`pos`/`neg` 长度是 `rows * bpr`，`dy`/`scale` 长度是 `rows`，
    /// `bpr * 64 >= cols`。
    #[target_feature(enable = "avx2")]
    pub unsafe fn input_grad(
        pos: &[u64],
        neg: &[u64],
        bpr: usize,
        rows: usize,
        cols: usize,
        dy: &[f32],
        scale: &[f32],
    ) -> Vec<f32> {
        let full = cols / 8;
        let mut acc: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0f32; cols]);
        for r in 0..rows {
            let coeff = dy[r] * scale[r];
            let cv = _mm256_set1_ps(coeff);
            let p = &pos[r * bpr..(r + 1) * bpr];
            let n = &neg[r * bpr..(r + 1) * bpr];
            let a = &mut acc[r & 3];
            for j in 0..full {
                let w = j / 8;
                let k = j % 8;
                let base = j * 8;
                let pb = ((p[w] >> (k * 8)) & 0xFF) as usize;
                let nb = ((n[w] >> (k * 8)) & 0xFF) as usize;
                let mp = _mm256_castsi256_ps(_mm256_loadu_si256(
                    MASK_LUT[pb].as_ptr() as *const __m256i
                ));
                let mn = _mm256_castsi256_ps(_mm256_loadu_si256(
                    MASK_LUT[nb].as_ptr() as *const __m256i
                ));
                // SAFETY: base + 8 <= full * 8 <= cols；a 长度是 cols。
                let mut v = _mm256_loadu_ps(a.as_ptr().add(base));
                v = _mm256_add_ps(v, _mm256_and_ps(cv, mp));
                v = _mm256_sub_ps(v, _mm256_and_ps(cv, mn));
                _mm256_storeu_ps(a.as_mut_ptr().add(base), v);
            }
            for (c, slot) in a.iter_mut().enumerate().skip(full * 8) {
                let word = c / 64;
                let bit = 1u64 << (c % 64);
                if p[word] & bit != 0 {
                    *slot += coeff;
                } else if n[word] & bit != 0 {
                    *slot -= coeff;
                }
            }
        }
        let mut out = vec![0.0f32; cols];
        for (c, slot) in out.iter_mut().enumerate() {
            *slot = acc[0][c] + acc[1][c] + acc[2][c] + acc[3][c];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_q(rows: usize, cols: usize, seed: u64) -> Vec<Vec<i8>> {
        let mut state = seed | 1;
        (0..rows)
            .map(|_| {
                (0..cols)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        (state % 3) as i8 - 1
                    })
                    .collect()
            })
            .collect()
    }

    fn sample_f32(len: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state % 2001) as f32 - 1000.0) / 500.0
            })
            .collect()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| (x - y).abs() <= 1e-4 * (1.0 + y.abs()))
    }

    #[test]
    fn packed_q_round_trips() {
        for (rows, cols) in [(1usize, 1usize), (3, 7), (5, 64), (4, 65), (7, 130)] {
            let q = sample_q(rows, cols, 11 + (rows * 100 + cols) as u64);
            let m = PlaneMatrix::from_rows(&q, cols);
            assert_eq!(m.rows(), rows);
            assert_eq!(m.cols(), cols);
            let flat: Vec<i8> = q.iter().flatten().copied().collect();
            assert_eq!(m.q(), flat.as_slice());
            assert_eq!(m.bytes(), rows * m.blocks_per_row() * 16);
        }
    }

    #[test]
    fn row_dot_matches_hand_reference() {
        for (rows, cols) in [(1usize, 1usize), (4, 8), (5, 9), (6, 63), (7, 65), (8, 130)] {
            let q = sample_q(rows, cols, 23 + rows as u64);
            let m = PlaneMatrix::from_rows(&q, cols);
            for (r, row) in q.iter().enumerate() {
                let x = sample_f32(cols, 31 + r as u64);
                let want: f32 = row
                    .iter()
                    .zip(&x)
                    .map(|(&qi, &xi)| f32::from(qi) * xi)
                    .sum();
                let got = m.row_dot(r, &x);
                assert!(
                    (got - want).abs() <= 1e-4 * (1.0 + want.abs()),
                    "rows={rows} cols={cols} r={r}: got={got} want={want}"
                );
            }
        }
    }

    #[test]
    fn reverse_matches_hand_reference() {
        for (rows, cols) in [
            (1usize, 1usize),
            (3, 7),
            (4, 8),
            (5, 9),
            (6, 63),
            (7, 64),
            (8, 65),
            (11, 130),
            (17, 576),
        ] {
            let q = sample_q(rows, cols, 41 + (rows * 31 + cols) as u64);
            let m = PlaneMatrix::from_rows(&q, cols);
            let dy = sample_f32(rows, 53 + rows as u64);
            let scale = sample_f32(rows, 67 + cols as u64)
                .into_iter()
                .map(|v| v.abs() + 0.01)
                .collect::<Vec<_>>();
            let want: Vec<f32> = (0..cols)
                .map(|c| {
                    (0..rows)
                        .map(|r| dy[r] * scale[r] * f32::from(q[r][c]))
                        .sum()
                })
                .collect();
            let mut fast = vec![0.0f32; cols];
            m.input_grad(&dy, &scale, &mut fast);
            let mut slow = vec![0.0f32; cols];
            m.input_grad_scalar(&dy, &scale, &mut slow);
            assert!(
                close(&fast, &want),
                "AVX2 反向不对：rows={rows} cols={cols}"
            );
            assert!(close(&slow, &want), "标量反向不对：rows={rows} cols={cols}");
        }
    }

    #[test]
    fn zero_matrix_returns_zero() {
        let m = PlaneMatrix::from_quantized(vec![0; 5 * 130], 5, 130);
        let dy = vec![1.0f32; 5];
        let scale = vec![0.5f32; 5];
        let mut out = vec![9.0f32; 130];
        m.input_grad(&dy, &scale, &mut out);
        assert!(out.iter().all(|v| v.abs() < 1e-6));
    }
}
