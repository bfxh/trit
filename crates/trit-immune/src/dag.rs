//! DeltaDB 式因果 DAG + 免疫语义。
//!
//! - **分支即攻击向量**：红队 Agent 在任意锚点零成本拉出 `NodeKind::AttackVector` 分支；
//! - **负数消息回链**：攻击成功时，`ExploitEvidence` 挂在攻击节点上，`target` 指回被打穿的代码节点；
//! - **因果链隔离**：`Message` 节点被 Prompt 注入污染时，`quarantine` 沿后代传播；
//! - **合并门禁**：`merge_gate(tip)` 对"因果链 + 链上锚定的攻击分支"做 K3 AND 折叠；
//! - **Delta 编码**：所有变更都走 [`ImmuneDag::apply`]——实时写入与日志重放同一条代码路径。

use crate::delta::{ApplyError, Delta};
use crate::propagate::Contagion;
use crate::trit::Trit;
use crate::verdict::{merge_gate, ExploitEvidence, GateDecision};
use std::fmt::Write as _;

/// 节点编号：DAG 内自增，从 0（`main`）起。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// 节点种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// 开发 Agent 的代码增量。
    Delta,
    /// 红队 Agent 的攻击分支（分支即攻击向量）。
    AttackVector,
    /// 修复补丁。
    Patch,
    /// 对话/消息节点——Prompt 注入的污染源与溯源对象。
    Message,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Delta => "DELTA",
            NodeKind::AttackVector => "ATTACK",
            NodeKind::Patch => "PATCH",
            NodeKind::Message => "MSG",
        }
    }
}

impl std::str::FromStr for NodeKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DELTA" => Ok(NodeKind::Delta),
            "ATTACK" => Ok(NodeKind::AttackVector),
            "PATCH" => Ok(NodeKind::Patch),
            "MSG" => Ok(NodeKind::Message),
            other => Err(format!("未知节点类型：{other}")),
        }
    }
}

/// 一次裁决事件；同一节点可多次裁决（攻击成功 → 补丁 → 重放失败），历史即对抗时间线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictEvent {
    pub verdict: Trit,
    pub evidence: Option<ExploitEvidence>,
    pub note: String,
}

/// DAG 节点：一次代码增量、一次攻击、一条消息……
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub kind: NodeKind,
    pub title: String,
    /// 当前裁决。新节点默认 `0`：**未测即不可信**。
    pub verdict: Trit,
    /// 因果链隔离标记（Prompt 注入污染），只能由人工复核解除；不抹掉原裁决。
    pub quarantined: bool,
    pub history: Vec<VerdictEvent>,
}

/// 跨树因果边：依赖关系 / 共享污染消息等非树因果，参与三值传播。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossLink {
    pub from: NodeId,
    pub to: NodeId,
    /// 通过强度：`-1` 确认可穿透 / `0` 疑似可穿透 / `+1` 已证拦截（视为断开）。
    pub pass: Trit,
    pub note: String,
}

/// 结构耦合视图：扇入/扇出 + 入/出边最坏通过值（空集 = `+1` 无嫌疑）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coupling {
    pub fan_in: usize,
    pub fan_out: usize,
    /// 入边最坏通过值：`-1` = 有人确认能打进来
    pub inbound: Trit,
    /// 出边最坏通过值：`-1` = 确认能从这里打出去
    pub outbound: Trit,
}

/// 收敛度量：距"全 `+1` 固定点"的距离（`docs/GOAL.md`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Convergence {
    pub total: usize,
    pub hardened: usize,
    pub unknown: usize,
    pub vulnerable: usize,
    pub quarantined: usize,
    /// hardened / total；空网络视为 1.0（无可收敛项）
    pub ratio: f64,
}

/// 打穿路径种类（[`ImmuneDag::breach_sources`] 的路径标注）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreachKind {
    /// 沿因果链的祖先节点 `-1`（代码依赖链上的洞）。
    Ancestor,
    /// 锚定攻击分支 `-1`（红队在此锚点打穿）。
    AnchoredAttack,
    /// 跨树污染边 `-1`（外部依赖/共享消息把洞带进来）。
    CrossLink,
}

/// 免疫网络：节点数组 + 因果边（parent）+ 跨树污染边。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmuneDag {
    nodes: Vec<Node>,
    links: Vec<CrossLink>,
}

