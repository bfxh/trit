//! 字符串操作数求值（`pe.sections[N].name` / `pe.rich_signature.clear_data`）。
//!
//! 三态与数值侧同纪律：有证据 → `Val`；**确定的未定义**（越界索引 /
//! 无 Rich header）→ `DefiniteFalse`（比较为假）；缺上下文 → `Unknown`。
//! 非字符串操作数返回 `None`（交回数值求值路径）。

use super::Operand;
use super::eval::EvalCtx;
use super::rich;
use super::sections::{self, Sec};
use trit_core::value::Ternary;

/// 字符串操作数的三态结局。
pub(super) enum StrResolved {
    Val(Vec<u8>),
    DefiniteFalse,
    Unknown,
}

/// 求值字符串操作数；`None` = 不是字符串操作数。
pub(super) fn resolve_str(op: &Operand, ctx: &EvalCtx) -> Option<StrResolved> {
    match op {
        Operand::LitStr(s) => Some(StrResolved::Val(s.as_bytes().to_vec())),
        Operand::Opaque(path) if path.starts_with("pe.sections[") && path.ends_with(".name") => {
            Some(match ctx.pe {
                None => StrResolved::Unknown,
                Some(pe) => match sections::resolve(path, pe) {
                    Sec::Name(name) => StrResolved::Val(name),
                    Sec::Undefined => StrResolved::DefiniteFalse,
                    Sec::Val(_) | Sec::Abstain => StrResolved::Unknown,
                },
            })
        }
        Operand::Opaque(path)
            if path == "pe.rich_signature.clear_data" || path == "pe.rich_signature.raw_data" =>
        {
            Some(match ctx.pe {
                None => StrResolved::Unknown,
                Some(_) => match rich::resolve_str(path, ctx.data) {
                    Some(v) => StrResolved::Val(v),
                    None => StrResolved::DefiniteFalse, // 无 Rich header：确定的未定义
                },
            })
        }
        _ => None,
    }
}

/// 字符串比较（`==` / `!=`）。任一侧是字符串操作数时接管；否则 `None`。
pub(super) fn eval_cmp(
    lhs: &Operand,
    op: super::CmpOp,
    rhs: &Operand,
    ctx: &EvalCtx,
) -> Option<Ternary> {
    let l = resolve_str(lhs, ctx);
    let r = resolve_str(rhs, ctx);
    if l.is_none() && r.is_none() {
        return None;
    }
    Some(match (l, r) {
        (Some(StrResolved::Val(a)), Some(StrResolved::Val(b))) => match op {
            super::CmpOp::Eq => bool3(a == b),
            super::CmpOp::Ne => bool3(a != b),
            _ => Ternary::Unknown, // 解析层已拒绝，防御性弃权
        },
        (Some(StrResolved::DefiniteFalse), _) | (_, Some(StrResolved::DefiniteFalse)) => {
            Ternary::False
        }
        _ => Ternary::Unknown,
    })
}

fn bool3(b: bool) -> Ternary {
    if b { Ternary::True } else { Ternary::False }
}
