//! `pe.sections[N]` 数组式访问（pe.* 深水区第一潜）。
//!
//! 官方语义：越界索引的字段 = **确定的未定义**（比较一律为假）——与
//! 零命中串的 `@a[N]` 同款纪律（第五号语义分歧的推广）；非数值下标
//! （解析层已吞成 `[…]`）维持记名弃权。字段集 = Section 的数值成员；
//! `.name` 是字符串（条件等值比较基建未建）→ 弃权。

use super::eval::EvalCtx;
use super::resolve::Resolved;
use crate::pe::PeInfo;

/// 数组式解析结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sec {
    Val(u64),
    /// 确定的未定义（越界索引）——比较为假。
    Undefined,
    /// 路径不在子集内（记名弃权）。
    Abstain,
}

/// 解析 `pe.sections[N].<field>`；其余路径返回 `Abstain`。
pub(super) fn resolve(path: &str, pe: &PeInfo) -> Sec {
    let Some(rest) = path.strip_prefix("pe.sections[") else {
        return Sec::Abstain;
    };
    let Some(close) = rest.find(']') else {
        return Sec::Abstain;
    };
    let Ok(idx) = rest[..close].parse::<usize>() else {
        return Sec::Abstain; // 非数值下标（如按名查找）→ 弃权
    };
    let Some(field) = rest.get(close + 2..) else {
        return Sec::Abstain; // 缺字段段
    };
    let Some(sec) = pe.sections.get(idx) else {
        return Sec::Undefined; // 越界 = 确定的未定义（官方：比较为假）
    };
    let v = match field {
        "characteristics" => u64::from(sec.characteristics),
        "virtual_size" => u64::from(sec.virtual_size),
        "virtual_address" => u64::from(sec.virtual_address),
        "raw_size" => u64::from(sec.raw_size),
        "raw_ptr" => u64::from(sec.raw_ptr),
        _ => return Sec::Abstain, // name（字符串）等未实装
    };
    Sec::Val(v)
}

/// `Operand::Opaque` 统一分派：`pe.sections[N].*` 三态优先，其余路径
/// 回落标量子集（子集外 / 缺上下文 = 记名弃权）。
pub(super) fn resolve_opaque(path: &str, ctx: &EvalCtx) -> Resolved {
    if path.starts_with("pe.sections[") {
        return match ctx.pe {
            Some(pe) => match resolve(path, pe) {
                Sec::Val(v) => Resolved::Val(v),
                Sec::Undefined => Resolved::DefiniteFalse,
                Sec::Abstain => Resolved::Unknown,
            },
            None => Resolved::Unknown,
        };
    }
    super::resolve_pe_prop(path, ctx.pe, Some(ctx.data.len() as u64))
        .map_or(Resolved::Unknown, Resolved::Val)
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::pe::{PeInfo, Section};

    fn pe2() -> PeInfo {
        PeInfo {
            plus: true,
            machine: 0x8664,
            timestamp: 0,
            characteristics: 0x0102,
            subsystem: 3,
            entry_rva: 0x1000,
            image_base: 0x400_000,
            sections: vec![
                Section {
                    name: ".text".into(),
                    virtual_size: 0x1000,
                    virtual_address: 0x1000,
                    raw_size: 0x200,
                    raw_ptr: 0x400,
                    characteristics: 0x6000_0020,
                    entropy: None,
                },
                Section {
                    name: ".rdata".into(),
                    virtual_size: 0x800,
                    virtual_address: 0x2000,
                    raw_size: 0x400,
                    raw_ptr: 0x600,
                    characteristics: 0x4000_0040,
                    entropy: None,
                },
            ],
            imports: None,
            exports: Vec::new(),
            debug_pdb: None,
            has_security_dir: false,
            has_signed_data: false,
            rich_header: false,
            runtime_functions: None,
            coff_functions: None,
            clr_dir_present: false,
            clr: None,
            tls_dir_present: false,
            tls_callbacks: None,
            overlay: None,
            headers_size: 0x400,
        }
    }

    #[test]
    fn numeric_fields_resolve_by_index() {
        let pe = pe2();
        assert_eq!(
            resolve("pe.sections[0].characteristics", &pe),
            super::Sec::Val(0x6000_0020)
        );
        assert_eq!(
            resolve("pe.sections[1].raw_ptr", &pe),
            super::Sec::Val(0x600)
        );
    }

    #[test]
    fn out_of_range_is_definite_undefined() {
        let pe = pe2();
        assert_eq!(
            resolve("pe.sections[5].characteristics", &pe),
            super::Sec::Undefined
        );
    }

    #[test]
    fn non_members_abstain() {
        let pe = pe2();
        // name（字符串等值基建未建）与非成员路径维持弃权
        assert_eq!(resolve("pe.sections[0].name", &pe), super::Sec::Abstain);
        assert_eq!(resolve("pe.machine", &pe), super::Sec::Abstain);
        assert_eq!(resolve("pe.sections[x]", &pe), super::Sec::Abstain);
    }
}
