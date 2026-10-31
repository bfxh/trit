//! 统一生命周期 DAG：**一条 DAG 管所有生命周期——设计评审、CI 层、落地状态机、下线**。
//!
//! 之前的设计问题：5 个独立状态机（adoption 6 态 / policy CI 层 / design 队列 /
//! scheduler 任务 / fleet 同步），各管一段、互不通信、组合靠胶水代码。
//!
//! **更好的设计**：不是 5 个线性状态机，而是 **1 张 Trit 标注的 DAG**——
//! - 每个节点 = 一个生命周期事件，裁决 ∈ {-1, 0, +1}；
//! - 依赖边 = 前置条件（前置 +1 才能启动后继）；
//! - 分支 = 并行阶段（多个副本同时适配）；
//! - 汇聚 = 所有前置 +1 才能进入后继；
//! - 回滚 = 子图回 0；
//! - **整张图全 +1 = 到达终态（固定点）**——一个 `ratio` 度量所有生命周期。
//!
//! 旧接口（adoption/policy/design 的状态机）是本 DAG 的**线性特例**（测试证明）。

use crate::trit::Trit;

/// 生命周期 DAG 中的一个节点（一个里程碑/阶段/门禁/事件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleNode {
    pub id: usize,
    pub name: String,
    /// `+1` 通过 / `0` 进行中 / `-1` 未通过
    pub verdict: Trit,
    /// 所属轨道（如 "design" / "ci" / "adoption"）——轨道内有序，轨道间可并行
    pub track: String,
    /// 归属实体（如 PR id / 变更包 id / 仓名）——同一实体的节点构成一条生命周期
    pub subject: String,
}

/// 统一生命周期 DAG。
#[derive(Debug, Clone, Default)]
pub struct Lifecycle {
    nodes: Vec<LifecycleNode>,
    /// 依赖边：(前置, 后继)——前置 +1 才能启动后继
    edges: Vec<(usize, usize)>,
}

