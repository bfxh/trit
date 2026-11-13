//! 循环与 `of` 的三值求值（接缝实现）。
//!
//! 语义依据 `docs/YARA-EXPR/01-SEMANTICS.md` §8–§9：零次迭代对
//! all/any/none 都是确定的假；循环体原值累加；出现 `Unknown` 时改用三值
//! 计数（`t`/`f`/`u`），并遵守 `of_result` 的判定。

use super::xval::{Val, int};

/// 求值后的量词（`N`/`Percent` 已经算成数）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuantVal {
    All,
    Any,
    None,
    N(i64),
    Percent(f64),
}

/// `all/any/none/N/percent` 的三值判定。
pub fn of_result(quant: QuantVal, t: i64, f: i64, u: i64, total: i64) -> Val {
    match quant {
        QuantVal::All => {
            if f > 0 {
                Val::Int(0)
            } else if u > 0 {
                Val::Unknown
            } else {
                Val::Int(1)
            }
        }
        QuantVal::None => {
            if t > 0 {
                Val::Int(0)
            } else if u > 0 {
                Val::Unknown
            } else {
                Val::Int(1)
            }
        }
        QuantVal::Any => {
            if t > 0 {
                Val::Int(1)
            } else if u > 0 {
                Val::Unknown
            } else {
                Val::Int(0)
            }
        }
        QuantVal::N(n) => {
            if t >= n {
                Val::Int(1)
            } else if t + u < n {
                Val::Int(0)
            } else {
                Val::Unknown
            }
        }
        QuantVal::Percent(p) => {
            if total == 0 {
                return Val::Int(0);
            }
            if u > 0 && t < total {
                return Val::Unknown;
            }
            int(i64::from((t as f64 / total as f64) * 100.0 >= p))
        }
    }
}

/// 对 `len` 项执行循环体；`body(i)` 返回第 i 项的原值。
pub fn run_loop<F>(quant: QuantVal, len: usize, mut body: F) -> Val
where
    F: FnMut(usize) -> Val,
{
    if len == 0 {
        // 官方：零次迭代 all/any/none/百分比全是确定的假。
        return Val::Int(0);
    }
    let mut count: i64 = 0;
    let mut t = 0i64;
    let mut f = 0i64;
    let mut u = 0i64;
    let mut saw_unknown = false;
    for i in 0..len {
        match body(i) {
            Val::Unknown => {
                u += 1;
                saw_unknown = true;
            }
            Val::Undef => f += 1,
            Val::Int(n) => {
                count = count.wrapping_add(n);
                t += i64::from(n != 0);
                f += i64::from(n == 0);
            }
            Val::Float(bits) => {
                count = count.wrapping_add(bits as i64);
                t += 1;
            }
            Val::Str(s) => {
                let n = i64::from(!s.is_empty());
                count = count.wrapping_add(n);
                t += n;
                f += 1 - n;
            }
            Val::Regex(_) => {
                count += 1;
                t += 1;
            }
        }
    }
    if saw_unknown {
        return of_result(quant, t, f, u, len as i64);
    }
    match quant {
        QuantVal::All => int(i64::from(count == len as i64)),
        QuantVal::Any => int(i64::from(count != 0)),
        QuantVal::None => int(i64::from(count == 0)),
        QuantVal::N(n) => int(i64::from(count >= n)),
        QuantVal::Percent(p) => int(i64::from((t as f64 / len as f64) * 100.0 >= p)),
    }
}
