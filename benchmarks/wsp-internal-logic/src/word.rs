//! 字母表与串编码：Σ = {0,1}，串长度可变（`0..=max_len`）。
//!
//! 每个位置编成 **2 维** `(is_one, is_end)`：
//!
//! | 符号 | is_one | is_end |
//! |---|---|---|
//! | `0` | `Zero` | `Neg` |
//! | `1` | `Pos` | `Neg` |
//! | 串结束后的填充 | `Zero` | `Pos` |
//!
//! `is_one` 维上「0」与「结束」都取 `Zero`，所以网络的线性阈值只数 `Pos` 时
//! 天然忽略填充长度——这是变长串能进定长前馈网的关键。

use trit_wspace::Trit;

/// 二进制符号 → 三值。
#[must_use]
pub fn symbol(bit: bool) -> Trit {
    if bit {
        Trit::Pos
    } else {
        Trit::Neg
    }
}

/// 所有长度 `0..=max_len` 的串，长度升序、同长字典序。
#[must_use]
pub fn all_words(max_len: usize) -> Vec<Vec<Trit>> {
    let mut out = Vec::new();
    for len in 0..=max_len {
        for mask in 0..(1usize << len) {
            out.push(
                (0..len)
                    .map(|j| symbol((mask >> (len - 1 - j)) & 1 == 1))
                    .collect(),
            );
        }
    }
    out
}

/// 定长编码：`max_len` 个位置，每位置 2 维（见模块文档）。
#[must_use]
pub fn encode(word: &[Trit], max_len: usize) -> Vec<Trit> {
    let mut out = Vec::with_capacity(max_len * 2);
    for i in 0..max_len {
        match word.get(i) {
            Some(Trit::Pos) => {
                out.push(Trit::Pos);
                out.push(Trit::Neg);
            }
            Some(Trit::Neg) => {
                out.push(Trit::Zero);
                out.push(Trit::Neg);
            }
            _ => {
                out.push(Trit::Zero);
                out.push(Trit::Pos);
            }
        }
    }
    out
}

/// 串的比特串（打印/比对用）。
#[must_use]
pub fn bits(word: &[Trit]) -> String {
    word.iter()
        .map(|t| if *t == Trit::Pos { '1' } else { '0' })
        .collect()
}
