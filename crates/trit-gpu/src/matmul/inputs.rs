//! 确定性随机矩阵、双导轨打包、CPU 期望值与 f32 转置视图。

use crate::{pack_columns, pack_rows};

pub(crate) struct Inputs {
    pub m: usize,
    pub n: usize,
    pub k: usize,
    /// 每行占多少个 64 位字。
    pub words: usize,
    pub a_pos: Vec<u32>,
    pub a_neg: Vec<u32>,
    pub b_pos: Vec<u32>,
    pub b_neg: Vec<u32>,
    /// CPU 位并行镜像（三值整数结果）。
    pub expected: Vec<i64>,
    /// A 的 f32 展开（vec4 视图要求长度是 4 的倍数，尾部补零）。
    pub fa: Vec<f32>,
    /// B 的转置 f32 展开（同上，补齐）。
    pub fb: Vec<f32>,
}

impl Inputs {
    pub fn generate(m: usize, k: usize, n: usize) -> Self {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 3) as i32 - 1
        };
        let a: Vec<i32> = (0..m * k).map(|_| next()).collect();
        let b: Vec<i32> = (0..k * n).map(|_| next()).collect();

        let (a_pos, a_neg) = pack_rows(&a, k);
        let (b_pos, b_neg) = pack_columns(&b, k, n);
        let expected = crate::matmul_words(m, n, &a_pos, &a_neg, &b_pos, &b_neg);
        let words = k.div_ceil(crate::TRITS_PER_WORD);

        let mut fa: Vec<f32> = a.iter().map(|v| *v as f32).collect();
        let mut fb: Vec<f32> = Vec::with_capacity(n * k);
        for col in 0..n {
            for row in 0..k {
                fb.push(b[row * n + col] as f32);
            }
        }
        // vec4 视图要求绑定尺寸是 16 字节的倍数，小矩阵（如 1×1×1）必须补齐。
        fa.resize(fa.len().next_multiple_of(4), 0.0);
        fb.resize(fb.len().next_multiple_of(4), 0.0);

        Self {
            m,
            n,
            k,
            words,
            a_pos,
            a_neg,
            b_pos,
            b_neg,
            expected,
            fa,
            fb,
        }
    }
}
