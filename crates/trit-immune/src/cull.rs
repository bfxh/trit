//! 自动淘汰规则：免疫系统的"负选择"——长期不动的病态节点自动处置。
//!
//! 免疫学类比：**克隆删除**（自我反应的 T 细胞被删除）和**耐受诱导**
//! （反复暴露于抗原而不产生应答 = 系统"习惯"了这个状态）。
//!
//! 三条规则（历史计数驱动，无需 LLM）：
//! 1. **慢性漏洞**：`-1` 在历史中出现 ≥ 阈值次且未修复 → 自动隔离（不再生成 Fix 任务）；
//! 2. **僵化未知**：`0` 在历史中出现 ≥ 阈值次 → 升级人工（不再留在调度队列）；
//! 3. **慢性设计 smell**：同节点 Refactor 建议 ≥ 阈值次 → 升级人工。
//!
//! 淘汰 ≠ 删除——是**状态跃迁**（从活跃队列移入终态），减少智能体的无效劳动。

use crate::dag::{ImmuneDag, NodeId};
use crate::trit::Trit;
use std::fmt;

/// 自动淘汰规则集。
#[derive(Debug, Clone)]
pub struct CullRules {
    /// 慢性漏洞阈值：同一节点 `-1` 出现次数 ≥ 此值 → 自动隔离。
    pub chronic_vulnerable: u32,
    /// 僵化未知阈值：同一节点 `0` 出现次数 ≥ 此值 → 升级人工。
    pub stale_unknown: u32,
    /// 慢性设计 smell 阈值：同节点 Refactor 建议 ≥ 此值 → 升级人工。
    pub chronic_refactor: u32,
    /// 设计层耦合阈值。
    pub max_fan_in: usize,
}

impl Default for CullRules {
    fn default() -> Self {
        Self {
            chronic_vulnerable: 3,
            stale_unknown: 5,
            chronic_refactor: 3,
            max_fan_in: 5,
        }
    }
}

/// 淘汰动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CullAction {
    /// 慢性漏洞 → 自动隔离（从活跃队列移除）。
    QuarantineChronic { node: NodeId, count: u32 },
    /// 僵化未知 → 升级人工。
    EscalateStale { node: NodeId, count: u32 },
    /// 慢性设计 smell → 升级人工。
    EscalateRefactor { node: NodeId, count: u32 },
}

impl fmt::Display for CullAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuarantineChronic { node, count } => {
                write!(f, "慢性漏洞 #{:02}（{count} 次 -1）→ 自动隔离", node.0)
            }
            Self::EscalateStale { node, count } => {
                write!(f, "僵化未知 #{:02}（{count} 次 0）→ 升级人工", node.0)
            }
            Self::EscalateRefactor { node, count } => {
                write!(f, "慢性设计 #{:02}（{count} 次重构建议）→ 升级人工", node.0)
            }
        }
    }
}

/// 统计一个节点的裁决历史分布。
fn verdict_counts(dag: &ImmuneDag, id: NodeId) -> (u32, u32, u32) {
    let n = dag.node(id);
    let mut v = 0u32;
    let mut u = 0u32;
    let mut h = 0u32;
    for e in &n.history {
        match e.verdict {
            Trit::Vulnerable => v += 1,
            Trit::Unknown => u += 1,
            Trit::Hardened => h += 1,
        }
    }
    (v, u, h)
}

/// 统计 Refactor 建议次数（从历史 note 中匹配）。
fn refactor_count(dag: &ImmuneDag, id: NodeId) -> u32 {
    dag.node(id)
        .history
        .iter()
        .filter(|e| e.note.contains("重构建议"))
        .count() as u32
}

