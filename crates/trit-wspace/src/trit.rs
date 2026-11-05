//! 权重空间的基本单位：一个三值权重。
//!
//! 与 `trit-core::Ternary`（逻辑值 `False/Unknown/True`）不同，这里用**带符号**
//! 的 `-1/0/+1`，因为权重需要参与整数内积。映射关系：
//! `Neg ↔ -1`、`Zero ↔ 0`、`Pos ↔ +1`。

use std::fmt;

/// 一个三值权重。
///
/// 声明顺序即大小顺序：`Neg < Zero < Pos`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i8)]
pub enum Trit {
    /// `-1`。
    Neg = -1,
    /// `0`。
    Zero = 0,
    /// `+1`。
    Pos = 1,
}

impl Trit {
    /// 三种取值的固定顺序（用于确定性枚举）。
    pub const ALL: [Self; 3] = [Self::Neg, Self::Zero, Self::Pos];

    /// 转成 `i8`。
    #[must_use]
    pub const fn to_i8(self) -> i8 {
        self as i8
    }

    /// 从 `i8` 解析；非法值返回 `None`。
    #[must_use]
    pub const fn from_i8(value: i8) -> Option<Self> {
        match value {
            -1 => Some(Self::Neg),
            0 => Some(Self::Zero),
            1 => Some(Self::Pos),
            _ => None,
        }
    }

    /// 整数符号：负数 → `Neg`，正数 → `Pos`，零 → `Zero`。
    #[must_use]
    pub const fn sign_of(value: i32) -> Self {
        if value < 0 {
            Self::Neg
        } else if value > 0 {
            Self::Pos
        } else {
            Self::Zero
        }
    }

    /// 取反：`-1 ↔ +1`，`0` 不变。
    #[must_use]
    pub const fn neg(self) -> Self {
        match self {
            Self::Neg => Self::Pos,
            Self::Zero => Self::Zero,
            Self::Pos => Self::Neg,
        }
    }

    /// 循环到下一个取值：`-1 → 0 → +1 → -1`。
    #[must_use]
    pub const fn cycle(self) -> Self {
        match self {
            Self::Neg => Self::Zero,
            Self::Zero => Self::Pos,
            Self::Pos => Self::Neg,
        }
    }

    /// 解析单个字符：`'-'` / `'0'` / `'+'`。
    #[must_use]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            '-' => Some(Self::Neg),
            '0' => Some(Self::Zero),
            '+' => Some(Self::Pos),
            _ => None,
        }
    }

    /// 序列化为单个字符：`'-'` / `'0'` / `'+'`。
    #[must_use]
    pub const fn to_char(self) -> char {
        match self {
            Self::Neg => '-',
            Self::Zero => '0',
            Self::Pos => '+',
        }
    }
}

impl fmt::Display for Trit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Neg => "-",
            Self::Zero => "0",
            Self::Pos => "+",
        })
    }
}

impl From<Trit> for i8 {
    fn from(value: Trit) -> Self {
        value.to_i8()
    }
}

#[cfg(test)]
mod tests {
    use super::Trit;

    #[test]
    fn conversions_are_total_on_valid_range() {
        for v in -1..=1i8 {
            let t = Trit::from_i8(v).expect("valid trit");
            assert_eq!(t.to_i8(), v);
            assert_eq!(i8::from(t), v);
        }
        for v in [-128, -2, 2, 127] {
            assert_eq!(Trit::from_i8(v), None);
        }
    }

    #[test]
    fn char_round_trip() {
        for t in Trit::ALL {
            assert_eq!(Trit::from_char(t.to_char()), Some(t));
        }
        assert_eq!(Trit::from_char('x'), None);
    }

    #[test]
    fn sign_of_is_total() {
        assert_eq!(Trit::sign_of(i32::MIN), Trit::Neg);
        assert_eq!(Trit::sign_of(-1), Trit::Neg);
        assert_eq!(Trit::sign_of(0), Trit::Zero);
        assert_eq!(Trit::sign_of(1), Trit::Pos);
        assert_eq!(Trit::sign_of(i32::MAX), Trit::Pos);
    }

    #[test]
    fn neg_is_involutive_and_cycle_has_order_three() {
        for t in Trit::ALL {
            assert_eq!(t.neg().neg(), t);
            assert_eq!(t.cycle().cycle().cycle(), t);
        }
    }
}
