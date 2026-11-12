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
use crate::NodeId;

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

/// 单次枚举的全局访问预算（`walk` 展开次数上限）：无论图多稠、`max_depth` 多深，
/// DFS 总工作量都被这道帽封住——满图 + 大深度 + 少链时原实现按 n! 爆炸（H6 未认证 DoS）。
pub const MAX_ATTACK_VISITS: usize = 200_000;
/// 深度上限：`walk` 是递归，`max_depth` 过大会爆栈又放大枚举；调用方传再大也夹到这里。
pub const MAX_ATTACK_DEPTH: usize = 64;
/// 链条数上限（输出规模可控）。
pub const MAX_ATTACK_CHAINS: usize = 4096;

/// 枚举从任一确认 `-1` 源到 `target` 的污染路径（跨仓 + 仓内因果 +
/// 锚定攻击分支三形态）。
pub fn attack_chains(
    fleet: &Fleet,
    target_repo: &str,
    target: NodeId,
    max_chains: usize,
    max_depth: usize,
) -> AttackChains {
    // 调用方（HTTP / 库）传入的 max_chains / max_depth 一律夹到硬顶：深度决定递归栈、
    // 链数决定输出规模——不放任外部指定（H6 未认证 DoS）。
    let max_chains = max_chains.min(MAX_ATTACK_CHAINS);
    let max_depth = max_depth.min(MAX_ATTACK_DEPTH);
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
    let mut budget = MAX_ATTACK_VISITS;
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
        &mut budget,
        max_depth,
    );
    out
}

mod walk;
use walk::walk;
