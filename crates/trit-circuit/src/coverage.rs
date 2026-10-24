//! 测试向量的集合覆盖：stuck-at 与桥接 ATPG 共用。

use std::collections::BTreeMap;

use crate::circuit::{Circuit, CircuitError};
use crate::pattern::TestPattern;

pub(crate) fn minimum_cover(
    sets: &[(u64, u64)],
    full: u64,
    budget: usize,
) -> (Option<Vec<u64>>, bool) {
    let mut best: Option<Vec<u64>> = None;
    let mut chosen: Vec<u64> = Vec::new();
    let max_gain = sets
        .iter()
        .map(|(_, faults)| faults.count_ones() as usize)
        .max()
        .unwrap_or(0)
        .max(1);
    let mut remaining = budget;
    let mut exhausted = false;
    search_cover(
        full,
        &mut chosen,
        &mut best,
        sets,
        max_gain,
        &mut remaining,
        &mut exhausted,
    );
    (best, !exhausted)
}

fn search_cover(
    uncovered: u64,
    chosen: &mut Vec<u64>,
    best: &mut Option<Vec<u64>>,
    sets: &[(u64, u64)],
    max_gain: usize,
    budget: &mut usize,
    exhausted: &mut bool,
) {
    if *budget == 0 {
        *exhausted = true;
        return;
    }
    *budget -= 1;
    if uncovered == 0 {
        if best.as_ref().is_none_or(|b| chosen.len() < b.len()) {
            *best = Some(chosen.clone());
        }
        return;
    }
    let best_len = best.as_ref().map_or(usize::MAX, Vec::len);
    let lower = chosen.len() + (uncovered.count_ones() as usize).div_ceil(max_gain);
    if lower >= best_len {
        return;
    }
    let bit = uncovered & uncovered.wrapping_neg();
    let mut candidates: Vec<(u64, u64)> = sets
        .iter()
        .filter(|(_, faults)| faults & bit != 0)
        .copied()
        .collect();
    candidates.sort_by_key(|(mask, faults)| {
        (std::cmp::Reverse((faults & uncovered).count_ones()), *mask)
    });
    for (position, (mask, faults)) in candidates.iter().enumerate() {
        chosen.push(*mask);
        search_cover(
            uncovered & !faults,
            chosen,
            best,
            sets,
            max_gain,
            budget,
            exhausted,
        );
        chosen.pop();
        // 预算在子调用里耗尽时，若还有**没试过的兄弟分支**，就不能声称已证明最优。
        // 旧实现在这里直接 break 却不置 `exhausted`，于是「3 个向量」冒充最优（复审 M03）。
        if *budget == 0 && position + 1 < candidates.len() {
            *exhausted = true;
            break;
        }
    }
}

/// 精确最小覆盖：仅对 ≤64 项、≤256 向量的小实例尝试。
///
/// 返回 `(patterns, proven_optimal)`；规模超限或没有解时返回 `None`。
pub(crate) fn exact_cover(
    circuit: &Circuit,
    by_mask: &BTreeMap<u64, Vec<usize>>,
    item_count: usize,
    budget: usize,
) -> Result<Option<(Vec<TestPattern>, bool)>, CircuitError> {
    if item_count > 64 || by_mask.len() > 256 {
        return Ok(None);
    }
    let sets: Vec<(u64, u64)> = by_mask
        .iter()
        .map(|(mask, covers)| {
            let bits = covers
                .iter()
                .fold(0u64, |acc, &index| acc | (1u64 << index));
            (*mask, bits)
        })
        .collect();
    let full = sets.iter().fold(0u64, |acc, (_, items)| acc | items);
    let (cover, proven) = minimum_cover(&sets, full, budget);
    let Some(masks) = cover else {
        return Ok(None);
    };
    let mut assigned = vec![false; item_count];
    let mut patterns = Vec::with_capacity(masks.len());
    for mask in masks {
        let mut covers = Vec::new();
        for &index in &by_mask[&mask] {
            if !assigned[index] {
                assigned[index] = true;
                covers.push(index);
            }
        }
        patterns.push(TestPattern {
            inputs: circuit.decode_binary(mask)?,
            covers,
            dont_care: Vec::new(),
        });
    }
    Ok(Some((patterns, proven)))
}

