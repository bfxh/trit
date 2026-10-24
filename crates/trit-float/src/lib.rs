//! 平衡三进制浮点实验层（`publish = false`，不属于稳定 API）。
//!
//! 表示：`value = mantissa · 3^exponent`，尾数用平衡三进制整数（数字 `{-1,0,1}`），
//! 规格化后最高位是 `±1`，有效位数不超过 `precision`。
//!
//! 舍入：规约到 `precision` 位时，在相邻两个指数上各取「最近的合法尾数」（`|尾数| ≤ (3^p-1)/2`），
//! 取误差更小者；误差相同取更大指数（远离零）。合法尾数集合是离散的，直接取整再 clamp，
//! 不能靠逐位截断（`5` 在 p=1 时最近是 `3`，截断进位会得到 `9`）。
//!
//! 关键结论（`trit-float stats` 可复算）：同样**存储比特**下，三值浮点的相对误差上界约是二进制的
//! **3 倍**（`3^{1-p}` vs `2^{-B}`，其中 `B = p·log2 3`）——基数换了，信息量没变，白丢约 1.58 bit。

#![forbid(unsafe_code)]

/// `log2(3)`。
pub const LOG2_3: f64 = 1.584_962_500_721_156_2;

/// 精度下限。
pub const MIN_PRECISION: u32 = 1;
/// 精度上限（尾数留在 `i128` 的乘法安全区）。
pub const MAX_PRECISION: u32 = 40;

/// 一个平衡三进制浮点数。
#[derive(Debug, Clone, Copy)]
pub struct TritFloat {
    mantissa: i128,
    exponent: i32,
    precision: u32,
}

impl TritFloat {
    /// 用 `mantissa · 3^exponent` 构造并规格化；`precision` 会被夹到 `[1, 40]`。
    pub fn new(mantissa: i128, exponent: i32, precision: u32) -> Self {
        let precision = precision.clamp(MIN_PRECISION, MAX_PRECISION);
        let (mantissa, exponent) = normalize(mantissa, exponent, precision);
        Self {
            mantissa,
            exponent,
            precision,
        }
    }

    /// 正零。
    pub fn zero(precision: u32) -> Self {
        Self::new(0, 0, precision)
    }

    /// 从 `f64` 转换；`NaN` / `±Inf` 返回 `None`。
    pub fn from_f64(value: f64, precision: u32) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let precision = precision.clamp(MIN_PRECISION, MAX_PRECISION);
        if value == 0.0 {
            return Some(Self::zero(precision));
        }
        let sign = if value.is_sign_negative() { -1i128 } else { 1 };
        let mut x = value.abs();
        let mut exponent = 0i32;
        while x >= 1.5 {
            x /= 3.0;
            exponent += 1;
        }
        while x < 0.5 {
            x *= 3.0;
            exponent -= 1;
        }
        let scaled = x * 3f64.powi(precision as i32);
        let mantissa = (scaled.round() as i128) * sign;
        Some(Self::new(mantissa, exponent - precision as i32, precision))
    }

    /// 转回 `f64`（可能溢出为 `±Inf` 或下溢为 `0`）。
    pub fn to_f64(self) -> f64 {
        self.mantissa as f64 * 3f64.powi(self.exponent)
    }

    /// 加法实现见 [`std::ops::Add`]。
    fn plus(self, other: Self) -> Self {
        let precision = self.precision.max(other.precision);
        // 对齐到**较小**指数：把大指数那项的尾数乘 3^shift（精确），避免除法的精度损失。
        let (high, low, high_exp, low_exp) = if self.exponent >= other.exponent {
            (self.mantissa, other.mantissa, self.exponent, other.exponent)
        } else {
            (other.mantissa, self.mantissa, other.exponent, self.exponent)
        };
        // i64 里做差：i32 极值相减本身会溢出。
        let shift = i64::from(high_exp) - i64::from(low_exp);
        if shift > 76 {
            // 差值超过 i128 的安全范围，较小项相对影响 < 3^-76，直接丢弃。
            return Self::new(high, high_exp, precision);
        }
        // 对齐同样可能溢出（大尾数 × 3^shift）。此时较小项低于任何精度的半个 ulp，
        // 同样丢弃——不允许 saturating 静默改值（`x + 0` 必须还是 `x`）。
        let Some(boosted) = high.checked_mul(3i128.pow(shift as u32)) else {
            return Self::new(high, high_exp, precision);
        };
        Self::new(boosted.saturating_add(low), low_exp, precision)
    }

    /// 乘法实现见 [`std::ops::Mul`]。
    fn times(self, other: Self) -> Self {
        let precision = self.precision.max(other.precision);
        Self::new(
            self.mantissa.saturating_mul(other.mantissa),
            self.exponent.saturating_add(other.exponent),
            precision,
        )
    }

    /// 取反实现见 [`std::ops::Neg`]。
    fn flipped(self) -> Self {
        Self {
            mantissa: self.mantissa.saturating_neg(),
            ..self
        }
    }

    /// 精度（有效三进制位数上限）。
    pub fn precision(self) -> u32 {
        self.precision
    }

    /// 尾数。
    pub fn mantissa(self) -> i128 {
        self.mantissa
    }

    /// 指数。
    pub fn exponent(self) -> i32 {
        self.exponent
    }

    /// 实际有效位数。
    pub fn significant_digits(self) -> u32 {
        digit_count(self.mantissa)
    }

    /// 这套精度占多少比特（`precision · log2 3`）。
    pub fn bits(self) -> f64 {
        self.precision as f64 * LOG2_3
    }

    /// 是否为零。
    pub fn is_zero(self) -> bool {
        self.mantissa == 0
    }
}

