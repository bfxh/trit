//! 字节级匹配引擎（从 `sigscan.rs` 拆出，控制单文件行数预算）：
//! hex 串（通配 + 跳跃 + 确定性预算）与文本串（XOR 密钥展开 × 变体 +
//! nocase 折叠 + fullword 词边界）。

use super::{HexByte, HexPattern, Jump, MAX_HITS, TextPattern};

/// 单模式扫描的步数预算（段尝试 + 首段候选比较合计）。跳跃模式的回溯在
/// 对抗数据上会指数爆炸（`{ 90 [0-4096] 90 [0-4096] … }` 配全 0x90 数据），
/// 预算封顶 = **不挂起**（"任何输入不 panic 不挂起"不变量的扫描面）。
/// 耗尽 = 扫描不完整 → 规则裁决按 `Unknown` 落账（**"未命中只在完整扫描
/// 后给出"的组成面**：截断的沉默不是清白，预算耗尽的沉默同样不是）；
/// YARA 用时间超时，我们用确定性预算——同样的 fail-closed，可复算。
pub const SCAN_BUDGET: u64 = 1_000_000;

/// hex 模式匹配结果：命中 + 扫描是否完整（预算耗尽 = 不完整）。
pub(super) fn find_hex(data: &[u8], hp: &HexPattern) -> (Vec<u64>, bool) {
    let mut hits = Vec::new();
    let mut budget = SCAN_BUDGET;
    let mut complete = true;
    for pos in 0..data.len() {
        budget = budget.saturating_sub(1);
        if budget == 0 {
            complete = false;
            break; // 预算耗尽：该模式停止搜索（不挂起）
        }
        if chunk_matches(data, pos, &hp.lead)
            && extend_rest(data, &hp.rest, pos + hp.lead.len(), &mut budget)
        {
            hits.push(pos as u64);
            if hits.len() >= MAX_HITS {
                complete = false; // 命中表封顶截断 = 不完整（位置性条件弃权）
                break;
            }
        }
    }
    (hits, complete)
}

/// 从 `pos` 起，按 `rest` 的（跳跃, 段）序列依次匹配——每段起点 ∈
/// `[前段末尾+min, 前段末尾+max]`（YARA 跳跃语义），全部段落位即成功。
/// 每次段尝试扣一步预算，耗尽即整条放弃（fail-closed 不挂起）。
fn extend_rest(data: &[u8], rest: &[(Jump, Vec<HexByte>)], pos: usize, budget: &mut u64) -> bool {
    let Some((jump, chunk)) = rest.first() else {
        return true; // 没有后续段：首段落位即整条命中
    };
    let start = pos.saturating_add(jump.min as usize);
    let last = pos.saturating_add(jump.max as usize);
    for seg_start in start..=last {
        *budget = budget.saturating_sub(1);
        if *budget == 0 {
            return false; // 预算耗尽：放弃（不挂起）
        }
        if seg_start + chunk.len() > data.len() {
            break;
        }
        if chunk_matches(data, seg_start, chunk)
            && extend_rest(data, &rest[1..], seg_start + chunk.len(), budget)
        {
            return true;
        }
    }
    false
}

fn chunk_matches(data: &[u8], pos: usize, chunk: &[HexByte]) -> bool {
    pos + chunk.len() <= data.len()
        && chunk.iter().enumerate().all(|(i, b)| match b {
            HexByte::Lit(v) => data[pos + i] == *v,
            HexByte::Wild => true,
        })
}

/// 文本匹配：逐（XOR 密钥 × 变体）展开后找全部命中（nocase = ASCII 折叠；
/// fullword = 词边界）。预算按滑窗步数计——256 路 xor 在大文件上会如实
/// 耗尽为「扫描不完整」→ Unknown（fail-closed，不夸大覆盖）。
pub(super) fn find_text(data: &[u8], tp: &TextPattern) -> (Vec<u64>, bool) {
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
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
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
