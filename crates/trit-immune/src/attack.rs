//! 多跳攻击链枚举（fleet 级渗透路径报告，`dag::breach_sources` 的跨仓完全体）。
//!
//! 从 `Fleet` 拆出（上游 module budget：单文件 ≤800 行）——枚举逻辑纯图搜索，
//! 只读 Fleet；[`crate::fleet::Fleet::attack_chains`] 是薄委托保持方法路径。
//!
//! 语义（与传播引擎严格对齐）：
//! - **源判定 = 节点自身 verdict 为 `-1`**——跨仓反熵
//!   （`Fleet::sync_cross_repo`）已把污染落账进目标节点自身，所以
//!   "最近的确认源"直接从账本读；
//! - **只有确认穿透边（pass = `-1`）传播打穿**——(min,max) 语义下
//!   max(源 `-1`, 边) ≤ 0 的边都不传 `-1`；
//! - 可疑节点（自身 `0`）不 emit 但**继续追根**（上游可能有确认源）；
//! - 自身 `+1` 的节点**挡路即停**（确认源不会在它上游——污染没穿过它）；
//! - 锚定攻击分支（`AttackVector` 且 `-1`）是叶子源；
//! - 深度/条数双上限防爆炸，截断如实报告（[`AttackChains::truncated`]）。

use crate::fleet::Fleet;
use crate::{NodeId, Trit};

/// 攻击链的一步：节点 + **进入该节点的方式**（首步 = 源标记 [`ChainVia::Source`]；
/// target 步骤的 via = 进入 target 的边——攻击方向上最后一跳）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainStep {
    pub node: (String, NodeId),
    pub via: ChainVia,
}

/// 进入节点的方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainVia {
    /// 源（确认 `-1`，链的起点）。
    Source,
    /// 仓内因果链边（parent → child）。
    Causal,
    /// 锚定攻击分支打穿锚点。
    AnchoredAttack,
    /// 跨仓确认穿透边。
    CrossLink,
}

/// 一条完整攻击链：源 → … → target（顺序即攻击方向）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttackChain {
    pub steps: Vec<ChainStep>,
}

/// 枚举结果：链集合 + 截断标志（深度/条数/环任一触发即如实报告）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttackChains {
    pub chains: Vec<AttackChain>,
    pub truncated: bool,
}

/// 枚举从任一确认 `-1` 源到 `target` 的污染路径（跨仓 + 仓内因果 +
/// 锚定攻击分支三形态）。
pub fn attack_chains(
    fleet: &Fleet,
    target_repo: &str,
    target: NodeId,
    max_chains: usize,
    max_depth: usize,
) -> AttackChains {
    let mut out = AttackChains {
        chains: Vec::new(),
        truncated: false,
    };
    let Some(j) = fleet.get(target_repo) else {
        return out;
    };
    if target.0 as usize >= j.dag().nodes().len() {
        return out;
    }
    let mut nodes: Vec<(String, NodeId)> = vec![(target_repo.to_string(), target)];
    let mut edges: Vec<ChainVia> = vec![ChainVia::Source];
    let mut visited: std::collections::BTreeSet<(String, u32)> = std::collections::BTreeSet::new();
    walk(
        fleet,
        target_repo,
        target,
        true,
        &mut nodes,
        &mut edges,
        &mut visited,
        &mut out,
        max_chains,
        max_depth,
    );
    out
}