impl std::ops::Add for TritFloat {
    type Output = Self;
    /// 对齐到较小指数后相加，再按精度舍入。
    ///
    /// 指数差超过 76 时较小项低于 `i128` 的安全范围，直接丢弃（相对影响 `< 3^-76`）。
    fn add(self, other: Self) -> Self {
        self.plus(other)
    }
}

impl std::ops::Mul for TritFloat {
    type Output = Self;
    /// 尾数相乘、指数相加，再按精度舍入。
    fn mul(self, other: Self) -> Self {
        self.times(other)
    }
}

impl std::ops::Neg for TritFloat {
    type Output = Self;
    fn neg(self) -> Self {
        self.flipped()
    }
}

/// 相对误差上界的理论值：`3^{1-p}`。
pub fn relative_error_bound(precision: u32) -> f64 {
    3f64.powi(1 - precision as i32)
}

fn normalize(mut mantissa: i128, mut exponent: i32, precision: u32) -> (i128, i32) {
    if mantissa == 0 {
        return (0, 0);
    }
    while mantissa % 3 == 0 {
        mantissa /= 3;
        exponent = exponent.saturating_add(1);
    }
    let digits = digit_count(mantissa);
    if digits > precision {
        // 复审 M07：一次舍入到「precision 位平衡尾数」的最近值，而不是逐位截断。
        // 逐位截断会把 5（p=1）带成 9，而最近的是 3。
        let shift = digits - precision;
        let limit = mantissa_limit(precision);
        let mut best: Option<(u128, i128, u32)> = None; // (误差, 尾数, 丢掉的位数)
        for dropped in shift.saturating_sub(1)..=shift {
            let divisor = pow3(dropped);
            let rounded = round_quotient(mantissa, divisor).clamp(-limit, limit);
            if rounded == 0 {
                continue;
            }
            let error = distance(mantissa, rounded, divisor);
            // 误差相同取更大指数（远离零），与二进制浮点的进位习惯一致。
            let better = match best {
                None => true,
                Some((best_error, _, best_dropped)) => {
                    error < best_error || (error == best_error && dropped > best_dropped)
                }
            };
            if better {
                best = Some((error, rounded, dropped));
            }
        }
        let (_, rounded, dropped) = best.unwrap_or((0, limit, shift));
        mantissa = rounded;
        exponent = exponent.saturating_add(dropped as i32);
    }
    while mantissa != 0 && mantissa % 3 == 0 {
        mantissa /= 3;
        exponent = exponent.saturating_add(1);
    }
    (mantissa, exponent)
}

