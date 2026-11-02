//! 三值攻击图（M2）：节点与边都带证据等级，`X` 明确表示「待验证」。
//!
//! 自动渗透文献的共同缺陷（见 `docs/SECURITY-NET.md` §与论文的关系）：攻击图的
//! 边没有证据等级，agent 把「没验证」当「成功」，跑出来的路径一半不可信。
//! 本模块把证据等级做成**类型系统事实**，并用两条机械纪律堵死升格：
//!
//! 1. **路径折叠 = K3 AND**（走 [`Algebra::Kleene`] 内核）：路径上每个节点与每条
//!    边都参与合取——任何一个 `X` 让整条路径变 `X`，任何 `F` 让整条路径变 `F`。
//!    **没有一条「已证实」结论能从未验证的环节得出。**
//! 2. **跨路径合并 = K3 OR**：一条已证实路线 ⇒ 目标可达已证实；所有路线都已
//!    排除 ⇒ 已排除；否则待验证。**枚举不全（截断）⇒ 一律封顶为待验证**——
//!    既不因没找到已证实路径就说「已排除」，也不因找到一条就忽略没看到的。
//!
//! 与 [`super::decision`] 同构：`verdict + reason`，预算不足不降级成猜测。
//!
//! 边界（不猜）：图只登记已声明的事实，「无路径」的 `RuledOut` 是**图内已排除**
//! （以登记的节点/边为准），不是绝对不可能——图不完整时它只说明登记面没给出路。
//! 本模块不发包、不探测；证据从哪来（M3 的受控探测、人工复核）是登记时的
//! `source` 字段的事。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize, de::Deserializer, ser::Serializer};
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

/// 证据等级：`F` 已排除 < `X` 待验证 < `T` 已证实（声明序即序，`Ord` 可用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Evidence {
    /// 已排除：有证据说明这一步/这个立场走不通。
    RuledOut,
    /// 待验证：有人声称可行但尚无证实证据。
    Unverified,
    /// 已证实：有可追溯证据。
    Confirmed,
}

/// JSON 里的证据等级一律是单字符 `T`/`X`/`F`（与台账字形一致）。
impl Serialize for Evidence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_char(self.as_char())
    }
}

impl<'de> Deserialize<'de> for Evidence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let c = char::deserialize(deserializer)?;
        Evidence::parse_char(c)
            .ok_or_else(|| serde::de::Error::custom(format!("证据字符 {c:?} 不认识")))
    }
}

impl Evidence {
    /// 单字符视图（与 [`super::features::encode`] 同一套字形）。
    pub const fn as_char(self) -> char {
        match self {
            Self::RuledOut => 'F',
            Self::Unverified => 'X',
            Self::Confirmed => 'T',
        }
    }

    /// 解析单字符（`T`/`X`/`F`，大小写不敏感）。
    pub fn parse_char(c: char) -> Option<Self> {
        match c {
            't' | 'T' => Some(Self::Confirmed),
            'x' | 'X' => Some(Self::Unverified),
            'f' | 'F' => Some(Self::RuledOut),
            _ => None,
        }
    }

    /// 映射到三值内核：`T → True`、`X → Unknown`、`F → False`。
    pub const fn to_ternary(self) -> Ternary {
        match self {
            Self::RuledOut => Ternary::False,
            Self::Unverified => Ternary::Unknown,
            Self::Confirmed => Ternary::True,
        }
    }

    /// 由三值内核还原（`True/Unknown/False` 之外回退 `X`，保守）。
    pub const fn from_ternary(value: Ternary) -> Self {
        match value {
            Ternary::True => Self::Confirmed,
            Ternary::False => Self::RuledOut,
            Ternary::Unknown => Self::Unverified,
        }
    }

    /// 路径折叠：K3 AND（格序 `min`，`F` 支配）——直接走内核的 [`Algebra::Kleene`]。
    ///
    /// 注意不是 [`Algebra::strong_and`]（那是 ⊗ 张量积，`X` 沾边即污染）：
    /// 一条已排除的跳数必须毙掉整条路，哪怕别的环节还没验证。
    /// 折叠表与「`F < X < T` 上的 `min`」严格一致，由穷举测试钉死。
    pub fn fold(self, other: Self) -> Self {
        Self::from_ternary(Algebra::Kleene.and(self.to_ternary(), other.to_ternary()))
    }

