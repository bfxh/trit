//! 三值求值原语（接缝实现）：`Val`、哨兵归一、算术/比较/布尔。
//!
//! 语义依据 `docs/YARA-EXPR/01-SEMANTICS.md` §2–§4；严格运算表（表 1）、
//! 一元与裁决（表 2）。`Undef` = 官方 undefined（确定无值），`Unknown` =
//! 证据缺失；两者一路分开，到规则裁决才合并。

use std::cmp::Ordering;

use super::CmpOp;
use super::expr::{BinOp, CmpKind, RegexLit};

/// 官方 undefined 哨兵：整数结果恰等于它时按 undefined 处理。
pub const SENTINEL: i64 = 0xFFFA_BADA_FABA_DAFFu64 as i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Val {
    Int(i64),
    Float(u64),
    Str(Vec<u8>),
    Regex(RegexLit),
    Undef,
    Unknown,
}

/// 整数归一：哨兵 → Undef。
pub fn int(v: i64) -> Val {
    if v == SENTINEL {
        Val::Undef
    } else {
        Val::Int(v)
    }
}

/// 表 1 的 Undef/Unknown 传播；没有特殊项时返回 `None`。
fn strict(a: &Val, b: &Val) -> Option<Val> {
    match (a, b) {
        (Val::Undef, _) => Some(Val::Undef),
        (Val::Unknown, Val::Undef) => Some(Val::Undef),
        (Val::Unknown, _) | (_, Val::Unknown) => Some(Val::Unknown),
        _ => None,
    }
}

fn cmp_int(op: CmpOp, a: i64, b: i64) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}

