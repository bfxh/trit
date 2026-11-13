//! 扫描循环（`impl SigScan`，从 sigscan.rs 拆出守预算）。纪律：命中封顶
//! 截断 = 不完整（False 折 Unknown）；Unscannable 串整条 Unknown。

use super::{Rule, SigReport, SigScan};

impl<'a> SigScan<'a> {
    pub fn new(rules: &'a [Rule]) -> Self {
        Self { rules }
    }

    /// 扫描一段字节。`truncated` = 调用方声明输入不完整（读取中断/分片）。
    pub fn scan(&self, data: &[u8], truncated: bool) -> SigReport {
        self.scan_with_entrypoint(data, truncated, None)
    }

    /// 带入口点上下文的扫描。`entrypoint` = 入口点的**文件偏移**——与官方
    /// YARA 语义一致（已实测对拍：RVA≠文件偏移的样本上，官方命中 offset
    /// 不命中 RVA）；由调用方从 PE/ELF 头解析传入（PE：`pe::parse` 的
    /// `entry_rva` 过 `rva_to_offset`）。`None` = 非可执行文件/解析失败 →
    /// `entrypoint` 操作数如实弃权（Unknown），绝不猜。
    pub fn scan_with_entrypoint(
        &self,
        data: &[u8],
        truncated: bool,
        entrypoint: Option<u64>,
    ) -> SigReport {
        self.scan_with_context(data, truncated, entrypoint, None)
    }

    /// 带 PE 模块上下文的扫描：`pe` = 调用方从 PE 头解析出的信息——
    /// `pe.*` 子集属性据此求值；`None` = 非 PE / 解析失败 → `pe.*`
    /// 如实弃权（Unknown），绝不猜。
    pub fn scan_with_context(
        &self,
        data: &[u8],
        truncated: bool,
        entrypoint: Option<u64>,
        pe: Option<&crate::pe::PeInfo>,
    ) -> SigReport {
        super::xscan::scan_rules(self.rules, data, truncated, entrypoint, pe)
    }
}