/// `p` 位平衡三进制尾数的绝对值上界 `(3^p - 1) / 2`。
fn mantissa_limit(precision: u32) -> i128 {
    let power = 3i128.checked_pow(precision).unwrap_or(i128::MAX);
    (power - 1) / 2
}

/// `3^exponent`；超出 `i128` 时饱和（调用方只在可表示区间内使用）。
fn pow3(exponent: u32) -> i128 {
    3i128.checked_pow(exponent).unwrap_or(i128::MAX)
}

/// 最接近 `value / divisor` 的整数（半步向上），不做 clamp。
fn round_quotient(value: i128, divisor: i128) -> i128 {
    let quotient = value.div_euclid(divisor);
    let remainder = value.rem_euclid(divisor);
    if remainder == 0 {
        return quotient;
    }
    let twice = (remainder as u128) * 2;
    let bound = divisor as u128;
    if twice >= bound {
        quotient + 1
    } else {
        quotient
    }
}

/// `|value - rounded * divisor|`；溢出时返回 `u128::MAX`（该候选随后被淘汰）。
fn distance(value: i128, rounded: i128, divisor: i128) -> u128 {
    match rounded.checked_mul(divisor) {
        Some(product) => match value.checked_sub(product) {
            Some(diff) => diff.unsigned_abs(),
            None => u128::MAX,
        },
        None => u128::MAX,
    }
}

/// 平衡三进制 `{−1,0,1}` 表示 `|value|` 需要的**最少位数**。
fn digit_count(value: i128) -> u32 {
    // unsigned_abs：i128::MIN 的 |x| 不存在于 i128，abs() 会 panic。
    let magnitude = value.unsigned_abs();
    if magnitude == 0 {
        return 0;
    }
    let mut digits = 0u32;
    let mut limit = 0u128; // range(0) = 0
    loop {
        digits += 1;
        // range(n+1) = 3 * range(n) + 1
        limit = limit.saturating_mul(3).saturating_add(1);
        if magnitude <= limit {
            return digits;
        }
    }
}

/// 相等与全序都按**数值**（`mantissa · 3^exponent`）判定，不看 `precision`：
/// `from_f64(1.0, 10) == from_f64(1.0, 20)`。规格化后的 `(mantissa, exponent)`
/// 是数值的规范形（尾数不再被 3 整除），逐字段比较就是数值比较。
impl PartialEq for TritFloat {
    fn eq(&self, other: &Self) -> bool {
        self.mantissa == other.mantissa && self.exponent == other.exponent
    }
}

impl Eq for TritFloat {}

