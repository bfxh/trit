//! 三值反射格雷码（ternary reflected Gray code）。
//!
//! 把 `index ∈ [0, 3^n)` 编码成 n 个 trit 的码字，使得**相邻 index（k 与 k+1）的码字
//! 汉明距离恰好为 1**——只有一位 trit 不同，且该位按 ±1 mod 3 变化。这把「连续整数」
//! 映射成「连续码字」，在机械位置传感、旋转编码器、跨时钟域信号里能把跳变的
//! 误读降到最低。
//!
//! 为什么对错误检测友好：相邻状态只差一位，转动/采样时的亚稳态或抖动只会读到
//! **相邻的合法码字**，解码出的位置偏差最小，绝不会一步跳到不相干的状态；
//! 反过来，观测到「两位同时跳变」就强烈提示一次传输/采样错误，而不是正常步进。
//!
//! 编码规则（`k[i]` 是 index 的三进制位，`k[0]` = LSB，`k[n]` = 0 为虚拟高位）：
//!
//! - `g[i] = (k[i] - k[i+1]) mod 3`，i = 0..n-1；
//! - 输出字按 `g[0]`（LSB）在前、`g[n-1]`（MSB）在后存放。
//!
//! 解码反向递推（从虚拟高位 `k[n]=0` 开始）：
//!
//! - `k[i] = (k[i+1] + g[i]) mod 3`；
//! - 还原后 `index = Σ k[i] · 3^i`（Horner 法累加，不重复算幂）。
//!
//! 边界：
//!
//! - `n = 0`：只有 `index == 0` 合法，码字为空、解码空码字得 0；
//! - `index >= 3^n` → [`CodecError::GrayIndexOutOfRange`]；
//! - `3^n` 用 `checked_pow` 算，n 大到溢出 usize 时也归为越界。64 位 usize 在
//!   `n ≥ 41` 时溢出，实验用途（n ≤ 12）远在安全区内。

use crate::CodecError;
use trit_core::value::Ternary;