impl Default for ImmuneDag {
    fn default() -> Self {
        Self::empty()
    }
}

impl ImmuneDag {
    /// 空 DAG（配合 [`ImmuneDag::apply`] 做日志重放）。
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            links: Vec::new(),
        }
    }

    /// 从完整状态重建（快照恢复用）：校验 id 连续、父引用、证据目标与跨树边端点。
    pub fn from_parts(nodes: Vec<Node>, links: Vec<CrossLink>) -> Result<Self, String> {
        for (i, n) in nodes.iter().enumerate() {
            if n.id.0 as usize != i {
                return Err(format!(
                    "节点 id 不连续：第 {i} 个应是 #{i:02}，实际 #{:02}",
                    n.id.0
                ));
            }
            if let Some(p) = n.parent {
                if p.0 as usize >= i {
                    return Err(format!(
                        "#{:02} 的父节点 #{:02} 非法（必须先于自身存在）",
                        n.id.0, p.0
                    ));
                }
            }
            for ev in &n.history {
                if let Some(e) = &ev.evidence {
                    if e.target.0 as usize >= nodes.len() {
                        return Err(format!(
                            "#{:02} 的证据指向不存在的 #{:02}",
                            n.id.0, e.target.0
                        ));
                    }
                }
            }
        }
        for l in &links {
            if l.from.0 as usize >= nodes.len() || l.to.0 as usize >= nodes.len() {
                return Err(format!(
                    "跨树边 #{:02}→#{:02} 端点不存在",
                    l.from.0, l.to.0
                ));
            }
        }
        Ok(Self { nodes, links })
    }

    /// 新建网络，`#0` 为 `main` 根节点，基线裁决 `+1`（可信主干）。
    pub fn root(title: &str) -> Self {
        let mut dag = Self::empty();
        dag.apply(&Delta::Root {
            title: title.to_string(),
        })
        .expect("root：只能建在空 DAG 上");
        dag
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// 越界 id 会 panic（DAG 内部自增 id，正常使用不会越界）。
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0 as usize]
    }

    /// 零成本拉分支：开发增量、攻击向量、补丁、消息都只是新节点。
    /// 新节点默认裁决 `0`（未知）。
    pub fn spawn(&mut self, parent: NodeId, kind: NodeKind, title: &str) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.apply(&Delta::Spawn {
            parent,
            kind,
            title: title.to_string(),
        })
        .expect("spawn：父节点必须存在");
        id
    }

    /// 红队/蓝队/沙盒写入裁决；攻击成功时把 `ExploitEvidence` 作为负数消息挂在攻击节点上
    /// （`evidence.target` 指回被打穿的代码节点）。
    pub fn record(
        &mut self,
        id: NodeId,
        verdict: Trit,
        evidence: Option<ExploitEvidence>,
        note: &str,
    ) {
        self.apply(&Delta::Record {
            id,
            verdict,
            evidence,
            note: note.to_string(),
        })
        .expect("record：节点必须存在");
    }

    /// Prompt 注入溯源：把污染节点及其全部后代打上隔离标记（不抹掉原裁决，
    /// 只是"整条链在人工复核前不得合并"）。
    pub fn quarantine(&mut self, id: NodeId, note: &str) {
        self.apply(&Delta::Quarantine {
            id,
            note: note.to_string(),
        })
        .expect("quarantine：节点必须存在");
    }

    /// 人工裁决：解除该节点及整棵子树的隔离，并落锤当前节点。
    /// 人类是"0 态"的主宰——0 由人推向 ±1。
    pub fn human_review(&mut self, id: NodeId, verdict: Trit, note: &str) {
        self.apply(&Delta::HumanReview {
            id,
            verdict,
            note: note.to_string(),
        })
        .expect("human_review：节点必须存在");
    }

    /// 声明一条跨树因果边（依赖 / 共享污染消息）。
    /// `pass`：`-1` 确认可穿透 / `0` 疑似可穿透 / `+1` 已证拦截。
    pub fn cross_link(&mut self, from: NodeId, to: NodeId, pass: Trit, note: &str) {
        self.apply(&Delta::CrossLink {
            from,
            to,
            pass,
            note: note.to_string(),
        })
        .expect("cross_link：两端节点必须存在");
    }

    pub fn links(&self) -> &[CrossLink] {
        &self.links
    }

    /// 应用一条 Delta——实时写入与日志重放共用同一条代码路径。
    pub fn apply(&mut self, delta: &Delta) -> Result<(), ApplyError> {
        match delta {
            Delta::Root { title } => {
                if !self.nodes.is_empty() {
                    return Err(ApplyError::RootAlreadyExists);
                }
                self.nodes.push(Node {
                    id: NodeId(0),
                    parent: None,
                    kind: NodeKind::Delta,
                    title: title.clone(),
                    verdict: Trit::Hardened,
                    quarantined: false,
                    history: vec![VerdictEvent {
                        verdict: Trit::Hardened,
                        evidence: None,
                        note: "主干基线".into(),
                    }],
                });
                Ok(())
            }
            Delta::Spawn { parent, kind, title } => {
                if parent.0 as usize >= self.nodes.len() {
                    return Err(ApplyError::UnknownParent { parent: *parent });
                }
                let id = NodeId(self.nodes.len() as u32);
                self.nodes.push(Node {
                    id,
                    parent: Some(*parent),
                    kind: *kind,
                    title: title.clone(),
                    verdict: Trit::Unknown,
                    quarantined: false,
                    history: Vec::new(),
                });
                Ok(())
            }
            Delta::Record {
                id,
                verdict,
                evidence,
                note,
            } => {
                let idx = id.0 as usize;
                if idx >= self.nodes.len() {
                    return Err(ApplyError::UnknownNode { id: *id });
                }
                if let Some(ev) = evidence {
                    if ev.target.0 as usize >= self.nodes.len() {
                        return Err(ApplyError::UnknownTarget { id: ev.target });
                    }
                }
                let node = &mut self.nodes[idx];
                node.verdict = *verdict;
                node.history.push(VerdictEvent {
                    verdict: *verdict,
                    evidence: evidence.clone(),
                    note: note.clone(),
                });
                Ok(())
            }
            Delta::Quarantine { id, note } => {
                if id.0 as usize >= self.nodes.len() {
                    return Err(ApplyError::UnknownNode { id: *id });
                }
                let mut stack = vec![*id];
                while let Some(cur) = stack.pop() {
                    {
                        let node = &mut self.nodes[cur.0 as usize];
                        if !node.quarantined {
                            node.quarantined = true;
                            node.history.push(VerdictEvent {
                                verdict: node.verdict,
                                evidence: None,
                                note: format!("隔离：{note}"),
                            });
                        }
                    }
                    stack.extend(self.children(cur));
                }
                Ok(())
            }
            Delta::HumanReview { id, verdict, note } => {
                if id.0 as usize >= self.nodes.len() {
                    return Err(ApplyError::UnknownNode { id: *id });
                }
                let mut stack = vec![*id];
                while let Some(cur) = stack.pop() {
                    self.nodes[cur.0 as usize].quarantined = false;
                    stack.extend(self.children(cur));
                }
                let node = &mut self.nodes[id.0 as usize];
                node.verdict = *verdict;
                node.history.push(VerdictEvent {
                    verdict: *verdict,
                    evidence: None,
                    note: note.clone(),
                });
                Ok(())
            }
            Delta::CrossLink {
                from,
                to,
                pass,
                note,
            } => {
                if from.0 as usize >= self.nodes.len() {
                    return Err(ApplyError::UnknownNode { id: *from });
                }
                if to.0 as usize >= self.nodes.len() {
                    return Err(ApplyError::UnknownNode { id: *to });
                }
                self.links.push(CrossLink {
                    from: *from,
                    to: *to,
                    pass: *pass,
                    note: note.clone(),
                });
                Ok(())
            }
        }
    }

    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|n| n.parent == Some(id))
            .map(|n| n.id)
            .collect()
    }

    /// `root → tip` 的因果链。
    pub fn chain(&self, tip: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = Some(tip);
        while let Some(c) = cur {
            out.push(c);
            cur = self.nodes[c.0 as usize].parent;
        }
        out.reverse();
        out
    }

    /// 锚在节点上的红队攻击分支（"分支即攻击向量"）。
    pub fn anchored_attacks(&self, id: NodeId) -> Vec<NodeId> {
        self.children(id)
            .into_iter()
            .filter(|&c| self.nodes[c.0 as usize].kind == NodeKind::AttackVector)
            .collect()
    }

    /// 污染源：所有非 `+1` 或被隔离的节点。
    /// 确证漏洞以 `-1` 强度外泄，可疑/隔离节点以 `0` 强度外泄。
    fn sources(&self) -> Vec<(usize, Trit)> {
        self.nodes
            .iter()
            .filter(|n| n.quarantined || n.verdict != Trit::Hardened)
            .map(|n| {
                let sev = if n.quarantined {
                    n.verdict.and(Trit::Unknown)
                } else {
                    n.verdict
                };
                (n.id.0 as usize, sev)
            })
            .collect()
    }

    /// 三值污染传播：跨树 `CrossLink` 边上的 (min, max) 半环传播。
    /// 返回按节点下标对齐的污染等级（不可达 = `+1`）。
    pub fn contamination(&self) -> Vec<Trit> {
        let mut g = Contagion::new(self.nodes.len());
        for l in &self.links {
            g.link(l.from.0 as usize, l.to.0 as usize, l.pass);
        }
        g.spread(&self.sources())
    }

    /// 有效裁决 = min(自身裁决, 污染等级)——K3 AND 恰是数值 min。
    pub fn effective_verdicts(&self) -> Vec<Trit> {
        let c = self.contamination();
        self.nodes
            .iter()
            .map(|n| n.verdict.and(c[n.id.0 as usize]))
            .collect()
    }

    /// 合并门禁：对 `tip` 的因果链 + 链上每个节点锚定的攻击分支做 K3 AND 折叠，
    /// 折叠的是**有效裁决**（自身裁决 ∧ 跨树污染传播结果）。
    /// 任一 `-1` → `BLOCK`；任一 `0` → `FREEZE`；全 `+1` → `ALLOW`。
    /// 链上存在隔离节点时，`ALLOW` 降级为 `FREEZE`（等人工解除）。
    pub fn merge_gate(&self, tip: NodeId) -> GateDecision {
        let contamination = self.contamination();
        let mut verdicts = Vec::new();
        let mut quarantined = false;
        for n in self.chain(tip) {
            let node = &self.nodes[n.0 as usize];
            verdicts.push(node.verdict.and(contamination[n.0 as usize]));
            quarantined |= node.quarantined;
            for a in self.anchored_attacks(n) {
                let atk = &self.nodes[a.0 as usize];
                verdicts.push(atk.verdict.and(contamination[a.0 as usize]));
                quarantined |= atk.quarantined;
            }
        }
        let decision = merge_gate(verdicts);
        if quarantined && decision == GateDecision::Allow {
            GateDecision::Freeze
        } else {
            decision
        }
    }

    /// 红队/沙盒的待办队列：所有仍处 `0` 态的节点。
    pub fn suspects(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|n| n.verdict == Trit::Unknown)
            .map(|n| n.id)
            .collect()
    }

    /// **矛盾节点**（Belnap FOUR 的 ⊥"both" 态视图）：历史里既有 `-1` 又有
    /// `+1`、且未被隔离。K3 AND 折叠把矛盾压成坏消息支配，但**矛盾本身是
    /// 信息**——同一节点被某来源判坏、又被另一来源判好，值得人看一眼
    /// （0 态候选，与 `design_smells` 同一消费姿势：机械判据替人眼挡噪音）。
    /// 隔离节点不重复上报（已有人看）。
    pub fn contradictions(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|n| {
                !n.quarantined
                    && n.history.iter().any(|e| e.verdict == Trit::Vulnerable)
                    && n.history.iter().any(|e| e.verdict == Trit::Hardened)
            })
            .map(|n| n.id)
            .collect()
    }

    /// **打穿来源回溯**（Sheyner 攻击图的极简形态）：列出全部把 `target`
    /// 拉向 `-1` 的来源与路径种类——与 [`Self::merge_gate`] 折叠的同一批
    /// 来源严格对齐（因果链祖先的 `-1` / 锚定攻击分支的 `-1` / 指向 target
    /// 的 `-1` 跨树边），因此"门禁为什么坏"与"红队渗透路径"是同一份账的
    /// 两种读法。返回顺序：因果链序在前、跨树边在后。
    pub fn breach_sources(&self, target: NodeId) -> Vec<(NodeId, BreachKind)> {
        let cont = self.contamination();
        let mut out: Vec<(NodeId, BreachKind)> = Vec::new();
        let push = |id: NodeId, kind: BreachKind, out: &mut Vec<(NodeId, BreachKind)>| {
            if !out.iter().any(|(x, k)| *x == id && *k == kind) {
                out.push((id, kind));
            }
        };
        for n in self.chain(target) {
            let node = &self.nodes[n.0 as usize];
            if node.verdict.and(cont[n.0 as usize]) == Trit::Vulnerable {
                push(n, BreachKind::Ancestor, &mut out);
            }
            for a in self.anchored_attacks(n) {
                let atk = &self.nodes[a.0 as usize];
                if atk.verdict.and(cont[a.0 as usize]) == Trit::Vulnerable {
                    push(a, BreachKind::AnchoredAttack, &mut out);
                }
            }
        }
        for l in &self.links {
            if l.to == target {
                let src = &self.nodes[l.from.0 as usize];
                let inflow = src.verdict.and(cont[l.from.0 as usize]).max(l.pass);
                if inflow == Trit::Vulnerable {
                    push(l.from, BreachKind::CrossLink, &mut out);
                }
            }
        }
        out
    }

    /// 结构耦合视图：某节点的跨树边扇入/扇出与最坏边强度（三值）。
    /// 耦合直接从因果图上读——不需要智能体告诉你耦不耦合。
    pub fn coupling(&self, id: NodeId) -> Coupling {
        let (mut fan_in, mut fan_out) = (0usize, 0usize);
        let (mut inbound, mut outbound) = (Vec::new(), Vec::new());
        for l in &self.links {
            if l.to == id {
                fan_in += 1;
                inbound.push(l.pass);
            }
            if l.from == id {
                fan_out += 1;
                outbound.push(l.pass);
            }
        }
        Coupling {
            fan_in,
            fan_out,
            inbound: Trit::fold_and(inbound),
            outbound: Trit::fold_and(outbound),
        }
    }

    /// 收敛度量：距"全 `+1` 固定点"的距离。目标 = ratio 单向趋近 1.0、
    /// 队列清空、回退必须可解释（见 `docs/GOAL.md`）。
    pub fn convergence(&self) -> Convergence {
        let total = self.nodes.len();
        let hardened = self
            .nodes
            .iter()
            .filter(|n| n.verdict == Trit::Hardened)
            .count();
        let unknown = self.nodes.iter().filter(|n| n.verdict == Trit::Unknown).count();
        let vulnerable = self
            .nodes
            .iter()
            .filter(|n| n.verdict == Trit::Vulnerable)
            .count();
        let quarantined = self.nodes.iter().filter(|n| n.quarantined).count();
        let ratio = if total == 0 {
            1.0
        } else {
            hardened as f64 / total as f64
        };
        Convergence {
            total,
            hardened,
            unknown,
            vulnerable,
            quarantined,
            ratio,
        }
    }

    /// 设计层检查：扇入超过阈值的节点（设计层 `0` 态候选——阈值内不算病，超阈才算）。
    pub fn design_smells(&self, max_fan_in: usize) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|n| self.coupling(n.id).fan_in > max_fan_in)
            .map(|n| n.id)
            .collect()
    }

    /// ASCII 树渲染（含当前裁决、隔离标记与最近一次攻击证据）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.line(&self.nodes[0]));
        out.push('\n');
        self.render_children(NodeId(0), "", &mut out);
        out
    }

    fn render_children(&self, id: NodeId, prefix: &str, out: &mut String) {
        let kids = self.children(id);
        for (i, &c) in kids.iter().enumerate() {
            let last = i + 1 == kids.len();
            let node = &self.nodes[c.0 as usize];
            let _ = writeln!(
                out,
                "{}{}{}",
                prefix,
                if last { "`-- " } else { "|-- " },
                self.line(node)
            );
            let child_prefix = format!("{}{}", prefix, if last { "    " } else { "|   " });
            self.render_children(c, &child_prefix, out);
        }
    }

    fn line(&self, node: &Node) -> String {
        let mut s = format!(
            "#{:02} {:<6} {} [{} {}]",
            node.id.0,
            node.kind.as_str(),
            node.title,
            node.verdict,
            node.verdict.symbol()
        );
        if node.quarantined {
            s.push_str(" [隔离]");
        }
        if let Some(ev) = node.history.iter().rev().find_map(|e| e.evidence.as_ref()) {
            let _ = write!(s, "  payload={:?} → 目标#{:02}", ev.payload, ev.target.0);
        }
        s
    }
}
