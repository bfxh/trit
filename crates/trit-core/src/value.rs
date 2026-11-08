//! 三值逻辑的核心值类型。
//!
//! 三值逻辑（ternary logic）中的每个值属于 `{0, 1, X}`，其中 `X` 表示
//! “未知 / 无关 / 不确定”。不同的三值代数对 `X` 的解释不同：
//!
//! - **Kleene 强三值逻辑**（K3）：`X` 是“不知道”，是格序的中点。
//! - **Łukasiewicz 三值模糊逻辑**（Ł3）：`X` 是“介于 0 与 1 之间的不确定值”。
//! - **Bochvar 弱三值逻辑**（B3）：`X` 是“毒值”，吸收一切运算。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 三值逻辑中的单个取值。
///
/// 顺序即枚举顺序，`Ord` 的派生保证 `False < Unknown < True`，
/// 方便用 `min` / `max` 表达格序的 `AND` / `OR`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Ternary {
    /// 假（0）。
    False,
    /// 未知（X）。
    Unknown,
    /// 真（1）。
    True,
}

/// 默认值是 `False`：二进制视图下「没有 unknowns」的自然选择。
impl Default for Ternary {
    fn default() -> Self {
        Self::False
    }
}

impl From<bool> for Ternary {
    fn from(value: bool) -> Self {
        Self::from_bool(value)
    }
}

/// 三值取反；在 Kleene / Ł3 / B3 三套语义下相同。
impl std::ops::Not for Ternary {
    type Output = Self;

    fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// **Kleene** 合取（`False ∧ X = False`）。Ł3 / B3 请走 [`crate::algebra::Algebra`]。
impl std::ops::BitAnd for Ternary {
    type Output = Self;

    fn bitand(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }
}

/// **Kleene** 析取（`True ∨ X = True`）。
impl std::ops::BitOr for Ternary {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

/// **经典** 异或：任一为 `X` 则 `X`（对应 `GateKind::XorClassic`）。
/// Ł3 的强加法（`X ⊕ X = 1`）不是这个语义，请用 `Algebra::Lukasiewicz`。
impl std::ops::BitXor for Ternary {
    type Output = Self;

    fn bitxor(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::True, Self::True) | (Self::False, Self::False) => Self::False,
            _ => Self::True,
        }
    }
}

impl Ternary {
    /// 逻辑常量假。
    pub const FALSE: Self = Self::False;
    /// 逻辑常量未知。
    pub const UNKNOWN: Self = Self::Unknown;
    /// 逻辑常量真。
    pub const TRUE: Self = Self::True;

    /// 三值的所有取值，按 `False < Unknown < True` 顺序。
    pub const ALL: [Self; 3] = [Self::False, Self::Unknown, Self::True];

    /// 三态序号：`False = 0`、`Unknown = 1`、`True = 2`（可用来索引 9 项 LUT）。
    pub const fn index(self) -> usize {
        // `repr(u8)` + 声明顺序（False < Unknown < True）⇒ 这个转换是免费的。
        self as usize
    }

