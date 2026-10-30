//! **DP4A 数据面**：三值权重 / 量化激活的 int8 打包、CPU 参考实现与对拍。
//!
//! 与 `dp4a.wgsl` 严格同构：两侧都按 **K 方向连续**、**4 个 int8/u32 小端**打包，
//! B 侧**预转置**成 N 行 × K（每个输出列一行）。
//!
//! 这一层不依赖 GPU：`pack_*` 与 `reference` 都有单元测试，WGSL 侧只做同一条式子的向量化。

/// 一条 trit 值 → int8（`-1 → 0xFF`、`0 → 0x00`、`+1 → 0x01`）。
///
/// 必须是**有符号**语义：`dot4I8Packed` 把字节按 int8 解释，`0xFF` 才等于 `-1`。
#[inline]
pub fn trit_to_i8(v: i32) -> i8 {
    match v {
        -1 => -1,
        0 => 0,
        1 => 1,
        other => panic!("trit 只能是 -1/0/+1，收到 {other}"),
    }
}

/// 把一行 int8 值打包成 `ceil(len/4)` 个 u32（小端：第 0 个值在最低字节）。
pub fn pack_row(values: &[i8]) -> Vec<u32> {
    let mut out = vec![0u32; values.len().div_ceil(4)];
    for (i, v) in values.iter().enumerate() {
        out[i / 4] |= (*v as u8 as u32) << (8 * (i % 4));
    }
    out
}

/// 把 `rows × cols` 的矩阵按**行**打包（每行 `ceil(cols/4)` 个字）。
pub fn pack_matrix(values: &[i8], rows: usize, cols: usize) -> Vec<u32> {
    let kw = cols.div_ceil(4);
    let mut out = vec![0u32; rows * kw];
    for r in 0..rows {
        let packed = pack_row(&values[r * cols..(r + 1) * cols]);
        out[r * kw..(r + 1) * kw].copy_from_slice(&packed);
    }
    out
}

/// 把 `a`（M×K 三值）与 `b`（K×N int8）按内核布局打包：`a` 行主序、`b` **转置**成 N×K。
/// 返回 `(a_words, bt_words, k_words)`。
pub fn pack_for_kernel(
    a: &[i8],
    b: &[i8],
    m: usize,
    k: usize,
    n: usize,
) -> (Vec<u32>, Vec<u32>, usize) {
    let kw = k.div_ceil(4);
    let a_words = pack_matrix(a, m, k);
    let mut bt = vec![0i8; n * k];
    for kk in 0..k {
        for nn in 0..n {
            bt[nn * k + kk] = b[kk * n + nn];
        }
    }
    let bt_words = pack_matrix(&bt, n, k);
    (a_words, bt_words, kw)
}

/// **CPU 参考**：`C[m,n] = Σ_k A[m,k]·B[k,n]`（整型，无 scale）。
pub fn reference(a: &[i8], b: &[i8], m: usize, k: usize, n: usize) -> Vec<i32> {
    let mut c = vec![0i32; m * n];
    for mi in 0..m {
        for ni in 0..n {
            let mut acc = 0i32;
            for ki in 0..k {
                acc += a[mi * k + ki] as i32 * b[ki * n + ni] as i32;
            }
            c[mi * n + ni] = acc;
        }
    }
    c
}

/// **按内核的打包布局重算一遍**（模拟 `dot4I8Packed` 的字节语义），用于验证打包本身没错。
pub fn reference_packed(
    a_words: &[u32],
    bt_words: &[u32],
    m: usize,
    n: usize,
    kw: usize,
) -> Vec<i32> {
    let mut c = vec![0i32; m * n];
    for mi in 0..m {
        for ni in 0..n {
            let mut acc = 0i32;
            for w in 0..kw {
                let aw = a_words[mi * kw + w];
                let bw = bt_words[ni * kw + w];
                for byte in 0..4 {
                    let x = ((aw >> (8 * byte)) & 0xff) as u8 as i8;
                    let y = ((bw >> (8 * byte)) & 0xff) as u8 as i8;
                    acc += x as i32 * y as i32;
                }
            }
            c[mi * n + ni] = acc;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 打包 → 按字重算 必须等于直接参考（覆盖 K 不是 4 的倍数）。
    #[test]
    fn packed_reference_matches_direct() {
        for &(m, k, n) in &[(2usize, 3usize, 2usize), (3, 4, 5), (4, 7, 3)] {
            let a: Vec<i8> = (0..m * k).map(|i| [1i8, 0, -1][i % 3]).collect();
            let b: Vec<i8> = (0..k * n).map(|i| [1i8, -1, 1, 0][i % 4]).collect();
            let (aw, btw, kw) = pack_for_kernel(&a, &b, m, k, n);
            assert_eq!(kw, k.div_ceil(4));
            let want = reference(&a, &b, m, k, n);
            let got = reference_packed(&aw, &btw, m, n, kw);
            assert_eq!(got, want, "m={m} k={k} n={n}");
        }
    }

    /// `-1` 必须编码成 `0xFF`（有符号字节），否则 `dot4I8Packed` 会算成 +255。
    #[test]
    fn minus_one_encodes_signed() {
        let w = pack_row(&[-1, 0, 1, -1]);
        assert_eq!(w[0] & 0xff, 0xff);
        assert_eq!((w[0] >> 8) & 0xff, 0x00);
        assert_eq!((w[0] >> 16) & 0xff, 0x01);
        assert_eq!((w[0] >> 24) & 0xff, 0xff);
        // 按有符号解释回来
        assert_eq!((w[0] & 0xff) as u8 as i8, -1);
    }

    /// 全 −1 × 全 +1 ⇒ 每个输出都是 −K（整型，不受 f32 误差影响）。
    #[test]
    fn all_ones_times_all_minus_ones() {
        let (m, k, n) = (2usize, 8usize, 3usize);
        let a = vec![-1i8; m * k];
        let b = vec![1i8; k * n];
        let (aw, btw, kw) = pack_for_kernel(&a, &b, m, k, n);
        let got = reference_packed(&aw, &btw, m, n, kw);
        assert!(got.iter().all(|&v| v == -(k as i32)), "{got:?}");
    }

    /// 零权重（三值的 0）⇒ 输出全 0，与激活无关。
    #[test]
    fn zero_weights_give_zero() {
        let (m, k, n) = (1usize, 5usize, 3usize);
        let a = vec![0i8; m * k];
        let b: Vec<i8> = (0..k * n)
            .map(|i| if i % 2 == 0 { 1 } else { -1 })
            .collect();
        let (aw, btw, kw) = pack_for_kernel(&a, &b, m, k, n);
        let got = reference_packed(&aw, &btw, m, n, kw);
        assert!(got.iter().all(|&v| v == 0), "{got:?}");
    }
}
