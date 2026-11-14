//! 三值位置编码：每个位置一个 trit 向量，加到 key 上再做注意力。
//!
//! 与线性位置偏置不同，位置本身是一个三值向量——可以是从训练好的模型复用的
//! position embedding 三值化，也可以是用 `repair` / `analyze` 优化出来的表。

use crate::attention::AttentionTrace;
use crate::attention_multi::MultiHeadAttention;
use crate::error::{Error, Result};
use crate::trit::Trit;

/// 三值位置编码表：第 `i` 行是位置 `i` 的编码。
#[derive(Clone, Debug, PartialEq)]
pub struct PositionEncoding {
    /// 每行一个位置编码。
    pub table: Vec<Vec<Trit>>,
}

impl PositionEncoding {
    /// 所有行的宽度必须一致且非零。
    pub fn new(table: Vec<Vec<Trit>>) -> Result<Self> {
        let width = table.first().map_or(0, Vec::len);
        if width == 0 {
            return Err(Error::Shape {
                what: "position width",
                expected: 1,
                got: 0,
            });
        }
        if table.iter().any(|row| row.len() != width) {
            return Err(Error::Shape {
                what: "position width",
                expected: width,
                got: table.iter().map(Vec::len).max().unwrap_or(0),
            });
        }
        Ok(Self { table })
    }

    /// 把位置编码加到 key 上：逐元素三值相加后取符号。
    #[must_use]
    pub fn apply(&self, keys: &[Vec<Trit>]) -> Vec<Vec<Trit>> {
        keys.iter()
            .enumerate()
            .map(|(index, key)| {
                let position = self.table.get(index);
                key.iter()
                    .enumerate()
                    .map(|(col, value)| {
                        let bias = position
                            .and_then(|row| row.get(col))
                            .copied()
                            .unwrap_or(Trit::Zero);
                        Trit::sign_of(i32::from(value.to_i8()) + i32::from(bias.to_i8()))
                    })
                    .collect()
            })
            .collect()
    }
}

/// 对每个头应用对应的位置编码（缺省时用原 key）。
pub fn forward_with_encodings(
    multi: &MultiHeadAttention,
    query: &[Trit],
    keys: &[Vec<Trit>],
    values: &[Vec<Trit>],
    encodings: &[PositionEncoding],
) -> Result<Vec<AttentionTrace>> {
    multi
        .heads
        .iter()
        .enumerate()
        .map(|(index, head)| {
            let encoded = encodings
                .get(index)
                .map_or_else(|| keys.to_vec(), |encoding| encoding.apply(keys));
            head.forward(query, &encoded, values)
        })
        .collect()
}
