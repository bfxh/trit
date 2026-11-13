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

use crate::edit::{Edit, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

mod energy;
use energy::energy;

/// 修补预算。
#[derive(Clone, Debug, PartialEq)]
pub struct RepairOptions {
    /// 最多允许碰多少个三值位置。
    pub max_edits: usize,
    /// 束宽。
    pub beam_width: usize,
    /// 半径 2 穷举的候选对上限（超过就跳过，直接进束搜索）。
    pub exhaustive_pairs: usize,
    /// 状态空间预算：`3^权重数` 不超过它就改用完备 BFS（能给出最小编辑数）。
    pub exhaustive_states: u64,
    /// 候选编辑作用域；`None` 表示全权重穷举。给定时束搜索只在其中挑选。
    pub scope: Option<Vec<Edit>>,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            max_edits: 8,
            beam_width: 64,
            exhaustive_pairs: 20_000,
            exhaustive_states: 200_000,
            scope: None,
        }
    }
}

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
        if loss == 0 {
            return Ok(Some(Repair {
                patch: Patch::new(vec![candidate.clone()]),
                before_loss,
                after_loss: 0,
                evaluated,
            }));
        }
        let energy = energy(&trial, oracle)?;
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
                if oracle.exact(&trial)? {
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
                if loss == 0 {
                    let mut patch = state.patch.clone();
                    patch.push(candidate.clone());
                    return Ok(Some(Repair {
                        patch,
                        before_loss,
                        after_loss: 0,
                        evaluated,
                    }));
                }
                let energy = energy(&trial, oracle)?;
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
                if oracle.exact(&trial)? {
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

fn rank(left: &State, right: &State) -> std::cmp::Ordering {
    left.loss
        .cmp(&right.loss)
        .then_with(|| left.energy.total_cmp(&right.energy))
        .then_with(|| left.key.cmp(&right.key))
}

/// 候选编辑：有 scope 用 scope，否则全权重穷举。
pub(crate) fn scoped_candidates(net: &Net, options: &RepairOptions) -> Vec<Edit> {
    match &options.scope {
        Some(scope) => scope.clone(),
        None => candidates(net),
    }
}

/// 所有点编辑候选（每个三值位置的另外两个取值）。
pub(crate) fn candidates(net: &Net) -> Vec<Edit> {
    let mut out = Vec::new();
    for (layer, weights) in net.layers().iter().enumerate() {
        for row in 0..weights.rows() {
            for col in 0..weights.cols() {
                let current = weights.trit(row, col).expect("row/col in range");
                for &to in &Trit::ALL {
                    if to != current {
                        out.push(Edit::SetTrit {
                            layer,
                            row,
                            col,
                            to,
                        });
                    }
                }
            }
            let current = weights.bias_trit(row).expect("row in range");
            for &to in &Trit::ALL {
                if to != current {
                    out.push(Edit::SetBias { layer, row, to });
                }
            }
        }
    }
    out
}
