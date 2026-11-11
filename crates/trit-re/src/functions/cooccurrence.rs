//! 共现判定（无序 + 有序）——从 `mod.rs` 拆出，守单文件行数预算。
//!
//! 无序 = 同函数内能力桶齐备；有序 = 桶按**地址升序**成子序列出现
//!（强证据：真注入是三次独立调用按 申请→写→执行 的次序）。三值口径
//! 两者一致：证据不完整 → `Unknown`，绝不参与 `False` 结论。

use super::FunctionReport;
use trit_core::value::Ternary;

impl FunctionReport {
    /// 是否存在"同一函数内同时出现全部 `required` 能力桶"。
    ///
    /// 三值规则：
    /// - 找到 → `True`（正证据不会被别处的不确定性抹掉）；
    /// - 没找到，但边界不精确/交叉引用不完整/有调用点归不到函数 → `Unknown`
    ///   （"没看见"不等于"没有"）；
    /// - 没找到且证据完整 → `False`。
    pub fn cooccurrence(&self, required: &[&str], xref_complete: bool) -> Ternary {
        if !xref_complete || !self.exact {
            return Ternary::Unknown;
        }
        let found = self.functions.iter().any(|function| {
            required
                .iter()
                .all(|need| function.buckets.iter().any(|bucket| bucket == need))
        });
        if found {
            return Ternary::True;
        }
        // 未归属的调用点只在本组相关能力上才构成"可能漏看"。
        if required.iter().any(|need| {
            self.unattributed_buckets
                .iter()
                .any(|bucket| bucket == need)
        }) {
            Ternary::Unknown
        } else {
            Ternary::False
        }
    }

    /// **有序**共现：`required` 的能力桶在某个函数内按**地址升序**成子序列
    /// 出现（一个调用点最多满足一个需求——真注入是三次独立调用，不是一次
    /// 万能导入）。三值规则与 [`Self::cooccurrence`] 同口径。
    ///
    /// 例：注入三件套 `alloc → write → exec` 的地址序成立才算有序检出；
    /// 地址序反着写（exec 在前）只满足无序桶，不满足有序。
    pub fn cooccurrence_ordered(&self, required: &[&str], xref_complete: bool) -> Ternary {
        if !xref_complete || !self.exact {
            return Ternary::Unknown;
        }
        let found = self.functions.iter().any(|function| {
            let mut cursor = 0usize;
            required.iter().all(|need| {
                match function.site_buckets[cursor..]
                    .iter()
                    .position(|(_, buckets)| buckets.iter().any(|b| b == need))
                {
                    Some(offset) => {
                        cursor += offset + 1;
                        true
                    }
                    None => false,
                }
            })
        });
        if found {
            return Ternary::True;
        }
        if required.iter().any(|need| {
            self.unattributed_buckets
                .iter()
                .any(|bucket| bucket == need)
        }) {
            Ternary::Unknown
        } else {
            Ternary::False
        }
    }
}
