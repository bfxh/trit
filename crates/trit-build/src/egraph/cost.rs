//! e-graph 提取代价与重写选项（从 `egraph.rs` 拆出，避免单文件超过 800 行）。

use trit_core::Expr;

use super::Op;

/// 提取代价模型：`nodes` 按 AST 节点数，`gates` 按门数（变量/常量为 0，蕴含/等价更贵）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CostModel {
    Nodes,
    Gates,
}

impl CostModel {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "nodes" | "node" => Some(Self::Nodes),
            "gates" | "gate" => Some(Self::Gates),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Nodes => "nodes",
            Self::Gates => "gates",
        }
    }

    pub(super) const fn leaf(self) -> u64 {
        match self {
            Self::Nodes => 1,
            Self::Gates => 0,
        }
    }

    pub(super) const fn op(self, op: Op) -> u64 {
        match self {
            Self::Nodes => 1,
            Self::Gates => match op {
                Op::And | Op::Or => 1,
                Op::Xor | Op::Implies => 2,
                Op::Equiv => 3,
            },
        }
    }

    pub(super) const fn not(self) -> u64 {
        match self {
            Self::Nodes => 1,
            Self::Gates => 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RewriteOptions {
    /// 最大轮数；通常远早于上限就到达不动点。0 表示不进图。
    pub max_rounds: usize,
    /// e-node 上限（防止病态表达式把内存吃光）。
    pub node_limit: usize,
    pub cost: CostModel,
}

impl Default for RewriteOptions {
    fn default() -> Self {
        Self {
            max_rounds: 64,
            node_limit: 4096,
            cost: CostModel::Nodes,
        }
    }
}

#[derive(Debug)]
pub(crate) struct RewriteReport {
    pub expr: Expr,
    pub rules: Vec<&'static str>,
    pub rounds: usize,
    pub e_nodes: usize,
    pub saturated: bool,
    pub cost: u64,
    pub nodes: u64,
}

pub(super) fn expr_stats(expr: &Expr, model: CostModel) -> (u64, u64) {
    match expr {
        Expr::Const(_) | Expr::Var(_) => (model.leaf(), 1),
        Expr::Not(inner) => {
            let (cost, nodes) = expr_stats(inner, model);
            (cost.saturating_add(model.not()), nodes + 1)
        }
        Expr::And(left, right)
        | Expr::Or(left, right)
        | Expr::Xor(left, right)
        | Expr::Implies(left, right)
        | Expr::Equiv(left, right) => {
            let op = match expr {
                Expr::And(..) => Op::And,
                Expr::Or(..) => Op::Or,
                Expr::Xor(..) => Op::Xor,
                Expr::Implies(..) => Op::Implies,
                _ => Op::Equiv,
            };
            let (left_cost, left_nodes) = expr_stats(left, model);
            let (right_cost, right_nodes) = expr_stats(right, model);
            (
                left_cost
                    .saturating_add(right_cost)
                    .saturating_add(model.op(op)),
                left_nodes + right_nodes + 1,
            )
        }
    }
}