/// DFS 反向走图：`nodes/edges` 是从 target 到当前的路径（`edges[0]` 为
/// 起点标记）。`first` = 当前节点是 target（无条件展开）。
#[allow(clippy::too_many_arguments)]
fn walk(
    fleet: &Fleet,
    repo: &str,
    id: NodeId,
    first: bool,
    nodes: &mut Vec<(String, NodeId)>,
    edges: &mut Vec<ChainVia>,
    visited: &mut std::collections::BTreeSet<(String, u32)>,
    out: &mut AttackChains,
    max_chains: usize,
    depth_left: usize,
) {
    if !visited.insert((repo.to_string(), id.0)) {
        out.truncated = true; // 环：如实截断
        return;
    }
    let j = fleet.get(repo).expect("walk：仓必须存在");
    let dag = j.dag();
    let verdict = dag.node(id).verdict;

    // 源判定：自身 -1（target 自身 -1 也是源——没有已知上游注入）
    if !first && verdict == Trit::Vulnerable {
        // 条数预算：耗尽则如实截断，不再产生新链
        if out.chains.len() >= max_chains {
            out.truncated = true;
            visited.remove(&(repo.to_string(), id.0));
            return;
        }
        // 攻击方向序列 = [n_k(源), n_{k-1}, …, n_0(target)]；进入 n_i 的边
        // = edges[i+1]（edges[j] 连接 n_{j-1} 与 n_j，即逆走第 j 步用的边）
        let k = nodes.len() - 1;
        let mut steps: Vec<ChainStep> = Vec::with_capacity(nodes.len());
        steps.push(ChainStep {
            node: nodes[k].clone(),
            via: ChainVia::Source,
        });
        for i in (0..k).rev() {
            steps.push(ChainStep {
                node: nodes[i].clone(),
                via: edges[i + 1],
            });
        }
        out.chains.push(AttackChain { steps });
        if out.chains.len() >= max_chains {
            out.truncated = true;
        }
        visited.remove(&(repo.to_string(), id.0));
        return;
    }
    // 自身 +1（非 target）挡路：污染没穿过它，上游不属打穿路径
    if !first && verdict == Trit::Hardened {
        visited.remove(&(repo.to_string(), id.0));
        return;
    }
    // 深度耗尽：如实截断
    if depth_left == 0 {
        out.truncated = true;
        visited.remove(&(repo.to_string(), id.0));
        return;
    }

    // 展开上游：① 因果链 parent；② 锚定攻击分支（叶子源）；③ 确认穿透跨仓边
    let n = dag.node(id);
    if let Some(p) = n.parent {
        nodes.push((repo.to_string(), p));
        edges.push(ChainVia::Causal);
        walk(
            fleet,
            repo,
            p,
            false,
            nodes,
            edges,
            visited,
            out,
            max_chains,
            depth_left - 1,
        );
        nodes.pop();
        edges.pop();
    }
    for a in dag.anchored_attacks(id) {
        if dag.node(a).verdict == Trit::Vulnerable {
            if out.chains.len() >= max_chains {
                out.truncated = true;
                break;
            }
            // 攻击分支 a 是叶子源，打进锚点 n_k = 路径末端：
            // 序列 = [a(源), n_k(锚点), …, n_0(target)]；进入锚点 = AnchoredAttack，
            // 其余同 causal 构造（进入 n_i = edges[i+1]）
            let k = nodes.len() - 1;
            let mut steps: Vec<ChainStep> = Vec::with_capacity(nodes.len() + 1);
            steps.push(ChainStep {
                node: (repo.to_string(), a),
                via: ChainVia::Source,
            });
            for i in (0..=k).rev() {
                let via = if i == k {
                    ChainVia::AnchoredAttack
                } else {
                    edges[i + 1]
                };
                steps.push(ChainStep {
                    node: nodes[i].clone(),
                    via,
                });
            }
            out.chains.push(AttackChain { steps });
            if out.chains.len() >= max_chains {
                out.truncated = true;
            }
        }
    }
    for l in fleet.cross_links() {
        if l.to.0 == repo
            && l.to.1 == id
            && l.pass == Trit::Vulnerable
            && fleet.get(&l.from.0).is_some()
        {
            nodes.push(l.from.clone());
            edges.push(ChainVia::CrossLink);
            walk(
                fleet,
                &l.from.0,
                l.from.1,
                false,
                nodes,
                edges,
                visited,
                out,
                max_chains,
                depth_left - 1,
            );
            nodes.pop();
            edges.pop();
        }
    }
    visited.remove(&(repo.to_string(), id.0));
}