    /// 跨路径合并：K3 OR（格序 `max`，`T` 支配）——直接走内核。
    pub fn merge(self, other: Self) -> Self {
        Self::from_ternary(Algebra::Kleene.or(self.to_ternary(), other.to_ternary()))
    }
}

/// 攻击图错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttackError {
    /// 引用了不存在的节点。
    #[error("节点 {0} 不存在")]
    UnknownNode(u32),
    /// 证据出处为空：没有出处的证据不许登记。
    #[error("证据出处为空：登记（节点或边）必须带非空 source")]
    EmptySource,
    /// 台账解析失败。
    #[error("攻击图台账解析失败：{0}")]
    Json(String),
}

/// 图节点：一个攻击立场（立足点 / 中间步骤 / 目标）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// 人类可读名，图内唯一；重复登记复用同一节点（幂等）。
    pub label: String,
    /// 这个立场本身的证据等级——「已站稳」与「只是声称」是两回事。
    pub evidence: Evidence,
    /// 备注（可空）。
    #[serde(default)]
    pub note: String,
}

/// 有向边：一条攻击步骤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// 这一步的证据等级。
    pub evidence: Evidence,
    /// 证据出处（登记时必填）。
    pub source: String,
}

/// 三值攻击图。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttackGraph {
    nodes: BTreeMap<u32, Node>,
    edges: BTreeMap<(u32, u32), Edge>,
    next_id: u32,
}

/// 路径枚举预算；超出如实报告，不静默截断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 单条路径最多经过的节点数。
    pub max_depth: usize,
    /// 最多枚举的路径条数。
    pub max_paths: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 16,
            max_paths: 256,
        }
    }
}

/// 路径上待验证的环节：节点（`edge_to == None`）或一条边。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenStep {
    /// 该环节所在的节点。
    pub node: u32,
    /// 若是边，边的终点（起点即路径上的前一个节点）。
    pub edge_to: Option<u32>,
}

/// 一条路径的明细。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathDetail {
    /// 节点序列（含起点与终点）。
    pub nodes: Vec<u32>,
    /// 路径折叠值：K3 AND over 节点与边。
    pub fold: Evidence,
    /// 折叠值为 `X` 时的见证：哪些环节待验证。
    pub open: Vec<OpenStep>,
}

/// 「从 src 能否走到 dst」的判决——与 [`super::decision::Decision`] 同构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    /// 总判决。
    pub verdict: Evidence,
    /// 机器可读原因（稳定字符串；`String` 是为了随 [`Reach`] 走 JSON）。
    pub reason: String,
    /// 是否枚举不全（截断时 `verdict` 已封顶为 `X`）。
    pub truncated: bool,
    /// 已枚举路径的明细（发现序）。
    pub paths: Vec<PathDetail>,
}

impl AttackGraph {
    /// 空图。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记节点；同 `label` 复用已有节点（幂等），返回节点 id。
    pub fn add_node(&mut self, label: &str, evidence: Evidence, note: &str) -> u32 {
        if let Some((id, _)) = self.nodes.iter().find(|(_, node)| node.label == label) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(
            id,
            Node {
                label: label.to_string(),
                evidence,
                note: note.to_string(),
            },
        );
        id
    }

    /// 按 label 查节点 id。
    pub fn node_id(&self, label: &str) -> Option<u32> {
        self.nodes
            .iter()
            .find(|(_, node)| node.label == label)
            .map(|(id, _)| *id)
    }

