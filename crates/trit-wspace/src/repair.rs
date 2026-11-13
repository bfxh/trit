//! 无梯度最小修补：在离散权重空间里搜一个补丁，让网络在验证集上**逐样本**正确。
//!
//! 搜索按状态空间大小分两条路：
//!
//! 1. 小网络（`3^权重数 ≤ exhaustive_states`）：**按编辑数逐层 BFS**。第一次找到解
//!    的层数就是最小编辑数，结果是完备且确定性的；
//! 2. 大网络：半径 1 穷举 → 半径 2 穷举（候选对在预算内）→ 按 `(失配数, 裕量能量)`
//!    排序的束搜索。
//!
//! 不做梯度、不改 scale（scale 是行为规范自由度）。返回的补丁永远是**精确**的：
//! 找不到就在预算内返回 `None`，不会用“差不多对”糊弄。

use std::collections::HashSet;

use crate::edit::Patch;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;

mod options;
mod scope;
pub use options::RepairOptions;
pub(crate) use scope::candidates;
use scope::scoped_candidates;

mod energy;
use energy::energy;

mod softplus;

/// 修补结果（只在验证集完全正确时返回）。
#[derive(Clone, Debug, PartialEq)]
pub struct Repair {
    /// 找到的补丁。
    pub patch: Patch,
    /// 修补前失配数。
    pub before_loss: usize,
    /// 修补后失配数（恒为 0；保留字段是为了报告对齐）。
    pub after_loss: usize,
    /// 评估过的候选数。
    pub evaluated: usize,
}

struct State {
    net: Net,
    patch: Patch,
    loss: usize,
    energy: f32,
    key: u64,
}

/// 搜一个让 `oracle` 全部通过的补丁；预算内找不到返回 `None`。
pub fn repair(net: &Net, oracle: &Oracle, options: &RepairOptions) -> Result<Option<Repair>> {
    oracle.check_arch(net)?;
    let before_loss = oracle.loss(net)?;
    if before_loss == 0 {
        if !keep_ok(options.keep.as_ref(), net)? {
            return Ok(None);
        }
        return Ok(Some(Repair {
            patch: Patch::default(),
            before_loss,
            after_loss: 0,
            evaluated: 0,
        }));
    }
    if options.max_edits == 0 {
        return Ok(None);
    }

    let params = net.trit_count();
    if params <= 20 && 3u64.pow(params as u32) <= options.exhaustive_states {
        return bfs(net, oracle, options, before_loss);
    }

    let candidates = scoped_candidates(net, options);
    let mut evaluated = 0usize;

    // 半径 1：按定义最小。
    let mut seeds = Vec::with_capacity(candidates.len());
    for candidate in &candidates {
        let mut trial = net.clone();
        candidate.apply_to(&mut trial)?;
        evaluated += 1;
        let loss = oracle.loss(&trial)?;
        if loss == 0 && keep_ok(options.keep.as_ref(), &trial)? {
            return Ok(Some(Repair {
                patch: Patch::new(vec![candidate.clone()]),
                before_loss,
                after_loss: 0,
                evaluated,
            }));
        }
        let energy = energy(&trial, oracle, options.keep.as_ref())?;
        seeds.push(State {
            key: trial.state_hash(),
            net: trial,
            patch: Patch::new(vec![candidate.clone()]),
            loss,
            energy,
        });
    }
    let mut visited: HashSet<u64> = seeds.iter().map(|state| state.key).collect();
    visited.insert(net.state_hash());

    // 半径 2：仍然是最小解（两条不同坐标的编辑）。
    if options.max_edits >= 2 && candidates.len() * candidates.len() <= options.exhaustive_pairs {
        for (i, first) in candidates.iter().enumerate() {
            for second in &candidates[i + 1..] {
                if first.same_coord(second) {
                    continue;
                }
                let mut trial = net.clone();
                first.apply_to(&mut trial)?;
                second.apply_to(&mut trial)?;
                evaluated += 1;
                if oracle.exact(&trial)? && keep_ok(options.keep.as_ref(), &trial)? {
                    return Ok(Some(Repair {
                        patch: Patch::new(vec![first.clone(), second.clone()]),
                        before_loss,
                        after_loss: 0,
                        evaluated,
                    }));
                }
            }
        }
    }
    if options.max_edits < 2 {
        return Ok(None);
    }

    // 束搜索：从半径 1 的状态出发，逐层加一条编辑。
    seeds.sort_by(rank);
    seeds.truncate(options.beam_width.max(1));
    let mut beam = seeds;
    for _depth in 2..=options.max_edits {
        let mut next = Vec::new();
        for state in &beam {
            for candidate in &candidates {
                if state.patch.same_coord(candidate) {
                    continue;
                }
                let mut trial = state.net.clone();
                candidate.apply_to(&mut trial)?;
                let key = trial.state_hash();
                if !visited.insert(key) {
                    continue;
                }
                evaluated += 1;
                let loss = oracle.loss(&trial)?;
                if loss == 0 && keep_ok(options.keep.as_ref(), &trial)? {
                    let mut patch = state.patch.clone();
                    patch.push(candidate.clone());
                    return Ok(Some(Repair {
                        patch,
                        before_loss,
                        after_loss: 0,
                        evaluated,
                    }));
                }
                let energy = energy(&trial, oracle, options.keep.as_ref())?;
                let mut patch = state.patch.clone();
                patch.push(candidate.clone());
                next.push(State {
                    net: trial,
                    patch,
                    loss,
                    energy,
                    key,
                });
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort_by(rank);
        next.truncate(options.beam_width.max(1));
        beam = next;
    }
    Ok(None)
}

fn bfs(
    net: &Net,
    oracle: &Oracle,
    options: &RepairOptions,
    before_loss: usize,
) -> Result<Option<Repair>> {
    let candidates = scoped_candidates(net, options);
    let mut evaluated = 0usize;
    let mut visited: HashSet<u64> = HashSet::new();
    visited.insert(net.state_hash());
    let mut frontier: Vec<(Net, Patch)> = vec![(net.clone(), Patch::default())];
    for _depth in 1..=options.max_edits {
        let mut next = Vec::new();
        for (state, patch) in &frontier {
            for candidate in &candidates {
                if patch.same_coord(candidate) {
                    continue;
                }
                let mut trial = state.clone();
                candidate.apply_to(&mut trial)?;
                let key = trial.state_hash();
                if !visited.insert(key) {
                    continue;
                }
                evaluated += 1;
                let mut found = patch.clone();
                found.push(candidate.clone());
                if oracle.exact(&trial)? && keep_ok(options.keep.as_ref(), &trial)? {
                    return Ok(Some(Repair {
                        patch: found,
                        before_loss,
                        after_loss: 0,
                        evaluated,
                    }));
                }
                next.push((trial, found));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(None)
}

/// 保持集检查：`None` 恒通过。
fn keep_ok(keep: Option<&Oracle>, net: &Net) -> Result<bool> {
    match keep {
        Some(keep) => keep.exact(net),
        None => Ok(true),
    }
}

fn rank(left: &State, right: &State) -> std::cmp::Ordering {
    left.loss
        .cmp(&right.loss)
        .then_with(|| left.energy.total_cmp(&right.energy))
        .then_with(|| left.key.cmp(&right.key))
}
