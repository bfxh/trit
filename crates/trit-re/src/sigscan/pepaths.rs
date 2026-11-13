//! `Operand::Opaque` 的 pe.* 路径统一分派（从 `sections.rs` 拆出并扩展）：
//! rich_signature → sections 数组式 → 标量子集；子集外 / 缺上下文 = 记名
//! 弃权。字符串属性（`.name` / rich 的 clear_data/raw_data）走
//! [`super::strval`]，不在此处。

use super::eval::EvalCtx;
use super::resolve::Resolved;
use super::{peprops, rich, sections};

pub(super) fn resolve_opaque(path: &str, ctx: &EvalCtx) -> Resolved {
    if path.starts_with("pe.rich_signature.") {
        return match ctx.pe {
            None => Resolved::Unknown,
            // 无 Rich header = 确定的未定义（官方：undefined 比较为假）
            Some(_) => {
                rich::resolve_num(path, ctx.data).map_or(Resolved::DefiniteFalse, Resolved::Val)
            }
        };
    }
    if path.starts_with("pe.sections[") {
        return match ctx.pe {
            Some(pe) => match sections::resolve(path, pe) {
                sections::Sec::Val(v) => Resolved::Val(v),
                // 字符串字段在数值上下文不可用（解析层应已拒绝）→ 弃权
                sections::Sec::Name(_) => Resolved::Unknown,
                sections::Sec::Undefined => Resolved::DefiniteFalse,
                sections::Sec::Abstain => Resolved::Unknown,
            },
            None => Resolved::Unknown,
        };
    }
    peprops::resolve(path, ctx.pe, Some(ctx.data.len() as u64))
        .map_or(Resolved::Unknown, Resolved::Val)
}
