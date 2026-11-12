//! 操作数解析三态（从 eval.rs 拆出——模块预算棘轮纪律）：每个 Operand
//! 有确定值 / 确定的未定义 / 证据缺失三种归宿。差分实测纪律（第四个
//! 语义分歧的修正）：**确定的未定义 ≠ 证据缺失**——零命中串的 `@a[N]`
//! 比较为假；截断/缺上下文 = 弃权（Unknown），绝不折叠成假。

use super::Operand;
use super::eval::EvalCtx;

pub(super) enum Resolved {
    Val(u64),
    DefiniteFalse,
    Unknown,
}

pub(super) fn resolve(op: &Operand, occurrences: &[(String, Vec<u64>)], ctx: &EvalCtx) -> Resolved {
    use Resolved::*;
    match op {
        Operand::Lit(v) => Val(*v),
        Operand::Filesize => {
            if ctx.truncated {
                Unknown // 截断输入的大小不完整——filesize 上的比较不可信
            } else {
                Val(ctx.data.len() as u64)
            }
        }
        Operand::Count(index) => occurrences
            .get(*index)
            .map_or(Unknown, |(_, offs)| Val(offs.len() as u64)),
        Operand::Offset { index, nth } => {
            occurrences
                .get(*index)
                .map_or(Unknown, |(_, offs)| match offs.get(*nth) {
                    Some(v) => Val(*v),
                    // 零命中 = 确定的未定义（官方：比较为假）；有命中但下标越界
                    //（命中表可能被 MAX_HITS 截断）= 证据不完整 → 弃权
                    None if offs.is_empty() => DefiniteFalse,
                    None => Unknown,
                })
        }
        Operand::UintRead { size, offset } => {
            if ctx.truncated {
                return Unknown;
            }
            let start = *offset as usize;
            match start.checked_add(*size as usize) {
                Some(end) if end <= ctx.data.len() => {
                    let mut padded = [0u8; 8];
                    padded[..*size as usize].copy_from_slice(&ctx.data[start..end]);
                    Val(u64::from_le_bytes(padded))
                }
                _ => DefiniteFalse, // 非截断的越界读：字节客观不存在
            }
        }
        Operand::Entrypoint => ctx.entrypoint.map_or(Unknown, Val),
        // pe.*：pe.sections[N] 三态 + 标量子集，子集外/缺上下文 = 弃权。
        Operand::Opaque(path) => super::sections::resolve_opaque(path, ctx),
        Operand::CurrentCount => ctx
            .binding
            .and_then(|i| occurrences.get(i))
            .map_or(Unknown, |(_, offs)| Val(offs.len() as u64)),
        Operand::CurrentOffset { nth } => {
            ctx.binding
                .and_then(|i| occurrences.get(i))
                .map_or(Unknown, |(_, offs)| match offs.get(*nth) {
                    Some(v) => Val(*v),
                    None if offs.is_empty() => DefiniteFalse,
                    None => Unknown,
                })
        }
    }
}
