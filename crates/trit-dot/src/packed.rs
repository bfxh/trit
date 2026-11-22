//! 带长度的二值（`±1`）向量打包（#590 根治）。
//!
//! `pack_bipolar` 把有效 trit 数一起放进 [`PackedBipolar`]，`bipolar_dot` 只接受
//! 这个类型——调用方不再有机会传一个与底层字数不符的 `len`。

use super::{BLOCK_BITS, DotError};

/// 已打包的二值（`±1`）向量：`1 = +1`、`0 = -1`，**尾部补 1**。
///
/// 不变式：`words.len() == len.div_ceil(64)`。字段私有，只能经
/// [`pack_bipolar`] 或 [`PackedBipolar::from_words`] 构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedBipolar {
    words: Vec<u64>,
    len: usize,
}

impl PackedBipolar {
    /// 从已有的字平面构造；字数必须正好覆盖 `len`（`len.div_ceil(64)`）。
    pub fn from_words(words: Vec<u64>, len: usize) -> Result<Self, DotError> {
        let expected = len.div_ceil(BLOCK_BITS);
        if words.len() != expected {
            return Err(DotError::LengthMismatch {
                left: words.len(),
                right: expected,
            });
        }
        Ok(Self { words, len })
    }

    /// 有效 trit 数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 打包后的字平面。
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }
}

/// 二值（`±1`）向量的位打包：`1 = +1`、`0 = -1`，**尾部补 1**。
pub fn pack_bipolar(values: &[i32]) -> PackedBipolar {
    let blocks = values.len().div_ceil(BLOCK_BITS);
    let mut words = vec![u64::MAX; blocks];
    for (index, value) in values.iter().enumerate() {
        if *value < 0 {
            words[index / BLOCK_BITS] &= !(1u64 << (index % BLOCK_BITS));
        }
    }
    PackedBipolar {
        words,
        len: values.len(),
    }
}
