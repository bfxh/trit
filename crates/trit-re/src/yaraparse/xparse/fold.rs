//! 编译期常量折叠——**只为复刻官方的编译错误**，不改写表达式树（求值
//! 永远走运行期语义）。探针 + `grammar.y` 定案的规则：
//! - 只有整数字面量及其运算结果是"常量"；`filesize`、`uintN()`、模块
//!   字段与常量（`pe.MACHINE_*`）、循环变量都不是。
//! - `+ - *`：两侧都是常量且溢出 → 编译错误（判别式照抄官方，`*` 那条
//!   偏保守：`2 * -4611686018427387904` 数学上没溢出也报错）。
//! - `\` `%`：右侧是常量 0 → 编译错误（左侧是不是常量无所谓）。
//! - 移位：计数是负常量 → 编译错误；计数是 ≥ 64 的常量 → 结果是常量 0
//!   （左侧不是常量也一样）。
//! - **官方 bug，照搬**：`a >> b` 的编译期值按 `a << b` 算（`2 >> 1` 被当
//!   成 4）。只影响后续的编译期检查，运行期右移是对的。
//! - `INT64_MIN \ -1` 两侧都是常量时官方编译器直接崩溃（SIGFPE）——
//!   这里拒收。
//! - 值恰为官方哨兵 `0xFFFABADAFABADAFF` 的折叠结果被官方当成"非常量"。

use crate::sigscan::expr::Arith;

const SENTINEL: i64 = 0xFFFA_BADA_FABA_DAFF_u64 as i64;

fn known(value: i64) -> Option<i64> {
    (value != SENTINEL).then_some(value)
}

pub(super) fn negate(value: Option<i64>) -> Option<i64> {
    value.and_then(|v| known(v.wrapping_neg()))
}

pub(super) fn invert(value: Option<i64>) -> Option<i64> {
    value.and_then(|v| known(!v))
}

/// 二元运算的编译期值；`Err` = 官方编译错误。
pub(super) fn binary(op: Arith, a: Option<i64>, b: Option<i64>) -> Result<Option<i64>, String> {
    let overflow = |sign: &str| match (a, b) {
        (Some(a), Some(b)) => format!("常量溢出（官方：integer overflow in \"{a} {sign} {b}\"）"),
        _ => String::new(),
    };
    let both = a.zip(b);
    let value = match op {
        Arith::Add => match both {
            Some((a, b)) if a.checked_add(b).is_none() => return Err(overflow("+")),
            pair => pair.map(|(a, b)| a + b),
        },
        Arith::Sub => match both {
            Some((a, b)) if a.checked_sub(b).is_none() => return Err(overflow("-")),
            pair => pair.map(|(a, b)| a - b),
        },
        Arith::Mul => match both {
            Some((a, b)) if b != 0 && a.wrapping_abs() > i64::MAX / b.wrapping_abs() => {
                return Err(overflow("*"));
            }
            pair => pair.map(|(a, b)| a.wrapping_mul(b)),
        },
        Arith::Div | Arith::Rem => {
            if b == Some(0) {
                return Err("除数是常量 0（官方：division by zero）".into());
            }
            match both {
                Some((i64::MIN, -1)) => {
                    return Err("INT64_MIN 除以 -1：官方编译器在此崩溃（SIGFPE），拒收".into());
                }
                Some((a, b)) if op == Arith::Div => Some(a / b),
                Some((a, b)) => Some(a % b),
                None => None,
            }
        }
        Arith::And => both.map(|(a, b)| a & b),
        Arith::Or => both.map(|(a, b)| a | b),
        Arith::Xor => both.map(|(a, b)| a ^ b),
        Arith::Shl | Arith::Shr => match b {
            Some(count) if count < 0 => {
                return Err("移位计数是负常量（官方：invalid operand）".into());
            }
            Some(count) if count >= 64 => Some(0),
            // 官方把 `>>` 的编译期值也按 `<<` 算
            _ => both.map(|(a, count)| a.wrapping_shl(count as u32)),
        },
    };
    Ok(value.and_then(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_checks_match_official_predicates() {
        assert!(binary(Arith::Add, Some(i64::MAX), Some(1)).is_err());
        assert_eq!(
            binary(Arith::Add, Some(i64::MAX), Some(-1)),
            Ok(Some(i64::MAX - 1))
        );
        assert!(binary(Arith::Sub, Some(-i64::MAX), Some(2)).is_err());
        assert_eq!(
            binary(Arith::Sub, Some(-i64::MAX), Some(1)),
            Ok(Some(i64::MIN))
        );
        assert!(binary(Arith::Mul, Some(3_037_000_500), Some(3_037_000_500)).is_err());
        // 官方判别式偏保守：恰好等于 INT64_MIN 的乘积也报错
        assert!(binary(Arith::Mul, Some(2), Some(-4_611_686_018_427_387_904)).is_err());
        // llabs(INT64_MIN) 回绕成负数 → 左侧是 MIN 时判别式放行
        assert_eq!(
            binary(Arith::Mul, Some(i64::MIN), Some(-1)),
            Ok(Some(i64::MIN))
        );
        assert!(binary(Arith::Mul, Some(-1), Some(i64::MIN)).is_err());
        // 非常量参与：不报错、结果非常量（运行期回绕）
        assert_eq!(binary(Arith::Add, Some(i64::MAX), None), Ok(None));
    }

    #[test]
    fn division_and_shift_errors() {
        assert!(binary(Arith::Div, None, Some(0)).is_err());
        assert!(binary(Arith::Rem, Some(1), Some(0)).is_err());
        assert_eq!(binary(Arith::Div, None, None), Ok(None));
        assert_eq!(binary(Arith::Div, Some(-7), Some(2)), Ok(Some(-3)));
        assert!(binary(Arith::Div, Some(i64::MIN), Some(-1)).is_err());
        assert!(binary(Arith::Shl, Some(1), Some(-1)).is_err());
        assert!(binary(Arith::Shr, None, Some(-1)).is_err());
        // 计数 ≥ 64：常量 0，哪怕左侧不是常量 → 之后当除数会报除零
        assert_eq!(binary(Arith::Shl, None, Some(64)), Ok(Some(0)));
        // 官方 bug：>> 的编译期值按 << 算（2 >> 1 记成 4、1 >> 1 记成 2）
        assert_eq!(binary(Arith::Shr, Some(2), Some(1)), Ok(Some(4)));
        assert_eq!(binary(Arith::Shr, Some(1), Some(1)), Ok(Some(2)));
        assert_eq!(negate(Some(i64::MIN)), Some(i64::MIN));
        assert_eq!(invert(Some(-1)), Some(0));
    }
}