/// 对 DAG 执行一轮自动淘汰：返回所有触发的动作。
/// **不自动删数据**——只做状态跃迁（隔离/标记），删数据由人决定。
pub fn cull(dag: &mut ImmuneDag, rules: &CullRules) -> Vec<CullAction> {
    let mut actions = Vec::new();
    let ids: Vec<NodeId> = dag.nodes().iter().map(|n| n.id).collect();
    for id in ids {
        if dag.node(id).quarantined {
            continue; // 已隔离的不重复
        }
        let (v, u, _) = verdict_counts(dag, id);

        // 规则 1：慢性漏洞
        if v >= rules.chronic_vulnerable && dag.node(id).verdict == Trit::Vulnerable {
            let count = v;
            dag.quarantine(id, &format!("自动淘汰：慢性漏洞（{count} 次 -1）"));
            actions.push(CullAction::QuarantineChronic { node: id, count });
            continue;
        }

        // 规则 2：僵化未知（多次 0 未变化）
        if u >= rules.stale_unknown && dag.node(id).verdict == Trit::Unknown {
            let count = u;
            dag.record(
                id,
                Trit::Unknown,
                None,
                &format!("自动淘汰：僵化未知（{count} 次 0）→ 升级人工"),
            );
            actions.push(CullAction::EscalateStale { node: id, count });
            continue;
        }

        // 规则 3：慢性设计 smell
        let rc = refactor_count(dag, id);
        if rc >= rules.chronic_refactor {
            let count = rc;
            dag.record(
                id,
                dag.node(id).verdict,
                None,
                &format!("自动淘汰：慢性设计 smell（{count} 次）→ 升级人工"),
            );
            actions.push(CullAction::EscalateRefactor { node: id, count });
        }
    }
    // 设计层耦合检查
    for id in dag.design_smells(rules.max_fan_in) {
        let fan = dag.coupling(id).fan_in;
        let rc = refactor_count(dag, id);
        if rc < rules.chronic_refactor {
            actions.push(CullAction::EscalateRefactor {
                node: id,
                count: fan as u32,
            });
        }
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;

    #[test]
    fn chronic_vulnerable_gets_quarantined() {
        let mut dag = ImmuneDag::root("main");
        let n = dag.spawn(NodeId(0), NodeKind::Delta, "长期未修");
        // 3 次_record(-1) → 慢性漏洞
        for i in 0..3 {
            dag.record(n, Trit::Vulnerable, None, &format!("扫描命中{i}"));
        }
        let rules = CullRules::default();
        let actions = cull(&mut dag, &rules);
        assert!(
            actions.iter().any(|a| matches!(a, CullAction::QuarantineChronic { node, .. } if *node == n)),
            "3 次 -1 应触发自动隔离"
        );
        assert!(dag.node(n).quarantined, "隔离标记生效");
    }

    #[test]
    fn not_chronic_if_below_threshold() {
        let mut dag = ImmuneDag::root("main");
        let n = dag.spawn(NodeId(0), NodeKind::Delta, "刚打穿");
        dag.record(n, Trit::Vulnerable, None, "打穿");
        dag.record(n, Trit::Hardened, None, "修复");
        dag.record(n, Trit::Vulnerable, None, "复发");
        // 历史: -1, +1, -1 → Vulnerable 只出现 1 次（当前 1 次），不到阈值 3
        let rules = CullRules::default();
        let actions = cull(&mut dag, &rules);
        assert!(
            !actions.iter().any(|a| matches!(a, CullAction::QuarantineChronic { node, .. } if *node == n)),
            "阈值未到不应隔离"
        );
    }

    #[test]
    fn stale_unknown_escalates_to_human() {
        let mut dag = ImmuneDag::root("main");
        let n = dag.spawn(NodeId(0), NodeKind::Delta, "长期搁置");
        // 5 次_record(0) → 僵化
        for i in 0..5 {
            dag.record(n, Trit::Unknown, None, &format!("扫描可疑{i}"));
        }
        let rules = CullRules::default();
        let actions = cull(&mut dag, &rules);
        assert!(
            actions.iter().any(|a| matches!(a, CullAction::EscalateStale { node, .. } if *node == n)),
            "5 次 0 应升级人工"
        );
        // 历史 +1（升级留痕）
        assert!(dag.node(n).history.last().unwrap().note.contains("升级人工"));
    }
}
