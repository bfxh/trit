//! 三值元胞规则与其邻域编号。

use trit_core::value::Ternary;

/// 一条总istic 三值规则：按邻域和查表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule(pub [Ternary; 7]);

impl Rule {
    /// 用 0..2187 的编号构造（三进制展开，低位对应 `sum = 0`）。
    pub fn from_index(index: usize) -> Option<Self> {
        if index >= 2187 {
            return None;
        }
        let mut digits = [Ternary::False; 7];
        let mut rest = index;
        for digit in &mut digits {
            *digit = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
        }
        Some(Self(digits))
    }
}

pub(crate) fn offset(value: Ternary) -> usize {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}