    /// 由序号还原；`0/1/2` 之外回退到 `Unknown`（保守）。
    pub const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::False,
            1 => Self::Unknown,
            2 => Self::True,
            _ => Self::Unknown,
        }
    }

    /// `bool -> Ternary`：`true -> True`，`false -> False`（无分支 Select）。
    pub const fn from_bool(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }

    /// `Ternary -> Option<bool>`：`Unknown` 返回 `None`。
    pub const fn to_bool(self) -> Option<bool> {
        match self {
            Self::True => Some(true),
            Self::False => Some(false),
            Self::Unknown => None,
        }
    }

    /// 是否为二值（非 `Unknown`）。
    pub const fn is_binary(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// 数值形式：`False -> 0`，`Unknown -> 0.5`，`True -> 1`。
    ///
    /// 这是把 `X` 视为“0 与 1 之间的不确定值”时的自然映射，
    /// 也是 Łukasiewicz 强加法 / 强合取的定义基础。
    pub const fn as_f64(self) -> f64 {
        match self {
            Self::False => 0.0,
            Self::Unknown => 0.5,
            Self::True => 1.0,
        }
    }

    /// 是否**精确等于**假；`X` 返回 `false`。
    ///
    /// 这里不做任何折叠。若要把 `X` 当假/当真的二值化，请用
    /// [`to_bool_strict`](Self::to_bool_strict) / [`to_bool_loose`](Self::to_bool_loose)。
    pub const fn is_false(self) -> bool {
        matches!(self, Self::False)
    }

    /// 是否**精确等于**真；`X` 返回 `false`。
    ///
    /// 若要把 `X` 当真的二值化，请用 [`to_bool_loose`](Self::to_bool_loose)。
    pub const fn is_true(self) -> bool {
        matches!(self, Self::True)
    }

    /// 是否为未知值。
    pub const fn is_unknown(self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// 是否为确定值（`0` 或 `1`）。
    pub const fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// 三值 → 二值的**唯一收口**：把「`Unknown` 该怎么办」变成显式策略。
    ///
    /// 任何 `Ternary → bool` 的降级都应走这里，不要手写 `is_true()` / `match`：
    /// 手写会把「未知」静默折成某一边，且无法在审计时定位。
    pub const fn collapse(self, policy: BinaryCollapse) -> Option<bool> {
        match (self, policy) {
            (Self::True, _) => Some(true),
            (Self::False, _) => Some(false),
            (Self::Unknown, BinaryCollapse::ToFalse) => Some(false),
            (Self::Unknown, BinaryCollapse::ToTrue) => Some(true),
            (Self::Unknown, BinaryCollapse::Reject) => None,
        }
    }

    /// 同 [`collapse`](Self::collapse)，但策略为 `Reject` 时回落 `default`。
    pub const fn collapse_or(self, policy: BinaryCollapse, default: bool) -> bool {
        match self.collapse(policy) {
            Some(b) => b,
            None => default,
        }
    }

    /// 经典二值化：把 `X` 折成假。
    ///
    /// 等价于 `collapse(BinaryCollapse::ToFalse).unwrap()`。
    /// 新代码请直接用 [`collapse`](Self::collapse)，把策略写在调用点上。
    pub const fn to_bool_strict(self) -> bool {
        matches!(self, Self::True)
    }

    /// 经典二值化：把 `X` 折成真。
    ///
    /// 等价于 `collapse(BinaryCollapse::ToTrue).unwrap()`。
    /// 新代码请直接用 [`collapse`](Self::collapse)，把策略写在调用点上。
    pub const fn to_bool_loose(self) -> bool {
        !matches!(self, Self::False)
    }

    /// 解析单个字符：`0`/`f`/`F` → 假，`1`/`t`/`T` → 真，`x`/`X`/`?` → 未知。
    pub fn parse_char(c: char) -> Option<Self> {
        match c {
            '0' | 'f' | 'F' => Some(Self::False),
            '1' | 't' | 'T' => Some(Self::True),
            'x' | 'X' | '?' => Some(Self::Unknown),
            _ => None,
        }
    }

    /// 单字符表示：假为 `0`，未知为 `X`，真为 `1`。
    pub const fn as_char(self) -> char {
        match self {
            Self::False => '0',
            Self::Unknown => 'X',
            Self::True => '1',
        }
    }

    /// 单字符紧凑表示（用于真值表列头，未知记作小写 `x`）。
    pub const fn as_compact_char(self) -> char {
        match self {
            Self::False => '0',
            Self::Unknown => 'x',
            Self::True => '1',
        }
    }
}

impl fmt::Display for Ternary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::False => "0",
            Self::Unknown => "X",
            Self::True => "1",
        })
    }
}

/// 把 `Unknown` 折成布尔时的策略——三值 → 二值的**唯一**收口。
///
/// 三值纪律：任何降级都必须在这里选一个策略，而不是在调用点手写
/// `is_true()` / `match`（那样「未知」会被静默折成某一边，审计时无法定位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryCollapse {
    /// 把 `Unknown` 折成假（Kleene：只认「确定真」）。
    ToFalse,
    /// 把 `Unknown` 折成真（Bochvar：只排除「确定假」）。
    ToTrue,
    /// 拒绝折叠：遇到 `Unknown` 返回 `None`。
    Reject,
}