/// 贪心覆盖：每轮选“覆盖最多未覆盖项”的向量，平局取更小 mask。
pub(crate) fn greedy_cover(
    circuit: &Circuit,
    by_mask: &BTreeMap<u64, Vec<usize>>,
    covered: &[bool],
) -> Result<Vec<TestPattern>, CircuitError> {
    let mut uncovered = covered.to_vec();
    let mut patterns = Vec::new();
    loop {
        let mut best: Option<(u64, usize)> = None;
        for (&mask, covers) in by_mask {
            let gain = covers.iter().filter(|&&index| uncovered[index]).count();
            if gain == 0 {
                continue;
            }
            let better = match best {
                None => true,
                Some((best_mask, best_gain)) => {
                    gain > best_gain || (gain == best_gain && mask < best_mask)
                }
            };
            if better {
                best = Some((mask, gain));
            }
        }
        let Some((mask, _)) = best else {
            break;
        };
        let mut covers = Vec::new();
        for &index in &by_mask[&mask] {
            if uncovered[index] {
                uncovered[index] = false;
                covers.push(index);
            }
        }
        patterns.push(TestPattern {
            inputs: circuit.decode_binary(mask)?,
            covers,
            dont_care: Vec::new(),
        });
    }
    Ok(patterns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute_force_cover(sets: &[(u64, u64)], full: u64) -> usize {
        let mut best = usize::MAX;
        for pick in 1u32..(1u32 << sets.len()) {
            let mut union = 0u64;
            let mut count = 0;
            for (index, (_, faults)) in sets.iter().enumerate() {
                if pick & (1 << index) != 0 {
                    union |= faults;
                    count += 1;
                }
            }
            if union == full {
                best = best.min(count);
            }
        }
        best
    }

    #[test]
    fn exhausted_budget_is_not_reported_as_optimal() {
        let sets = vec![(1u64, 0b11u64), (2u64, 0b01u64)];
        let (_, proven) = minimum_cover(&sets, 0b11, 1);
        assert!(!proven, "预算耗尽不能宣称已证明最优");
    }

    #[test]
    fn budget_exhaustion_with_unexplored_siblings_is_not_optimal() {
        // 复审 M03 复现：full=63、budget=4。修复前返回 3 个向量却声称 proven，
        // 而穷举最优是 2。
        let sets = vec![
            (1u64, 16u64),
            (2, 57),
            (3, 37),
            (4, 29),
            (5, 27),
            (6, 5),
            (7, 56),
        ];
        let (cover, proven) = minimum_cover(&sets, 63, 4);
        assert_eq!(
            cover.map(|masks| masks.len()),
            Some(3),
            "预算 4 下只能找到 3 个"
        );
        assert!(!proven, "预算触底且有未探索分支时不能声称最优");

        let (cover, proven) = minimum_cover(&sets, 63, 100_000);
        assert_eq!(cover.map(|masks| masks.len()), Some(2), "穷举最优应为 2");
        assert!(proven, "穷举完必须能证明最优");
    }

    #[test]
    fn minimum_cover_matches_bruteforce() {
        let mut state = 0x1234_5678u64;
        let mut checked = 0;
        for _ in 0..16 {
            let mut sets: Vec<(u64, u64)> = Vec::new();
            let mut full = 0u64;
            for mask in 0..8u64 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let faults = (state >> 33) & 0x3F;
                if faults != 0 {
                    sets.push((mask, faults));
                    full |= faults;
                }
            }
            if full == 0 || sets.len() > 10 {
                continue;
            }
            let (cover, _) = minimum_cover(&sets, full, 100_000);
            let cover = cover.expect("小实例应有解");
            let union = cover.iter().fold(0u64, |acc, mask| {
                acc | sets.iter().find(|(m, _)| m == mask).unwrap().1
            });
            assert_eq!(union, full, "覆盖不完整");
            assert_eq!(cover.len(), brute_force_cover(&sets, full), "不是最小覆盖");
            checked += 1;
        }
        assert!(checked >= 8, "有效实例太少：{checked}");
    }
}
