//! 平衡三值标量与三值代数。
//!
//! 取值 `{-1, 0, +1}`。与逻辑三值 `{False, Unknown, True}` 的映射是
//! `-1 -> False`、`0 -> Unknown`、`+1 -> True`。

use trit_core::value::Ternary;

/// 平衡三值标量。
///
/// 不变量：内部字节恒为 `-1`、`0` 或 `+1`。用 [`Trit::new`] 或常量构造。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[repr(transparent)]
pub struct Trit(i8);

impl Trit {
    /// `-1`
    pub const NEG: Trit = Trit(-1);
    /// `0`
    pub const ZERO: Trit = Trit(0);
    /// `+1`
    pub const POS: Trit = Trit(1);

    /// 校验构造；越界返回 `None`。
    #[inline]
    pub const fn new(value: i8) -> Option<Self> {
        if value >= -1 && value <= 1 {
            Some(Trit(value))
        } else {
            None
        }
    }

    /// 不做校验的构造（内部用）。调用方保证取值合法。
    #[inline]
    pub const fn from_i8_unchecked(value: i8) -> Self {
        Trit(value)
    }

    /// 取回 `i8`。
    #[inline]
    pub const fn get(self) -> i8 {
        self.0
    }

    /// 三值取反。
    #[inline]
    pub const fn neg(self) -> Self {
        Trit(-self.0)
    }

    /// 三值乘法：精确，结果仍是三值（`±1 * ±1 = ±1`）。
    #[inline]
    pub const fn mul(self, other: Self) -> Self {
        Trit(self.0 * other.0)
    }

    /// 饱和三值加法：`(-1) + (-1)` 与 `(+1) + (+1)` 截断，不产生进位。
    #[inline]
    pub const fn add_sat(self, other: Self) -> Self {
        let s = self.0 + other.0;
        Trit(if s > 1 {
            1
        } else if s < -1 {
            -1
        } else {
            s
        })
    }

    /// Kleene 合取（格序下确界，等价于 `min`）。
    #[inline]
    pub const fn and(self, other: Self) -> Self {
        Trit(if self.0 < other.0 { self.0 } else { other.0 })
    }

    /// Kleene 析取（格序上确界，等价于 `max`）。
    #[inline]
    pub const fn or(self, other: Self) -> Self {
        Trit(if self.0 > other.0 { self.0 } else { other.0 })
    }

    /// Łukasiewicz 合取：`max(-1, a + b - 1)`。
    #[inline]
    pub const fn luk_and(self, other: Self) -> Self {
        let s = self.0 + other.0 - 1;
        Trit(if s < -1 {
            -1
        } else if s > 1 {
            1
        } else {
            s
        })
    }

    /// Łukasiewicz 析取：`min(1, a + b + 1)`。
    #[inline]
    pub const fn luk_or(self, other: Self) -> Self {
        let s = self.0 + other.0 + 1;
        Trit(if s < -1 {
            -1
        } else if s > 1 {
            1
        } else {
            s
        })
    }

    /// 实数取符号并截到三值。
    #[inline]
    pub fn sign_of(value: f32) -> Self {
        if value > 0.0 {
            Trit(1)
        } else if value < 0.0 {
            Trit(-1)
        } else {
            Trit(0)
        }
    }

    /// 逻辑三值 -> 平衡三值。
    #[inline]
    pub const fn from_ternary(value: Ternary) -> Self {
        match value {
            Ternary::False => Trit(-1),
            Ternary::Unknown => Trit(0),
            Ternary::True => Trit(1),
        }
    }

    /// 平衡三值 -> 逻辑三值。
    #[inline]
    pub const fn to_ternary(self) -> Ternary {
        match self.0 {
            -1 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        }
    }
}

impl std::ops::Neg for Trit {
    type Output = Trit;

    #[inline]
    fn neg(self) -> Trit {
        Trit::neg(self)
    }
}

impl From<Trit> for i8 {
    #[inline]
    fn from(value: Trit) -> i8 {
        value.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_is_exact_and_closed() {
        for a in [-1i8, 0, 1] {
            for b in [-1i8, 0, 1] {
                let r = Trit::new(a).unwrap().mul(Trit::new(b).unwrap());
                assert_eq!(r.get(), a * b);
            }
        }
    }

    #[test]
    fn add_saturates() {
        assert_eq!(Trit::POS.add_sat(Trit::POS).get(), 1);
        assert_eq!(Trit::NEG.add_sat(Trit::NEG).get(), -1);
        assert_eq!(Trit::POS.add_sat(Trit::NEG).get(), 0);
        assert_eq!(Trit::POS.add_sat(Trit::ZERO).get(), 1);
    }

    #[test]
    fn kleene_lattice_is_min_max() {
        let all = [Trit::NEG, Trit::ZERO, Trit::POS];
        for &a in &all {
            for &b in &all {
                assert_eq!(a.and(b).get(), a.get().min(b.get()));
                assert_eq!(a.or(b).get(), a.get().max(b.get()));
            }
        }
    }

    #[test]
    fn ternary_mapping_roundtrips() {
        for t in [Ternary::False, Ternary::Unknown, Ternary::True] {
            assert_eq!(Trit::from_ternary(t).to_ternary(), t);
        }
    }

    #[test]
    fn out_of_range_is_rejected() {
        assert!(Trit::new(2).is_none());
        assert!(Trit::new(-2).is_none());
    }
}