/// 编码：把 `index` 编成长度为 `n` 的三值格雷码字（LSB 在前）。
pub fn encode(index: usize, n: usize) -> Result<Vec<Ternary>, CodecError> {
    let size = 3usize
        .checked_pow(n as u32)
        .ok_or(CodecError::GrayIndexOutOfRange { index, n })?;
    if index >= size {
        return Err(CodecError::GrayIndexOutOfRange { index, n });
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    // k[i] = index 的第 i 位三进制位（i=0 LSB）。
    let mut k: Vec<usize> = Vec::with_capacity(n + 1);
    let mut rem = index;
    for _ in 0..n {
        k.push(rem % 3);
        rem /= 3;
    }
    k.push(0); // k[n] = 0 虚拟高位
    // g[i] = (k[i] - k[i+1]) mod 3
    let word: Vec<Ternary> = (0..n)
        .map(|i| {
            let g = (k[i] as i8 - k[i + 1] as i8).rem_euclid(3) as usize;
            Ternary::from_index(g)
        })
        .collect();
    Ok(word)
}

/// 解码：把 n 位格雷码字还原成 index（LSB 在 `word[0]`）。
///
/// 任何长度的合法 trit 字都可逆；空字解码为 0。
///
/// 溢出：当 `3^n` 超出 `usize` 容量时（64 位约 n ≥ 41，远超实验用途），还原出的
/// index 无法用 `usize` 表示。本函数**绝不 panic**：改用 `checked_mul`/`checked_add`
/// 的 Horner 累加，一旦不可表示就饱和返回 [`usize::MAX`] 作为哨兵。由 [`encode`]
/// 产出的合法码字 n ≤ 40（64 位），永远不会走到这条路径。
pub fn decode(word: &[Ternary]) -> usize {
    let n = word.len();
    if n == 0 {
        return 0;
    }
    let mut k_next = 0usize; // k[n] = 0 虚拟高位
    let mut index = 0usize;
    // 从 MSB（i=n-1）往 LSB（i=0）递推 k[i] = (k[i+1] + g[i]) mod 3，
    // 同时 Horner 累加 index = index*3 + k_i，避免反复算 3^i（那才是溢出 panic 的来源）。
    for i in (0..n).rev() {
        let k_i = (k_next + word[i].index()) % 3;
        index = index
            .checked_mul(3)
            .and_then(|v| v.checked_add(k_i))
            .unwrap_or(usize::MAX);
        k_next = k_i;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(word: &[Ternary]) -> String {
        word.iter().map(|t| t.as_char()).collect()
    }

    #[test]
    fn n_zero_only_allows_index_zero() {
        assert_eq!(encode(0, 0).unwrap(), Vec::<Ternary>::new());
        assert!(matches!(
            encode(1, 0),
            Err(CodecError::GrayIndexOutOfRange { index: 1, n: 0 })
        ));
        assert_eq!(decode(&[]), 0);
    }

    #[test]
    fn out_of_range_index_is_rejected() {
        // n=2 → 合法 index 0..=8。
        assert!(encode(9, 2).is_err());
        assert!(encode(0, 2).is_ok());
    }

    #[test]
    fn hand_known_n2_table() {
        // 手算 n=2：k[0]=LSB, k[1]=MSB, k[2]=0。
        // idx 0: k=(0,0,0) → g=(0-0,0-0)=(0,0) → "00"
        // idx 1: k=(1,0,0) → g=(1-0,0-0)=(1,0) → "X0"
        // idx 2: k=(2,0,0) → g=(2-0,0-0)=(2,0) → "10"
        // idx 3: k=(0,1,0) → g=(0-1,1-0)=(2,1) → "1X"
        let cases = [(0usize, "00"), (1, "X0"), (2, "10"), (3, "1X")];
        for (idx, expect) in cases {
            let w = encode(idx, 2).unwrap();
            assert_eq!(chars(&w), expect, "idx={idx}");
            assert_eq!(decode(&w), idx);
        }
    }

    #[test]
    fn exhaustive_n2_adjacent_distance_is_one() {
        let n = 2;
        let size = 3usize.pow(n as u32);
        let words: Vec<Vec<Ternary>> = (0..size).map(|i| encode(i, n).unwrap()).collect();
        for k in 0..size - 1 {
            let dist = words[k]
                .iter()
                .zip(words[k + 1].iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                dist,
                1,
                "idx {k} vs {} words {} {}",
                k + 1,
                chars(&words[k]),
                chars(&words[k + 1])
            );
            // round-trip
            assert_eq!(decode(&words[k]), k);
        }
    }

    #[test]
    fn exhaustive_n3_adjacent_distance_is_one() {
        let n = 3;
        let size = 3usize.pow(n as u32);
        let words: Vec<Vec<Ternary>> = (0..size).map(|i| encode(i, n).unwrap()).collect();
        for k in 0..size - 1 {
            let dist = words[k]
                .iter()
                .zip(words[k + 1].iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(dist, 1, "idx {k} vs {}", k + 1);
            assert_eq!(decode(&words[k]), k);
        }
        // 最后一个码字应该也是合法的
        assert_eq!(decode(&words[size - 1]), size - 1);
    }

    #[test]
    fn word_length_matches_n() {
        for n in 1..=6 {
            let w = encode(0, n).unwrap();
            assert_eq!(w.len(), n);
        }
    }

    #[test]
    fn n_one_exhaustive() {
        // n=1：3 个状态 0/X/1，相邻两两汉明距离 1，往返恒等。
        let words: Vec<Vec<Ternary>> = (0..3).map(|i| encode(i, 1).unwrap()).collect();
        assert_eq!(
            words.iter().map(|w| chars(w)).collect::<Vec<_>>(),
            ["0", "X", "1"]
        );
        for k in 0..2 {
            let dist = words[k]
                .iter()
                .zip(words[k + 1].iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(dist, 1, "idx {k} vs {}", k + 1);
            assert_eq!(decode(&words[k]), k);
        }
        assert_eq!(decode(&words[2]), 2);
    }

    #[test]
    fn zero_index_is_all_false_anchor() {
        // k=0 的三进制位全 0 ⇒ g[i]=(0-0)=0 ⇒ 码字全 False（"000…0"），这是格雷码的锚点。
        for n in 0..=6usize {
            let w = encode(0, n).unwrap();
            assert!(
                w.iter().all(|t| *t == Ternary::False),
                "n={n} -> {}",
                chars(&w)
            );
            assert_eq!(decode(&w), 0, "n={n}");
        }
    }

    #[test]
    fn overlong_word_saturates_without_panicking() {
        // 60 位码字还原出的 index 远超 usize::MAX（32/64 位都溢出）。
        // 实验层铁律：绝不 panic——必须饱和到 usize::MAX 而非 debug 下的 multiply overflow。
        let word = vec![Ternary::True; 60];
        assert_eq!(decode(&word), usize::MAX);
        // 空字仍得 0（不受饱和影响）。
        assert_eq!(decode(&[]), 0);
    }
}
