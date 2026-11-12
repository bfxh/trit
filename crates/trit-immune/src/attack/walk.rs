//! 攻击链 DFS（从 src/attack.rs 拆出，控制单文件行数预算）。

use super::{AttackChain, AttackChains, ChainStep, ChainVia};
use crate::fleet::Fleet;
use crate::{NodeId, Trit};

/// DFS 反向走图：`nodes/edges` 是从 target 到当前的路径（`edges[0]` 为
/// 起点标记）。`first` = 当前节点是 target（无条件展开）。
#[allow(clippy::too_many_arguments)]
pub(super) fn walk(
    fleet: &Fleet,
    repo: &str,
    id: NodeId,
    first: bool,
    nodes: &mut Vec<(String, NodeId)>,
    edges: &mut Vec<ChainVia>,
    visited: &mut std::collections::BTreeSet<(String, u32)>,
    out: &mut AttackChains,
    max_chains: usize,
    budget: &mut usize,
    depth_left: usize,
) {
    // 全局访问预算：耗尽即如实截断，杜绝满图 + 大深度下的 n! 枚举（H6）。
    if *budget == 0 {
        out.truncated = true;
        return;
    }
    *budget -= 1;
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
            budget,
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
    for l in &fleet.cross_edges() {
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
                budget,
                depth_left - 1,
            );
            nodes.pop();
            edges.pop();
        }
    }
    visited.remove(&(repo.to_string(), id.0));
}
