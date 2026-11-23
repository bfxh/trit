//! 文本串匹配（从 `matchers.rs` 拆出守模块预算）：逐（XOR 密钥 × 变体）展开后
//! 找全部命中（nocase = ASCII 折叠；fullword = 词边界）。

use super::super::{MAX_HITS, TextPattern};
use super::SCAN_BUDGET;

/// 文本匹配：逐（XOR 密钥 × 变体）展开后找全部命中（nocase = ASCII 折叠；
/// fullword = 词边界）。预算按滑窗步数计——256 路 xor 在大文件上会如实
/// 耗尽为「扫描不完整」→ Unknown（fail-closed，不夸大覆盖）。
pub(crate) fn find_text(data: &[u8], tp: &TextPattern) -> (Vec<u64>, bool) {
    let mut hits = Vec::new();
    let mut budget = SCAN_BUDGET;
    let mut complete = true;
    let keys: Vec<u8> = match tp.xor {
        Some((lo, hi)) => (lo..=hi).collect(),
        None => vec![0x00], // 无 xor：单"密钥"恒等
    };
    'keys: for key in keys {
        for variant in &tp.variants {
            // 变体逐字节 XOR（wide 亦然——组合语义的边界见模块注记）
            let mut needle: Vec<u8> = variant.bytes.iter().map(|b| b ^ key).collect();
            if needle.is_empty() {
                continue;
            }
            if tp.nocase {
                needle.make_ascii_lowercase();
            }
            let mut from = 0usize;
            while from + needle.len() <= data.len() {
                budget = budget.saturating_sub(1);
                if budget == 0 {
                    complete = false;
                    break 'keys; // 预算耗尽：停止搜索（不挂起、不假清白）
                }
                let hit = if tp.nocase {
                    data[from..from + needle.len()].to_ascii_lowercase() == needle
                } else {
                    &data[from..from + needle.len()] == needle.as_slice()
                };
                if hit && (!tp.fullword || word_bounded(data, from, needle.len(), variant.wide)) {
                    hits.push(from as u64);
                    if hits.len() >= MAX_HITS {
                        complete = false; // 命中表封顶截断 = 不完整
                        break 'keys;
                    }
                    from += needle.len(); // 非重叠推进（对齐官方与既有行为）
                } else {
                    from += 1;
                }
            }
        }
    }
    hits.sort_unstable();
    hits.dedup();
    (hits, complete)
}

/// fullword 的词边界：命中前后不得是「字母数字下划线」；宽串按宽字符判
///（前邻 = 两字节 `(字母, 0x00)`，后邻同理）。
fn word_bounded(data: &[u8], pos: usize, len: usize, wide: bool) -> bool {
    // 官方 fullword 的词字符 = **仅字母数字**（yara-python 实测判定：
    // `ab_cat_cd` 官方 HIT——下划线不算词字符，与正则 `\b` 不同）。
    let is_word = |b: u8| b.is_ascii_alphanumeric();
    if wide {
        let preceded = pos >= 2 && is_word(data[pos - 2]) && data[pos - 1] == 0x00;
        let followed =
            pos + len + 1 < data.len() && is_word(data[pos + len]) && data[pos + len + 1] == 0x00;
        !preceded && !followed
    } else {
        let preceded = pos > 0 && is_word(data[pos - 1]);
        let followed = pos + len < data.len() && is_word(data[pos + len]);
        !preceded && !followed
    }
}
