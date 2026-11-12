//! `pe.*` 模块属性的最小子集（YARA 完整子集方向的第一步）：
//! 官方语义的**标量**属性从 [`crate::pe::PeInfo`] 直接求值；未知路径
//!（数组式 `pe.sections[0]…`、`pe.rich_signature.*`、`math.*` 等）
//! 维持记名弃权——求值恒 `Unknown`，绝不猜。
//!
//! 语义注意（易错点，均为官方探针实测判定）：
//! ①`pe.entry_point` 与裸 `entrypoint` 关键字**同为入口点文件偏移**——
//! PE32 批 RVA≠offset 样本上官方只认 offset；文档写 RVA 但实现不是
//!（第六号语义分歧）。不得按文档猜 RVA。
//! ②`pe.is_dll` / `pe.is_32bit` 都是**函数**（裸属性形态官方编译报错
//! wrong usage），布尔用法必须带 `()`。
//! ③`pe.overlay.*` 与本仓 [`crate::pe::PeInfo::overlay`] **语义分歧**：
//! 官方把节区后的 Authenticode 证书也算进 overlay；本仓 packed 判定
//! 刻意排除证书（签名程序全落四值 B 的既有纪律）。yara 兼容层跟官方
//! （由文件长度直接算），两层分歧如实记录。

use crate::pe::PeInfo;

/// 解析 `pe.*` 标量路径；`None` = 路径不在子集内或上下文缺失（弃权）。
/// `file_len` = 扫描数据长度（官方 overlay 口径需要；`None` = overlay
/// 路径弃权）。
pub(super) fn resolve(path: &str, pe: Option<&PeInfo>, file_len: Option<u64>) -> Option<u64> {
    let pe = pe?;
    // 官方口径：节区（及头部）之后到文件尾都算 overlay——含证书。
    // 无 overlay 时官方给 0（确定的事实，不是证据缺失）。
    // 缺文件长度 = 上下文缺失 → 弃权。
    if path == "pe.overlay.size" || path == "pe.overlay.offset" {
        let end = official_overlay_end(pe);
        let len = file_len?;
        return Some(match path {
            "pe.overlay.size" => len.saturating_sub(end),
            _ if len > end => end,
            _ => 0,
        });
    }
    Some(match path {
        "pe.machine" => u64::from(pe.machine),
        "pe.number_of_sections" => pe.sections.len() as u64,
        "pe.timestamp" => u64::from(pe.timestamp),
        "pe.characteristics" => u64::from(pe.characteristics),
        // 官方探针实测（PE32 批 RVA≠offset 样本上官方只认 offset）：
        // pe.entry_point 与裸 entrypoint 同义 = 入口点**文件偏移**——
        // 官方文档写 RVA 但实现不是（第六号语义分歧）。映射失败 =
        // 证据不完整 → 弃权
        "pe.entry_point" => pe.rva_to_offset(pe.entry_rva)? as u64,
        // 官方探针实测：is_dll / is_32bit 都是**函数**（裸属性形态官方
        // 编译报错 wrong usage）——布尔用法必须带 `()`，路径带 "(…)" 后缀
        "pe.is_dll(…)" => u64::from(pe.characteristics & 0x2000 != 0),
        "pe.is_32bit(…)" => u64::from(!pe.plus),
        _ => return None,
    })
}

/// 官方 overlay 起点：头部大小与各节 raw 区间的最大者。
fn official_overlay_end(pe: &PeInfo) -> u64 {
    let mut end = u64::from(pe.headers_size).max(0x40);
    for s in &pe.sections {
        if s.raw_ptr > 0 {
            end = end.max(u64::from(s.raw_ptr) + u64::from(s.raw_size));
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn known_scalars_resolve_from_info() {
        let mut pe = crate::pe::PeInfo {
            plus: false,
            machine: 0x8664,
            timestamp: 0x6000_0000,
            characteristics: 0x0102,
            subsystem: 3,
            entry_rva: 0x1000,
            image_base: 0x400_000,
            sections: vec![crate::pe::Section {
                name: ".text".into(),
                virtual_size: 0x1000,
                virtual_address: 0x1000,
                raw_size: 0x200,
                raw_ptr: 0x400,
                characteristics: 0x6000_0020,
                entropy: None,
            }],
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
        };
        assert_eq!(resolve("pe.machine", Some(&pe), Some(0x400)), Some(0x8664));
        assert_eq!(resolve("pe.is_dll(…)", Some(&pe), Some(0x400)), Some(0));
        assert_eq!(resolve("pe.is_32bit(…)", Some(&pe), Some(0x400)), Some(1));
        // entry_point = 文件偏移（RVA 0x1000 过节映射 → raw_ptr 0x400）
        assert_eq!(
            resolve("pe.entry_point", Some(&pe), Some(0x400)),
            Some(0x400)
        );
        pe.characteristics |= 0x2000;
        assert_eq!(resolve("pe.is_dll(…)", Some(&pe), Some(0x400)), Some(1));
        // 官方 overlay 口径（含证书）：end = max(头部 0x400, 节末 0x600)
        // = 0x600；文件 0x700 → size 0x100 / offset 0x600；无余量 → 0
        assert_eq!(
            resolve("pe.overlay.size", Some(&pe), Some(0x700)),
            Some(0x100)
        );
        assert_eq!(
            resolve("pe.overlay.offset", Some(&pe), Some(0x700)),
            Some(0x600)
        );
        assert_eq!(resolve("pe.overlay.size", Some(&pe), Some(0x500)), Some(0));
        // overlay 路径缺 file_len = 上下文缺失 → 弃权
        assert_eq!(resolve("pe.overlay.size", Some(&pe), None), None);
        // 上下文缺失 / 子集外路径 / 裸属性形态（官方 wrong usage）= 弃权
        assert_eq!(resolve("pe.machine", None, Some(0x400)), None);
        assert_eq!(resolve("pe.is_dll", Some(&pe), Some(0x400)), None);
        assert_eq!(
            resolve("pe.sections[0].characteristics", Some(&pe), Some(0x400)),
            None
        );
        assert_eq!(
            resolve("pe.rich_signature.version", Some(&pe), Some(0x400)),
            None
        );
    }
}