    /// 登记边；同一对 (from, to) 只有一条边，重复登记**后写覆盖**（LWW），
    /// 返回被覆盖的旧证据（首次登记返回 `None`）。
    ///
    /// `source` 为空或端点不存在都是 [`AttackError`]——出处与端点都不猜。
    pub fn add_edge(
        &mut self,
        from: u32,
        to: u32,
        evidence: Evidence,
        source: &str,
    ) -> Result<Option<Evidence>, AttackError> {
        if source.is_empty() {
            return Err(AttackError::EmptySource);
        }
        if !self.nodes.contains_key(&from) {
            return Err(AttackError::UnknownNode(from));
        }
        if !self.nodes.contains_key(&to) {
            return Err(AttackError::UnknownNode(to));
        }
        Ok(self
            .edges
            .insert(
                (from, to),
                Edge {
                    evidence,
                    source: source.to_string(),
                },
            )
            .map(|old| old.evidence))
    }

    /// 查节点。
    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// 查边。
    pub fn edge(&self, from: u32, to: u32) -> Option<&Edge> {
        self.edges.get(&(from, to))
    }

    /// 节点数（诊断用）。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否为空图。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 判决「从 src 能否走到 dst」。
    ///
    /// - `src == dst`：退化为起点立场自身的证据，路径只有起点一个节点。
    /// - 枚举不全（触达 `max_depth` 或 `max_paths`）⇒ `verdict` **一律封顶 `X`**
    ///   （`reason = "truncated"`）：枚举不全不给「已证实」，也不给「已排除」。
    pub fn reach(&self, src: u32, dst: u32, limits: Limits) -> Result<Reach, AttackError> {
        let src_node = self.nodes.get(&src).ok_or(AttackError::UnknownNode(src))?;
        if !self.nodes.contains_key(&dst) {
            return Err(AttackError::UnknownNode(dst));
        }
        if src == dst {
            let open = open_node(src, &src_node.evidence);
            return Ok(Reach {
                verdict: src_node.evidence,
                reason: "self".to_string(),
                truncated: false,
                paths: vec![PathDetail {
                    nodes: vec![src],
                    fold: src_node.evidence,
                    open,
                }],
            });
        }
        let mut paths = Vec::new();
        let mut truncated = false;
        let mut visited = BTreeSet::new();
        visited.insert(src);
        let src_open = open_node(src, &src_node.evidence);
        self.dfs(
            src,
            dst,
            src_node.evidence,
            vec![src],
            src_open,
            &mut visited,
            limits,
            &mut paths,
            &mut truncated,
        );
        // 跨路径合并：K3 OR over 折叠值。
        let mut verdict = paths
            .first()
            .map(|path| path.fold)
            .unwrap_or(Evidence::RuledOut);
        for path in paths.iter().skip(1) {
            verdict = verdict.merge(path.fold);
        }
        let mut reason = match verdict {
            Evidence::Confirmed => "proven-path",
            Evidence::Unverified => "unverified-only",
            Evidence::RuledOut => {
                if paths.is_empty() {
                    "no-path"
                } else {
                    "all-ruled-out"
                }
            }
        }
        .to_string();
        if truncated {
            // 截断封顶：枚举不全，确认与排除都不许下。
            verdict = Evidence::Unverified;
            reason = "truncated".to_string();
        }
        Ok(Reach {
            verdict,
            reason,
            truncated,
            paths,
        })
    }

