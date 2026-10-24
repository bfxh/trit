//! 三值块交织 / 解交织。
//!
//! 把 trit 序列按 `block` 行的矩阵**按行写入、按列读出**，让原本连续的 burst error
//! 在解交织后被打散到不同的码字里——配合 `hamming` 的单符号纠错，能纠正长突发错。
//!
//! 例子（`block=2`，输入 6 个符号）：
//!
//! ```text
//! 行写：a b c
//!       d e f
//! 列读：a d b e c f
//! ```
//!
//! 解交织就是逆操作。`block=1` 或 `block=len` 是恒等变换。
//! 尾部不满一行的部分自然保留在对角位置，解交织按原长度还原。

use crate::CodecError;
use trit_core::value::Ternary;

/// 交织：按行写入 `block` 行矩阵，按列读出。
pub fn interleave(values: &[Ternary], block: usize) -> Result<Vec<Ternary>, CodecError> {
    if block == 0 {
        return Err(CodecError::BadBlockSize(0));
    }
    let n = values.len();
    if n == 0 || block == 1 || block >= n {
        return Ok(values.to_vec());
    }
    let cols = n.div_ceil(block);
    let mut out = Vec::with_capacity(n);
    for c in 0..cols {
        for r in 0..block {
            let idx = r * cols + c;
            if idx < n {
                out.push(values[idx]);
            }
        }
    }
    Ok(out)
}

/// 解交织：按列写回 `block` 行矩阵，按行读出，长度为 `len`。
pub fn deinterleave(
    values: &[Ternary],
    block: usize,
    len: usize,
) -> Result<Vec<Ternary>, CodecError> {
    if block == 0 {
        return Err(CodecError::BadBlockSize(0));
    }
    if values.len() != len {
        return Err(CodecError::InterleaveLengthMismatch {
            expected: len,
            actual: values.len(),
        });
    }
    if len == 0 || block == 1 || block >= len {
        return Ok(values.to_vec());
    }
    let cols = len.div_ceil(block);
    let mut out = vec![Ternary::Unknown; len];
    let mut k = 0;
    for c in 0..cols {
        for r in 0..block {
            let idx = r * cols + c;
            if idx < len {
                out[idx] = values[k];
                k += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(chars: &[char]) -> Vec<Ternary> {
        chars
            .iter()
            .map(|c| Ternary::parse_char(*c).unwrap())
            .collect()
    }

    #[test]
    fn block_one_is_identity() {
        let input = v(&['1', '0', 'X', '1']);
        assert_eq!(interleave(&input, 1).unwrap(), input);
        assert_eq!(deinterleave(&input, 1, input.len()).unwrap(), input);
    }

    #[test]
    fn block_ge_len_is_identity() {
        let input = v(&['1', '0', 'X']);
        assert_eq!(interleave(&input, 10).unwrap(), input);
        assert_eq!(deinterleave(&input, 10, input.len()).unwrap(), input);
    }

    #[test]
    fn zero_block_is_rejected() {
        assert!(matches!(
            interleave(&[], 0),
            Err(CodecError::BadBlockSize(0))
        ));
    }

    #[test]
    fn hand_known_example() {
        // block=2, 6 个符号：a b c / d e f → a d b e c f
        let input = v(&['1', '0', 'X', '1', '0', 'X']);
        let out = interleave(&input, 2).unwrap();
        assert_eq!(out, v(&['1', '1', '0', '0', 'X', 'X']));
        // 逆操作还原。
        let back = deinterleave(&out, 2, out.len()).unwrap();
        assert_eq!(back, input);
    }

    #[test]
    fn ragged_tail_round_trips() {
        // 7 个符号，block=3：cols=3，最后一列只有 1 个元素。
        let input = v(&['1', '0', 'X', '1', '0', 'X', '1']);
        let out = interleave(&input, 3).unwrap();
        assert_eq!(out.len(), 7);
        let back = deinterleave(&out, 3, out.len()).unwrap();
        assert_eq!(back, input);
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let input = v(&['1', '0', 'X']);
        assert!(matches!(
            deinterleave(&input, 2, 5),
            Err(CodecError::InterleaveLengthMismatch {
                expected: 5,
                actual: 3
            })
        ));
    }

    #[test]
    fn burst_error_is_spread() {
        // 模拟 burst error：原序列前 3 位连续翻转。交织后这 3 位落到不同列，
        // 解交织后被打散到相隔 block 的位置，配合 hamming 可分别纠正。
        let input = v(&['1', '1', '1', '0', '0', '0', 'X', 'X', 'X']);
        let interleaved = interleave(&input, 3).unwrap();
        // 模拟 burst：交错后前 3 位是原 [0,3,6] 位置。
        let mut corrupted = interleaved.clone();
        for cell in corrupted.iter_mut().take(3) {
            *cell = match *cell {
                Ternary::True => Ternary::False,
                Ternary::False => Ternary::True,
                Ternary::Unknown => Ternary::True,
            };
        }
        let deinterleaved = deinterleave(&corrupted, 3, corrupted.len()).unwrap();
        // 错误位置应分散在 0, 3, 6，而不是连续 0,1,2。
        let err_positions: Vec<usize> = deinterleaved
            .iter()
            .zip(input.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(err_positions, vec![0, 3, 6]);
    }
}