impl PartialOrd for TritFloat {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TritFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => {
                return if other.mantissa > 0 {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, true) => {
                return if self.mantissa > 0 {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            _ => {}
        }
        if self.exponent == other.exponent {
            return self.mantissa.cmp(&other.mantissa);
        }
        let shift = i64::from(self.exponent) - i64::from(other.exponent);
        if shift > 0 {
            align_cmp(self.mantissa, shift, other.mantissa)
        } else {
            align_cmp(other.mantissa, -shift, self.mantissa).reverse()
        }
    }
}

/// `high · 3^shift` 与 `low` 的大小；对齐溢出说明前者超出 i128 表示范围，
/// 必大于后者（`low` 本身是 i128），按 `high` 的符号判定。
fn align_cmp(high: i128, shift: i64, low: i128) -> std::cmp::Ordering {
    let boosted = if shift > 80 {
        None
    } else {
        3i128
            .checked_pow(shift as u32)
            .and_then(|power| high.checked_mul(power))
    };
    match boosted {
        Some(boosted) => boosted.cmp(&low),
        None if high > 0 => std::cmp::Ordering::Greater,
        None => std::cmp::Ordering::Less,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_integers_round_trip_exactly() {
        for value in [0i64, 1, -1, 2, -2, 3, 9, -27, 13, 243] {
            let float = TritFloat::from_f64(value as f64, 20).unwrap();
            assert_eq!(float.to_f64(), value as f64, "{value}");
        }
    }

    #[test]
    fn arithmetic_is_exact_when_digits_fit() {
        let one = TritFloat::from_f64(1.0, 20).unwrap();
        let two = TritFloat::from_f64(2.0, 20).unwrap();
        let three = TritFloat::from_f64(3.0, 20).unwrap();
        assert_eq!((one + two).to_f64(), 3.0);
        assert_eq!((three * three).to_f64(), 9.0);
        assert_eq!((-one).to_f64(), -1.0);
        // 指数不同也要对：1 + 3 = 4。
        assert_eq!((one + three).to_f64(), 4.0);
    }

    #[test]
    fn rounding_is_nearest_in_balanced_ternary() {
        // 11 = 102_3，位数 3 > 2：最近的是 12（距离 1），不是 9（距离 2）。
        let rounded = TritFloat::new(11, 0, 2);
        assert_eq!(rounded.to_f64(), 12.0);
        assert!(rounded.significant_digits() <= 2);
    }

    #[test]
    fn trailing_zeros_are_stripped() {
        let float = TritFloat::new(9, 0, 2);
        assert_eq!(float.significant_digits(), 1);
        assert_eq!(float.to_f64(), 9.0);
    }

    #[test]
    fn relative_error_stays_under_the_bound() {
        for precision in [4u32, 6, 8] {
            let mut x = 0.37f64;
            for _ in 0..50 {
                let float = TritFloat::from_f64(x, precision).unwrap();
                let error = ((float.to_f64() - x) / x).abs();
                assert!(
                    error <= relative_error_bound(precision),
                    "p={precision} x={x} error={error}"
                );
                x = (x * 1.7 + 0.3) % 10.0;
            }
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        assert!(TritFloat::from_f64(f64::NAN, 8).is_none());
        assert!(TritFloat::from_f64(f64::INFINITY, 8).is_none());
    }

    #[test]
    fn precision_is_clamped() {
        let float = TritFloat::new(1, 0, 0);
        assert_eq!(float.precision(), MIN_PRECISION);
        let float = TritFloat::new(1, 0, 99);
        assert_eq!(float.precision(), MAX_PRECISION);
    }

    #[test]
    fn adding_zero_returns_the_operand_unchanged() {
        // 大指数 + 0 曾被 saturating_mul 静默改值（相对误差 ~72%）；必须原样返回。
        let x = TritFloat::new(1000, 75, 40);
        let sum = x + TritFloat::zero(40);
        assert_eq!(sum, x);
        assert_eq!(sum.mantissa(), 1000);
        assert_eq!(sum.exponent(), 75);
        // 对齐溢出时小项低于半个 ulp：丢弃而不是饱和改值。
        let y = x + TritFloat::new(1, 0, 40);
        assert_eq!(y, x);
    }

    #[test]
    fn extreme_mantissa_and_exponent_do_not_panic() {
        // digit_count 曾对 i128::MIN 取 abs() 直接 panic；现在规格化到 40 位。
        let float = TritFloat::new(i128::MIN, 0, 40);
        assert!(float.to_f64().is_finite());
        assert!(float.mantissa().abs() < 3i128.pow(40));
        // 指数加减饱和而不是溢出 panic。
        let huge = TritFloat::new(1, i32::MAX, 5) * TritFloat::new(1, 5, 5);
        assert_eq!(huge.exponent(), i32::MAX);
        let stripped = TritFloat::new(3, i32::MAX, 5);
        assert_eq!(stripped.exponent(), i32::MAX);
        let tiny = TritFloat::new(1, i32::MIN, 5) + TritFloat::new(1, i32::MAX, 5);
        assert_eq!(tiny.exponent(), i32::MAX);
        // 取反对极端尾数饱和而不是 panic。
        let neg = -TritFloat::new(i128::MAX, 0, 40);
        assert!(neg.mantissa() < 0);
    }

    #[test]
    fn equality_and_order_are_numeric_not_representational() {
        // 相等看数值不看 precision。
        assert_eq!(
            TritFloat::from_f64(1.0, 10).unwrap(),
            TritFloat::from_f64(1.0, 20).unwrap()
        );
        assert_ne!(TritFloat::new(2, 0, 5), TritFloat::new(2, 1, 5));
        // 数值全序：跨精度、跨指数、含零与负数。
        let mut values = [
            TritFloat::from_f64(-9.0, 6).unwrap(),
            TritFloat::zero(20),
            TritFloat::from_f64(2.5, 8).unwrap(),
            TritFloat::from_f64(4.0, 2).unwrap(),
            TritFloat::from_f64(100.0, 30).unwrap(),
        ];
        values.sort();
        let sorted: Vec<f64> = values.iter().map(|value| value.to_f64()).collect();
        assert!(
            sorted.windows(2).all(|pair| pair[0] <= pair[1]),
            "{sorted:?}"
        );
        assert!(TritFloat::from_f64(2.0, 5).unwrap() < TritFloat::from_f64(3.0, 5).unwrap());
        assert!(TritFloat::from_f64(-3.0, 5).unwrap() < TritFloat::from_f64(-2.0, 5).unwrap());
        // 对齐溢出路径：3^100 远超 i128 尾数范围，符号定序。
        assert!(TritFloat::new(1, 100, 5) > TritFloat::new(i128::MAX, 0, 5));
        assert!(TritFloat::new(-1, 100, 5) < TritFloat::new(-i128::MAX, 0, 5));
        // 指数差直接超过 80（3^80 已是 i128 内最大幂）：不必算，符号定序。
        assert!(TritFloat::new(1, 200, 5) > TritFloat::new(2, 0, 5));
        assert!(TritFloat::new(-1, 200, 5) < TritFloat::new(-2, 0, 5));
        // 尾数满 40 位再对齐 3^41：乘积超出 i128，走 checked 的 None 分支。
        let big = 3i128.pow(40) - 1;
        assert!(TritFloat::new(big, 41, 40) > TritFloat::new(1, 0, 5));
        assert!(TritFloat::new(-big, 41, 40) < TritFloat::new(-1, 0, 5));
    }

    #[test]
    fn precision_bounds_follow_balanced_digits() {
        // 复审 M07：1 位平衡尾数的合法值是 ±1·3^e，2 的最近表示是 3，不能保持尾数 2。
        let two = TritFloat::new(2, 0, 1);
        assert_eq!(two.significant_digits(), 1);
        assert_eq!((two.mantissa(), two.exponent()), (1, 1));
        // 5 的最近 1 位表示是 3（误差 2）；「取整后进位」的 9 误差是 4。
        let five = TritFloat::new(5, 0, 1);
        assert_eq!((five.mantissa(), five.exponent()), (1, 1));
        // 8 的最近 1 位表示是 9。
        let eight = TritFloat::new(8, 0, 1);
        assert_eq!((eight.mantissa(), eight.exponent()), (1, 2));
        // 负数对称。
        let minus_two = TritFloat::new(-2, 0, 1);
        assert_eq!((minus_two.mantissa(), minus_two.exponent()), (-1, 1));
        // 精度足够时原样保留。
        let exact = TritFloat::new(2, 0, 5);
        assert_eq!((exact.mantissa(), exact.exponent()), (2, 0));
        assert_eq!(exact.significant_digits(), 2);
    }

    #[test]
    fn rounding_is_nearest_over_legal_mantissas() {
        for precision in 1..=3u32 {
            let limit = (3i128.pow(precision) - 1) / 2;
            for value in -300i128..=300 {
                let number = TritFloat::new(value, 0, precision);
                let mantissa = number.mantissa();
                let exponent = number.exponent();
                assert!(mantissa.abs() <= limit, "尾数越界 {mantissa}");
                assert!(number.significant_digits() <= precision);
                let got = mantissa * 3i128.pow(exponent.max(0) as u32);
                let got_error = (value - got).unsigned_abs();
                // 穷举合法候选（指数 0..=4），确认没有更近的表示。
                let mut best = u128::MAX;
                for exp in 0..=4u32 {
                    let unit = 3i128.pow(exp);
                    for digit in -limit..=limit {
                        let error = (value - digit * unit).unsigned_abs();
                        if error < best {
                            best = error;
                        }
                    }
                }
                assert!(
                    got_error <= best,
                    "{value} @ p={precision}: {got_error} > {best}"
                );
            }
        }
    }
}