    /// 简单路径 DFS（on-path 去重 ⇒ 环路必然终止）。
    ///
    /// `open` 沿途累积：每个环节（边、节点）若为 `X` 就记入见证。
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        &self,
        current: u32,
        dst: u32,
        fold: Evidence,
        trail: Vec<u32>,
        open: Vec<OpenStep>,
        visited: &mut BTreeSet<u32>,
        limits: Limits,
        paths: &mut Vec<PathDetail>,
        truncated: &mut bool,
    ) {
        if paths.len() >= limits.max_paths {
            *truncated = true;
            return;
        }
        if trail.len() >= limits.max_depth {
            *truncated = true;
            return;
        }
        for ((from, to), edge) in &self.edges {
            if *from != current || visited.contains(to) {
                continue;
            }
            let next_fold = fold.fold(edge.evidence).fold(self.nodes[to].evidence);
            let mut next_open = open.clone();
            next_open.extend(open_edge(*from, Some(*to), &edge.evidence));
            next_open.extend(open_node(*to, &self.nodes[to].evidence));
            let mut next_trail = trail.clone();
            next_trail.push(*to);
            if *to == dst {
                if paths.len() >= limits.max_paths {
                    *truncated = true;
                    return;
                }
                paths.push(PathDetail {
                    nodes: next_trail,
                    fold: next_fold,
                    open: next_open,
                });
            } else {
                visited.insert(*to);
                self.dfs(
                    *to, dst, next_fold, next_trail, next_open, visited, limits, paths, truncated,
                );
                visited.remove(to);
            }
        }
    }

    /// 导出 JSONL 台账：每行一个节点或边，字段顺序固定。
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for (id, node) in &self.nodes {
            let line = Row::Node {
                id: *id,
                label: node.label.clone(),
                evidence: node.evidence.as_char(),
                note: node.note.clone(),
            };
            out.push_str(&serde_json::to_string(&line).expect("节点行序列化不会失败"));
            out.push('\n');
        }
        for ((from, to), edge) in &self.edges {
            let line = Row::Edge {
                from: *from,
                to: *to,
                evidence: edge.evidence.as_char(),
                source: edge.source.clone(),
            };
            out.push_str(&serde_json::to_string(&line).expect("边行序列化不会失败"));
            out.push('\n');
        }
        out
    }

    /// 从 JSONL 台账复原（往返由测试钉死；坏行显式报错）。
    pub fn from_jsonl(text: &str) -> Result<Self, AttackError> {
        let mut graph = Self::new();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let row: Row = serde_json::from_str(line)
                .map_err(|error| AttackError::Json(format!("第 {} 行：{error}", number + 1)))?;
            match row {
                Row::Node {
                    id,
                    label,
                    evidence,
                    note,
                } => {
                    let evidence = Evidence::parse_char(evidence).ok_or_else(|| {
                        AttackError::Json(format!(
                            "第 {} 行：证据字符 {evidence:?} 不认识",
                            number + 1
                        ))
                    })?;
                    graph.nodes.insert(
                        id,
                        Node {
                            label,
                            evidence,
                            note,
                        },
                    );
                    graph.next_id = graph.next_id.max(id + 1);
                }
                Row::Edge {
                    from,
                    to,
                    evidence,
                    source,
                } => {
                    let evidence = Evidence::parse_char(evidence).ok_or_else(|| {
                        AttackError::Json(format!(
                            "第 {} 行：证据字符 {evidence:?} 不认识",
                            number + 1
                        ))
                    })?;
                    // 端点与出处不猜：坏了就报错，不静默丢行。
                    if !graph.nodes.contains_key(&from) {
                        return Err(AttackError::Json(format!(
                            "第 {} 行：边端点 {from} 没有先登记",
                            number + 1
                        )));
                    }
                    if !graph.nodes.contains_key(&to) {
                        return Err(AttackError::Json(format!(
                            "第 {} 行：边端点 {to} 没有先登记",
                            number + 1
                        )));
                    }
                    graph.add_edge(from, to, evidence, &source)?;
                }
            }
        }
        Ok(graph)
    }
}

/// 待验证见证的单个环节：节点本身。
fn open_node(node: u32, evidence: &Evidence) -> Vec<OpenStep> {
    if *evidence == Evidence::Unverified {
        vec![OpenStep {
            node,
            edge_to: None,
        }]
    } else {
        Vec::new()
    }
}

/// 待验证见证的单个环节：一条边（`to == None` 时不产生）。
fn open_edge(from: u32, to: Option<u32>, evidence: &Evidence) -> Vec<OpenStep> {
    if *evidence == Evidence::Unverified {
        vec![OpenStep {
            node: from,
            edge_to: to,
        }]
    } else {
        Vec::new()
    }
}

/// 台账行：节点或边（owned——`Deserialize` 需要自持字符串）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Row {
    /// 节点行。
    Node {
        /// 节点 id。
        id: u32,
        /// label。
        label: String,
        /// 证据字符（`T`/`X`/`F`）。
        evidence: char,
        /// 备注。
        note: String,
    },
    /// 边行。
    Edge {
        /// 起点。
        from: u32,
        /// 终点。
        to: u32,
        /// 证据字符（`T`/`X`/`F`）。
        evidence: char,
        /// 出处。
        source: String,
    },
}