impl BinaryCollapse {
    /// 策略名（审计与日志用）。
    pub const fn name(self) -> &'static str {
        match self {
            Self::ToFalse => "to_false",
            Self::ToTrue => "to_true",
            Self::Reject => "reject",
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn index_round_trips_and_lut_stride_is_three_wide() {
        use Ternary::{False, True, Unknown};
        assert_eq!([False.index(), Unknown.index(), True.index()], [0, 1, 2]);
        for value in Ternary::ALL {
            assert_eq!(Ternary::from_index(value.index()), value);
        }
        // 9 项 LUT 用 `a*3 + b`，任意 (a, b) 都必须落到不同槽位，
        // 否则真值表会互相覆盖。
        let mut seen = std::collections::BTreeSet::new();
        for a in 0..3 {
            for b in 0..3 {
                assert!(seen.insert(a * 3 + b), "槽位重复：a={a} b={b}");
            }
        }
        assert_eq!(seen.len(), 9);
    }

    #[test]
    fn operators_use_kleene_and_classic_xor() {
        use Ternary::{False, True, Unknown};
        assert_eq!(!True, False);
        assert_eq!(!Unknown, Unknown);
        assert_eq!(False & Unknown, False);
        assert_eq!(True & Unknown, Unknown);
        assert_eq!(True | Unknown, True);
        assert_eq!(False | Unknown, Unknown);
        assert_eq!(True ^ True, False);
        assert_eq!(True ^ False, True);
        assert_eq!(Unknown ^ True, Unknown);
        assert_eq!(Ternary::default(), False);
        assert_eq!(Ternary::from(true), True);
    }

    use super::*;

    #[test]
    fn ordering_is_false_unknown_true() {
        assert!(Ternary::False < Ternary::Unknown);
        assert!(Ternary::Unknown < Ternary::True);
    }

    #[test]
    fn parse_char_round_trips() {
        for v in Ternary::ALL {
            assert_eq!(Ternary::parse_char(v.as_char()), Some(v));
        }
        assert_eq!(Ternary::parse_char('?'), Some(Ternary::Unknown));
        assert_eq!(Ternary::parse_char('z'), None);
    }

    #[test]
    fn numeric_mapping() {
        assert!((Ternary::False.as_f64() - 0.0).abs() < f64::EPSILON);
        assert!((Ternary::Unknown.as_f64() - 0.5).abs() < f64::EPSILON);
        assert!((Ternary::True.as_f64() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn binarization() {
        assert!(!Ternary::Unknown.to_bool_strict());
        assert!(Ternary::Unknown.to_bool_loose());
        assert!(Ternary::True.to_bool_strict());
        assert!(Ternary::False.is_known());
        assert!(!Ternary::Unknown.is_known());
    }

    #[test]
    fn collapse_policies_are_the_single_funnel() {
        use BinaryCollapse as C;
        assert_eq!(Ternary::Unknown.collapse(C::ToFalse), Some(false));
        assert_eq!(Ternary::Unknown.collapse(C::ToTrue), Some(true));
        assert_eq!(Ternary::Unknown.collapse(C::Reject), None);
        assert_eq!(Ternary::True.collapse(C::Reject), Some(true));
        assert_eq!(Ternary::False.collapse(C::Reject), Some(false));
        assert!(!Ternary::Unknown.collapse_or(C::Reject, false));
        assert!(Ternary::Unknown.collapse_or(C::Reject, true));
        assert!(Ternary::True.collapse_or(C::Reject, false));
        // 与旧 API 必须等价（迁移期不改语义）
        assert_eq!(
            Ternary::Unknown.collapse(C::ToFalse),
            Some(Ternary::Unknown.to_bool_strict())
        );
        assert_eq!(
            Ternary::Unknown.collapse(C::ToTrue),
            Some(Ternary::Unknown.to_bool_loose())
        );
        assert_eq!(C::ToFalse.name(), "to_false");
        assert_eq!(C::Reject.name(), "reject");
    }
}