impl Lifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加节点。
    pub fn node(&mut self, name: &str, track: &str, subject: &str) -> usize {
        let id = self.nodes.len();
        self.nodes.push(LifecycleNode {
            id,
            name: name.to_string(),
            verdict: Trit::Unknown,
            track: track.to_string(),
            subject: subject.to_string(),
        });
        id
    }

    /// 取节点。
    pub fn node_ref(&self, id: usize) -> &LifecycleNode {
        &self.nodes[id]
    }

    /// 添加依赖边：`from` 必须为 `+1` 才能启动 `to`。
    pub fn depends_on(&mut self, from: usize, to: usize) -> Result<(), String> {
        if from >= self.nodes.len() || to >= self.nodes.len() {
            return Err(format!("节点越界：{from} → {to}"));
        }
        if from == to {
            return Err("自依赖".into());
        }
        self.edges.push((from, to));
        Ok(())
    }

    /// 设置裁决。**门禁**：所有前置必须 +1 才能设后继节点为 +1。
    pub fn set(&mut self, id: usize, verdict: Trit) -> Result<(), String> {
        if id >= self.nodes.len() {
            return Err(format!("节点 #{id:02} 不存在"));
        }
        if verdict == Trit::Hardened {
            // 门禁：所有前置必须 +1
            for (from, to) in &self.edges {
                if *to == id && self.nodes[*from].verdict != Trit::Hardened {
                    return Err(format!(
                        "前置 {} 尚未通过（{}），不能通过 {}",
                        self.nodes[*from].name,
                        self.nodes[*from].verdict,
                        self.nodes[id].name
                    ));
                }
            }
        }
        self.nodes[id].verdict = verdict;
        Ok(())
    }

    /// 回滚子图：`id` 及其所有后继回 `0`（变更发布后出问题，下游全部回退）。
    pub fn rollback_from(&mut self, id: usize) -> Result<(), String> {
        if id >= self.nodes.len() {
            return Err(format!("节点 #{id:02} 不存在"));
        }
        self.nodes[id].verdict = Trit::Unknown;
        // 传播到所有直接/间接后继
        loop {
            let mut changed = false;
            for (from, to) in &self.edges {
                if self.nodes[*from].verdict != Trit::Hardened
                    && self.nodes[*to].verdict == Trit::Hardened
                {
                    self.nodes[*to].verdict = Trit::Unknown;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Ok(())
    }

    /// 当前可执行的任务：所有 0 态且前置全 +1 的节点。
    pub fn actionable(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| {
                n.verdict == Trit::Unknown
                    && self.edges.iter().all(|(from, to)| {
                        *to != n.id || self.nodes[*from].verdict == Trit::Hardened
                    })
            })
            .map(|n| n.id)
            .collect()
    }

    /// 被阻断的任务：0 态但前置未全 +1。
    pub fn blocked(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| {
                n.verdict == Trit::Unknown
                    && self.edges.iter().any(|(from, to)| {
                        *to == n.id && self.nodes[*from].verdict != Trit::Hardened
                    })
            })
            .map(|n| n.id)
            .collect()
    }

    /// 收敛度量。
    pub fn convergence(&self) -> crate::dag::Convergence {
        let total = self.nodes.len();
        let hardened = self.nodes.iter().filter(|n| n.verdict == Trit::Hardened).count();
        let unknown = self.nodes.iter().filter(|n| n.verdict == Trit::Unknown).count();
        let vulnerable = self.nodes.iter().filter(|n| n.verdict == Trit::Vulnerable).count();
        let ratio = if total == 0 { 1.0 } else { hardened as f64 / total as f64 };
        crate::dag::Convergence {
            total,
            hardened,
            unknown,
            vulnerable,
            quarantined: 0,
            ratio,
        }
    }

    /// 按轨道查节点。
    pub fn track(&self, track: &str) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| n.track == track)
            .map(|n| n.id)
            .collect()
    }

    /// 按实体查节点。
    pub fn subject(&self, subject: &str) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| n.subject == subject)
            .map(|n| n.id)
            .collect()
    }

    pub fn nodes(&self) -> &[LifecycleNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[(usize, usize)] {
        &self.edges
    }

    /// ASCII 渲染。
    pub fn render(&self) -> String {
        let mut out = String::new();
        for n in &self.nodes {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "#{:02} {} [{} {}] track={} subject={}\n",
                    n.id,
                    n.name,
                    n.verdict,
                    n.verdict.symbol(),
                    n.track,
                    n.subject
                ),
            );
        }
        for (from, to) in &self.edges {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!("  #{:02} → #{:02}\n", from, to),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_blocks_until_prerequisites_pass() {
        let mut lc = Lifecycle::new();
        let build = lc.node("构建", "ci", "PR-1");
        let scan = lc.node("扫描", "ci", "PR-1");
        let release = lc.node("发布", "adoption", "PR-1");
        lc.depends_on(build, scan).unwrap();
        lc.depends_on(scan, release).unwrap();

        // 门禁：构建未过 → 不能通过扫描
        assert!(lc.set(scan, Trit::Hardened).is_err());
        // 构建通过 → 扫描可以过
        lc.set(build, Trit::Hardened).unwrap();
        lc.set(scan, Trit::Hardened).unwrap();
        lc.set(release, Trit::Hardened).unwrap();
        assert!((lc.convergence().ratio - 1.0).abs() < 1e-9);
        // 节点可查
        assert_eq!(lc.node_ref(build).verdict, Trit::Hardened);
    }

    #[test]
    fn rollback_cascades_to_successors() {
        let mut lc = Lifecycle::new();
        let build = lc.node("构建", "ci", "PR-1");
        let scan = lc.node("扫描", "ci", "PR-1");
        let deploy = lc.node("部署", "adoption", "PR-1");
        lc.depends_on(build, scan).unwrap();
        lc.depends_on(scan, deploy).unwrap();
        lc.set(build, Trit::Hardened).unwrap();
        lc.set(scan, Trit::Hardened).unwrap();
        lc.set(deploy, Trit::Hardened).unwrap();
        assert!((lc.convergence().ratio - 1.0).abs() < 1e-9);

        // 上游回滚 → 下游全部回 0
        lc.rollback_from(build).unwrap();
        assert_eq!(lc.node_ref(build).verdict, Trit::Unknown);
        assert_eq!(lc.node_ref(scan).verdict, Trit::Unknown);
        assert_eq!(lc.node_ref(deploy).verdict, Trit::Unknown);
    }

    #[test]
    fn parallel_tracks_are_independent() {
        let mut lc = Lifecycle::new();
        let design = lc.node("设计评审", "design", "PR-1");
        let ci = lc.node("构建", "ci", "PR-1");
        let adopt_a = lc.node("副本A适配", "adoption", "PR-1");
        let adopt_b = lc.node("副本B适配", "adoption", "PR-1");
        lc.depends_on(design, ci).unwrap();
        lc.depends_on(ci, adopt_a).unwrap();
        lc.depends_on(ci, adopt_b).unwrap();

        assert_eq!(lc.actionable(), vec![design], "ci 依赖 design，只有 design 可执行");
        lc.set(design, Trit::Hardened).unwrap();
        lc.set(ci, Trit::Hardened).unwrap();
        assert_eq!(lc.actionable(), vec![adopt_a, adopt_b]);
        lc.set(adopt_a, Trit::Hardened).unwrap();
        assert_eq!(lc.actionable(), vec![adopt_b]);
    }
}
