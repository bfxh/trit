//! 字节级 hex 匹配引擎（从 `sigscan.rs` 拆出，控制单文件行数预算）：
//! hex 串（通配 + 跳跃 + 确定性预算）。文本串匹配见 [`super::text_match`]。

use super::{HexByte, HexPattern, Jump, MAX_HITS};

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
///
/// 显式栈迭代（取代逐段递归）：帧 = (段下标, 前段末尾位置, 本段下一候选起点)。
/// DFS 顺序、首命中即返回、无命中回溯与原递归逐一对应；预算口径不变——
/// 每个段候选尝试扣 1 步，耗尽返回 false（不挂起、不假清白）。
fn extend_rest(data: &[u8], rest: &[(Jump, Vec<HexByte>)], pos: usize, budget: &mut u64) -> bool {
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    let mut seg = 0usize;
    let mut cur_pos = pos;
    // `usize::MAX` = 新进一段，候选起点重置为本段 min；回溯帧带回下一候选。
    let mut seg_start = usize::MAX;
    while seg < rest.len() {
        let (jump, chunk) = &rest[seg];
        let start = cur_pos.saturating_add(jump.min as usize);
        let last = cur_pos.saturating_add(jump.max as usize);
        if seg_start == usize::MAX {
            seg_start = start;
        }
        if seg_start > last {
            // 本段候选耗尽 → 回溯上一段
            let Some((s, p, n)) = stack.pop() else {
                return false;
            };
            seg = s;
            cur_pos = p;
            seg_start = n;
            continue;
        }
        *budget = budget.saturating_sub(1);
        if *budget == 0 {
            return false; // 预算耗尽：放弃（不挂起）
        }
        if seg_start + chunk.len() > data.len() {
            // 起点更大的候选同样越界 → 结束本段，回溯
            let Some((s, p, n)) = stack.pop() else {
                return false;
            };
            seg = s;
            cur_pos = p;
            seg_start = n;
            continue;
        }
        if chunk_matches(data, seg_start, chunk) {
            // 候选成立：压入本段下一候选，推进到下一段
            stack.push((seg, cur_pos, seg_start + 1));
            seg += 1;
            cur_pos = seg_start + chunk.len();
            seg_start = usize::MAX;
        } else {
            seg_start += 1;
        }
    }
    true
}

fn chunk_matches(data: &[u8], pos: usize, chunk: &[HexByte]) -> bool {
    pos + chunk.len() <= data.len()
        && chunk.iter().enumerate().all(|(i, b)| match b {
            HexByte::Lit(v) => data[pos + i] == *v,
            HexByte::Wild => true,
        })
}

#[path = "text_match.rs"]
mod text_match;
pub(super) use text_match::find_text;
