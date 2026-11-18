//! 整数算术（int64，探针定案）：`+ - *` 运行期回绕；`\` `%` 除零或
//! `INT64_MIN \ -1` = 确定的未定义；移位计数 < 0 = 未定义、≥ 64 = 0，
//! `>>` 是算术右移。**编译期**常量折叠的报错规则在解析器
//! （`yaraparse::xparse::fold`）——那里才有"是不是常量"的信息。

use super::Arith;
use super::eval::V;

/// 官方的 undefined 哨兵（`YR_UNDEFINED`）。官方虚拟机里**任何**恰好等于它
/// 的整数都被当成未定义——包括算出来的（探针：
/// `filesize - filesize - 0x5452505452501` 未定义）。求值器照搬，否则
/// 攻击者能用文件内容凑出这个值制造分歧。
pub(super) const UNDEFINED: i64 = 0xFFFA_BADA_FABA_DAFF_u64 as i64;

/// 两个**已定义**整数上的二元运算。
pub(super) fn arith<'a>(op: Arith, a: i64, b: i64) -> V<'a> {
    match op {
        Arith::Add => V::Int(a.wrapping_add(b)),
        Arith::Sub => V::Int(a.wrapping_sub(b)),
        Arith::Mul => V::Int(a.wrapping_mul(b)),
        Arith::Div => a.checked_div(b).map_or(V::Undef, V::Int),
        Arith::Rem => a.checked_rem(b).map_or(V::Undef, V::Int),
        Arith::And => V::Int(a & b),
        Arith::Or => V::Int(a | b),
        Arith::Xor => V::Int(a ^ b),
        Arith::Shl => shift(b, || a.wrapping_shl(b as u32)),
        Arith::Shr => shift(b, || a.wrapping_shr(b as u32)),
    }
}

fn shift<'a>(count: i64, apply: impl FnOnce() -> i64) -> V<'a> {
    if count < 0 {
        V::Undef
    } else if count >= 64 {
        V::Int(0)
    } else {
        V::Int(apply())
    }
}

/// `uintN`/`intN` 读取：`data[at..at+size]`，越界（含负偏移）= 未定义。
pub(super) fn read<'a>(data: &[u8], at: i64, size: u8, signed: bool, big_endian: bool) -> V<'a> {
    let size = usize::from(size);
    let Ok(start) = usize::try_from(at) else {
        return V::Undef;
    };
    let Some(bytes) = start.checked_add(size).and_then(|end| data.get(start..end)) else {
        return V::Undef;
    };
    let mut raw = 0u64;
    for (i, byte) in bytes.iter().enumerate() {
        let shift = if big_endian { size - 1 - i } else { i };
        raw |= u64::from(*byte) << (8 * shift);
    }
    if signed {
        let unused = 64 - 8 * size as u32;
        V::Int(((raw << unused) as i64) >> unused)
    } else {
        V::Int(raw as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: V) -> Option<i64> {
        match v {
            V::Int(i) => Some(i),
            _ => None,
        }
    }

    #[test]
    fn arithmetic_matches_probed_int64_semantics() {
        // 回绕（探针：9223372036854775807 + filesize < 0 命中）
        assert_eq!(int(arith(Arith::Add, i64::MAX, 13)), Some(i64::MIN + 12));
        assert_eq!(int(arith(Arith::Mul, 13, i64::MAX)), Some(i64::MAX - 12));
        // 截断除法 / 余数随被除数符号
        assert_eq!(int(arith(Arith::Div, -7, 2)), Some(-3));
        assert_eq!(int(arith(Arith::Rem, -7, 3)), Some(-1));
        // 除零与 MIN \ -1 = 未定义（不是 0，更不是 panic）
        assert!(matches!(arith(Arith::Div, 5, 0), V::Undef));
        assert!(matches!(arith(Arith::Rem, 5, 0), V::Undef));
        assert!(matches!(arith(Arith::Div, i64::MIN, -1), V::Undef));
        assert!(matches!(arith(Arith::Rem, i64::MIN, -1), V::Undef));
    }

    #[test]
    fn shifts_follow_probed_edges() {
        assert_eq!(int(arith(Arith::Shl, 1, 63)), Some(i64::MIN));
        assert_eq!(int(arith(Arith::Shl, 1, 64)), Some(0));
        assert_eq!(int(arith(Arith::Shr, -8, 1)), Some(-4));
        assert_eq!(int(arith(Arith::Shr, -1, 63)), Some(-1));
        // ≥ 64 一律 0（连负数也是——官方不做符号延拓）
        assert_eq!(int(arith(Arith::Shr, -1, 64)), Some(0));
        assert!(matches!(arith(Arith::Shl, 1, -1), V::Undef));
        assert!(matches!(arith(Arith::Shr, 1, -1), V::Undef));
    }

    #[test]
    fn reads_are_bounds_checked_and_sign_aware() {
        let data = b"hello world\x00\xff";
        assert_eq!(int(read(data, 12, 1, false, false)), Some(0xff));
        assert_eq!(int(read(data, 12, 1, true, false)), Some(-1));
        assert_eq!(int(read(data, 11, 2, false, false)), Some(0xff00));
        assert_eq!(int(read(data, 11, 2, false, true)), Some(0x00ff));
        assert_eq!(int(read(data, 11, 2, true, false)), Some(-256));
        assert_eq!(int(read(data, 9, 4, false, false)), Some(0xff00_646c));
        assert_eq!(int(read(data, 9, 4, false, true)), Some(0x6c64_00ff));
        assert!(matches!(read(data, 13, 1, false, false), V::Undef));
        assert!(matches!(read(data, 12, 2, false, false), V::Undef));
        assert!(matches!(read(data, -1, 1, false, false), V::Undef));
        assert!(matches!(read(data, i64::MAX, 4, false, false), V::Undef));
    }
}
