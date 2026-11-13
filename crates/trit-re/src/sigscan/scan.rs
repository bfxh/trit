//! 扫描循环（`impl SigScan`，从 sigscan.rs 拆出守预算）。纪律：命中封顶
//! 截断 = 不完整（False 折 Unknown）；Unscannable 串整条 Unknown。

use super::{EvalCtx, Pattern, Rule, RuleVerdict, SigReport, SigScan};
use super::{SCAN_BUDGET, eval, find_hex, find_text};
use trit_core::value::Ternary;

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
        let mut rules = Vec::new();
        for rule in self.rules {
            // 每个模式的命中偏移（首个起，最多 MAX_HITS 个）+ 扫描完成度
            //（hex/文本预算耗尽 = 不完整——"未命中只在完整扫描后给出"）
            let mut occurrences: Vec<(String, Vec<u64>)> = Vec::new();
            let mut complete = true;
            let mut has_unscannable = false;
            for (id, pattern) in &rule.patterns {
                let (hits, pattern_complete) = match pattern {
                    Pattern::Hex(hp) => find_hex(data, hp),
                    Pattern::Text(tp) => find_text(data, tp),
                    Pattern::Regex(rp) => super::regex::find_all(data, rp, SCAN_BUDGET),
                    Pattern::Base64(tps) => super::b64::find_union(data, tps),
                    Pattern::Unscannable(_) => {
                        // 评估不了的串不能报"没命中"：整条规则按 Unknown 落账
                        has_unscannable = true;
                        (Vec::new(), true)
                    }
                };
                complete &= pattern_complete;
                occurrences.push((id.clone(), hits));
            }
            let ctx = EvalCtx {
                data,
                truncated,
                binding: None,
                entrypoint,
                pe,
            };
            let met = eval(&rule.condition, &occurrences, &ctx);
            let hits = rule.report_hits(&occurrences);
            let value = if has_unscannable {
                Ternary::Unknown
            } else {
                match met {
                    Ternary::True => Ternary::True,
                    Ternary::Unknown => Ternary::Unknown,
                    Ternary::False if truncated || !complete => Ternary::Unknown,
                    Ternary::False => Ternary::False,
                }
            };
            rules.push(RuleVerdict {
                rule: rule.name.clone(),
                value,
                hits,
            });
        }
        // 整体 = Kleene OR：任一 True → True；否则有 Unknown → Unknown；
        // 全 False → False（报警语义：截断的沉默不是清白）。
        let mut value = Ternary::False;
        for r in &rules {
            value = match (value, r.value) {
                (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
                (Ternary::Unknown, _) | (_, Ternary::Unknown) => Ternary::Unknown,
                _ => Ternary::False,
            };
        }
        SigReport { rules, value }
    }
}