/// 字符串比较：按**有符号字节**逐个比，前缀相同则短者小。
pub fn ss_compare(a: &[u8], b: &[u8]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = (*x as i8).cmp(&(*y as i8));
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

fn cmp_float(op: CmpOp, a: f64, b: f64) -> bool {
    match op {
        // 官方：== 是 |a-b| < DBL_EPSILON，!= 是 >=
        CmpOp::Eq => (a - b).abs() < f64::EPSILON,
        CmpOp::Ne => (a - b).abs() >= f64::EPSILON,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}

pub fn cmp(op: CmpOp, kind: CmpKind, a: Val, b: Val) -> Val {
    // 浮点 `<` 的官方例外：任一侧 undefined 时结果是 0（有定义）。
    if kind == CmpKind::Float
        && op == CmpOp::Lt
        && (matches!(a, Val::Undef) || matches!(b, Val::Undef))
    {
        return Val::Int(0);
    }
    if let Some(v) = strict(&a, &b) {
        return v;
    }
    match (kind, a, b) {
        (CmpKind::Int, Val::Int(x), Val::Int(y)) => Val::Int(i64::from(cmp_int(op, x, y))),
        (CmpKind::Float, x, y) => {
            let fx = as_float(&x);
            let fy = as_float(&y);
            match (fx, fy) {
                (Some(x), Some(y)) => Val::Int(i64::from(cmp_float(op, x, y))),
                _ => Val::Undef,
            }
        }
        (CmpKind::Str, Val::Str(x), Val::Str(y)) => {
            let o = ss_compare(&x, &y);
            Val::Int(i64::from(match op {
                CmpOp::Eq => o == Ordering::Equal,
                CmpOp::Ne => o != Ordering::Equal,
                CmpOp::Lt => o == Ordering::Less,
                CmpOp::Le => o != Ordering::Greater,
                CmpOp::Gt => o == Ordering::Greater,
                CmpOp::Ge => o != Ordering::Less,
            }))
        }
        _ => Val::Undef,
    }
}

fn as_float(v: &Val) -> Option<f64> {
    match v {
        Val::Int(n) => Some(*n as f64),
        Val::Float(bits) => Some(f64::from_bits(*bits)),
        _ => None,
    }
}

pub fn bin(op: BinOp, float: bool, a: Val, b: Val) -> Val {
    if let Some(v) = strict(&a, &b) {
        return v;
    }
    if float || matches!(a, Val::Float(_)) || matches!(b, Val::Float(_)) {
        let (Some(x), Some(y)) = (as_float(&a), as_float(&b)) else {
            return Val::Undef;
        };
        return match op {
            BinOp::Add => Val::Float((x + y).to_bits()),
            BinOp::Sub => Val::Float((x - y).to_bits()),
            BinOp::Mul => Val::Float((x * y).to_bits()),
            BinOp::Div => Val::Float((x / y).to_bits()),
            _ => Val::Undef,
        };
    }
    let (Val::Int(x), Val::Int(y)) = (a, b) else {
        return Val::Undef;
    };
    match op {
        BinOp::Add => int(x.wrapping_add(y)),
        BinOp::Sub => int(x.wrapping_sub(y)),
        BinOp::Mul => int(x.wrapping_mul(y)),
        BinOp::Div => {
            if y == 0 || (x == i64::MIN && y == -1) {
                Val::Undef
            } else {
                int(x / y)
            }
        }
        BinOp::Mod => {
            if y == 0 || (x == i64::MIN && y == -1) {
                Val::Undef
            } else {
                int(x % y)
            }
        }
        BinOp::BitAnd => int(x & y),
        BinOp::BitOr => int(x | y),
        BinOp::BitXor => int(x ^ y),
        BinOp::Shl | BinOp::Shr => {
            if y < 0 {
                Val::Undef
            } else if y >= 64 {
                Val::Int(0)
            } else if op == BinOp::Shl {
                int(x.wrapping_shl(y as u32))
            } else {
                int(x.wrapping_shr(y as u32))
            }
        }
    }
}

pub fn neg(_float: bool, v: Val) -> Val {
    match v {
        Val::Int(n) => int(n.wrapping_neg()),
        Val::Float(bits) => Val::Float((-f64::from_bits(bits)).to_bits()),
        Val::Undef => Val::Undef,
        Val::Unknown => Val::Unknown,
        _ => Val::Undef,
    }
}

pub fn bit_not(v: Val) -> Val {
    match v {
        Val::Int(n) => int(!n),
        Val::Undef => Val::Undef,
        Val::Unknown => Val::Unknown,
        _ => Val::Undef,
    }
}

/// `not`：表 2。有值按 0/1，Undef 保持 Undef，Unknown 保持 Unknown。
pub fn not(v: Val) -> Val {
    match v {
        Val::Int(n) => Val::Int(i64::from(n == 0)),
        Val::Float(bits) => Val::Int(i64::from(bits == 0)),
        Val::Str(s) => Val::Int(i64::from(s.is_empty())),
        Val::Regex(_) => Val::Int(0),
        Val::Undef => Val::Undef,
        Val::Unknown => Val::Unknown,
    }
}

/// `defined`：有值 → 1，Undef → 0，Unknown → Unknown。
pub fn defined(v: Val) -> Val {
    match v {
        Val::Undef => Val::Int(0),
        Val::Unknown => Val::Unknown,
        _ => Val::Int(1),
    }
}

/// 进入布尔上下文（`StrToBool` 与裁决共用）：有值 → 0/1；Undef → Undef；
/// Unknown → Unknown。
pub fn to_int(v: Val) -> Val {
    match v {
        Val::Int(n) => Val::Int(i64::from(n != 0)),
        Val::Float(bits) => Val::Int(i64::from(bits != 0)),
        Val::Str(s) => Val::Int(i64::from(!s.is_empty())),
        Val::Regex(_) => Val::Int(1),
        Val::Undef => Val::Undef,
        Val::Unknown => Val::Unknown,
    }
}

/// 最终规则裁决：非零 → true；0/Undef → false；Unknown → None。
pub fn truth(v: &Val) -> Option<bool> {
    match v {
        Val::Int(n) => Some(*n != 0),
        Val::Float(bits) => Some(*bits != 0),
        Val::Str(s) => Some(!s.is_empty()),
        Val::Regex(_) => Some(true),
        Val::Undef => Some(false),
        Val::Unknown => None,
    }
}
