//! 三值表达式的 DAG IR 与 pass 框架。
//!
//! 为什么需要它：`trit-eval` 已有成熟的**化简器**（代数感知的规则 + 语义常量检测），
//! 但 `.tri`/TTIR 这条线缺三样东西：
//!
//! 1. **共享子表达式**（CSE）：重复子树只留一份，求值只走一遍；
//! 2. **可插拔的 pass 编排**：每一步都有报告（节点数、改写数），可单独开关；
//! 3. **fail-closed 的等价验证**：每个 pass 的结果必须被**两条独立 oracle**
//!    （`trit-sat` 符号判定 + `trit-bdd`）证明与原式等价，否则拒绝提交。
//!    “查不到（预算不够）”按判红处理——本仓口径：查不到 ≠ 没问题。
//!
//! IR 是**有序 arena**：子节点下标一定小于父节点下标（构造与所有 pass 都保持该不变量），
//! 因此求值只需一趟线性扫描。DAG 通过共享下标表达。

#![forbid(unsafe_code)]

pub mod passes;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::limits::{LimitError, Limits, check_expr};
use trit_core::value::Ternary;
use trit_eval::Assignment;

/// 节点下标（arena 索引）。
pub type NodeId = usize;

/// 一元/二元算子（`Xor` 是强加法，语义见 [`Algebra::strong_add`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// 否定。
    Not,
    /// 合取。
    And,
    /// 析取。
    Or,
    /// 强加法。
    Xor,
    /// 蕴含。
    Implies,
    /// 等价。
    Equiv,
}

/// IR 节点。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    /// 常量。
    Const(Ternary),
    /// 变量。
    Var(String),
    /// 一元节点（目前只有 `Not`）。
    Unary(Op, NodeId),
    /// 二元节点。
    Binary(Op, NodeId, NodeId),
}

impl Node {
    /// 子节点下标（保持递增约束：返回值都小于本节点下标）。
    #[must_use]
    pub fn children(&self) -> Vec<NodeId> {
        match self {
            Self::Const(_) | Self::Var(_) => Vec::new(),
            Self::Unary(_, a) => vec![*a],
            Self::Binary(_, a, b) => vec![*a, *b],
        }
    }
}

/// 有序 arena IR；`root` 一定是最后一个节点的下标（构造与 pass 共同保证）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ir {
    nodes: Vec<Node>,
    root: NodeId,
}

/// IR 错误。
#[derive(Debug, thiserror::Error)]
pub enum IrError {
    /// 输入表达式超出限制。
    #[error("表达式超出限制：{0}")]
    Limit(#[from] LimitError),
    /// 展开/递归深度超上限。
    #[error("IR 深度 {depth} 超过上限 {limit}（拒绝递归展开）")]
    Depth {
        /// 实际深度。
        depth: usize,
        /// 上限。
        limit: usize,
    },
    /// 某个 pass 破坏了等价性。
    #[error("pass `{pass}` 破坏了等价性（{oracle} 判不等价）")]
    NotEquivalent {
        /// pass 名。
        pass: &'static str,
        /// 判不等价的 oracle。
        oracle: &'static str,
    },
    /// oracle 自身失败（预算不够等）；按“查不到 ≠ 没问题”判红。
    #[error("pass `{pass}` 的 oracle 失败：{detail}")]
    Oracle {
        /// pass 名。
        pass: &'static str,
        /// 具体原因。
        detail: String,
    },
    /// 节点结构不合法（内部不变量被破坏）。
    #[error("IR 节点非法：{detail}")]
    BadNode {
        /// 具体原因。
        detail: String,
    },
}

/// `to_expr` 的递归深度上限（输入本身受 [`Limits`] 约束，这里是防御性上限）。
pub const MAX_LOWER_DEPTH: usize = 4096;

impl Ir {
    /// 从表达式构造（子节点先入 arena）。
    pub fn from_expr(expr: &Expr) -> Result<Self, IrError> {
        check_expr(expr, Limits::default())?;
        let mut nodes = Vec::new();
        let root = build(expr, &mut nodes);
        Ok(Self { nodes, root })
    }

    /// 节点数（DAG 去重后的实际数量）。
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 根下标。
    #[must_use]
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// 只读节点表（测试与诊断用）。
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// pass 用：从（已保持「子节点下标小于父节点」不变量的）节点表与根构造。
    pub(crate) fn from_parts(nodes: Vec<Node>, root: NodeId) -> Self {
        debug_assert!(root < nodes.len(), "root 越界：{root} ≥ {}", nodes.len());
        Self { nodes, root }
    }

    /// 出现过的变量（字典序）。
    #[must_use]
    pub fn variables(&self) -> Vec<String> {
        let mut vars: Vec<String> = self
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Var(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        vars.sort();
        vars.dedup();
        vars
    }

    /// 展开回表达式（DAG 会按路径重复展开）。
    pub fn to_expr(&self) -> Result<Expr, IrError> {
        self.lower(self.root, 0)
    }

    fn lower(&self, id: NodeId, depth: usize) -> Result<Expr, IrError> {
        if depth > MAX_LOWER_DEPTH {
            return Err(IrError::Depth {
                depth,
                limit: MAX_LOWER_DEPTH,
            });
        }
        let lower = |child: NodeId| self.lower(child, depth + 1).map(Box::new);
        Ok(match &self.nodes[id] {
            Node::Const(value) => Expr::Const(*value),
            Node::Var(name) => Expr::Var(name.clone()),
            Node::Unary(Op::Not, a) => Expr::Not(lower(*a)?),
            Node::Unary(op, _) => {
                return Err(IrError::BadNode {
                    detail: format!("一元节点只允许 Not，实际 {op:?}"),
                });
            }
            Node::Binary(op, a, b) => {
                let left = lower(*a)?;
                let right = lower(*b)?;
                match op {
                    Op::And => Expr::And(left, right),
                    Op::Or => Expr::Or(left, right),
                    Op::Xor => Expr::Xor(left, right),
                    Op::Implies => Expr::Implies(left, right),
                    Op::Equiv => Expr::Equiv(left, right),
                    Op::Not => {
                        return Err(IrError::BadNode {
                            detail: "Not 不能作为二元算子".into(),
                        });
                    }
                }
            }
        })
    }

    /// 在给定赋值下求值（未出现的变量按 `Unknown`）。
    ///
    /// arena 的递增不变量保证这里只需一趟线性扫描。
    #[must_use]
    pub fn eval(&self, assignment: &Assignment, alg: Algebra) -> Ternary {
        let mut values: Vec<Ternary> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let value = match node {
                Node::Const(value) => *value,
                Node::Var(name) => assignment.get(name).unwrap_or(Ternary::Unknown),
                Node::Unary(Op::Not, a) => alg.not(values[*a]),
                Node::Unary(op, _) => {
                    debug_assert!(false, "非法一元算子 {op:?} 不应进入求值");
                    Ternary::Unknown
                }
                Node::Binary(op, a, b) => {
                    let left = values[*a];
                    let right = values[*b];
                    match op {
                        Op::And => alg.and(left, right),
                        Op::Or => alg.or(left, right),
                        Op::Xor => alg.strong_add(left, right),
                        Op::Implies => alg.implies(left, right),
                        Op::Equiv => alg.equiv(left, right),
                        Op::Not => {
                            debug_assert!(false, "Not 不能作为二元算子");
                            Ternary::Unknown
                        }
                    }
                }
            };
            values.push(value);
        }
        values[self.root]
    }
}

fn build(expr: &Expr, nodes: &mut Vec<Node>) -> NodeId {
    let node = match expr {
        Expr::Const(value) => Node::Const(*value),
        Expr::Var(name) => Node::Var(name.clone()),
        Expr::Not(a) => {
            let a = build(a, nodes);
            Node::Unary(Op::Not, a)
        }
        Expr::And(a, b) => {
            let left = build(a, nodes);
            let right = build(b, nodes);
            Node::Binary(Op::And, left, right)
        }
        Expr::Or(a, b) => {
            let left = build(a, nodes);
            let right = build(b, nodes);
            Node::Binary(Op::Or, left, right)
        }
        Expr::Xor(a, b) => {
            let left = build(a, nodes);
            let right = build(b, nodes);
            Node::Binary(Op::Xor, left, right)
        }
        Expr::Implies(a, b) => {
            let left = build(a, nodes);
            let right = build(b, nodes);
            Node::Binary(Op::Implies, left, right)
        }
        Expr::Equiv(a, b) => {
            let left = build(a, nodes);
            let right = build(b, nodes);
            Node::Binary(Op::Equiv, left, right)
        }
    };
    nodes.push(node);
    nodes.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_core::parser::Parser;

    fn parse(text: &str) -> Expr {
        Parser::parse(text).unwrap()
    }

    fn xor_expr() -> Expr {
        Expr::Xor(
            Box::new(Expr::Var("a".into())),
            Box::new(Expr::Var("b".into())),
        )
    }

    fn assign(pairs: &[(&str, Ternary)]) -> Assignment {
        let mut assignment = Assignment::new();
        for (name, value) in pairs {
            assignment.set(*name, *value);
        }
        assignment
    }

    fn eval_all(ir: &Ir, alg: Algebra) -> Vec<Ternary> {
        let vars = ir.variables();
        let total = 3usize.pow(vars.len() as u32);
        let mut out = Vec::new();
        for index in 0..total {
            let mut rest = index;
            let mut assignment = Assignment::new();
            for name in &vars {
                let value = match rest % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                };
                rest /= 3;
                assignment.set(name.as_str(), value);
            }
            out.push(ir.eval(&assignment, alg));
        }
        out
    }

    #[test]
    fn round_trip_preserves_semantics() {
        let parsed = parse("(a & X) | (!b)");
        let ir = Ir::from_expr(&parsed).unwrap();
        let back = ir.to_expr().unwrap();
        for alg in Algebra::ALL {
            let asg = Assignment::new();
            assert_eq!(
                trit_eval::eval(&parsed, &asg, alg),
                trit_eval::eval(&back, &asg, alg)
            );
        }
    }

    #[test]
    fn shared_nodes_evaluate_once_but_lower_to_tree() {
        let parsed = parse("(a & b) | (a & b)");
        let ir = Ir::from_expr(&parsed).unwrap();
        assert_eq!(ir.node_count(), 7, "树形构造：a,b,& 出现两遍，外加根");
        let lowered = ir.to_expr().unwrap();
        assert_eq!(
            trit_eval::eval(&lowered, &Assignment::new(), Algebra::Kleene),
            Ternary::Unknown
        );
    }

    #[test]
    fn xor_uses_strong_add() {
        for alg in Algebra::ALL {
            let ir = Ir::from_expr(&xor_expr()).unwrap();
            for a in [Ternary::False, Ternary::Unknown, Ternary::True] {
                for b in [Ternary::False, Ternary::Unknown, Ternary::True] {
                    let assignment = assign(&[("a", a), ("b", b)]);
                    assert_eq!(
                        ir.eval(&assignment, alg),
                        alg.strong_add(a, b),
                        "xor a={a:?} b={b:?} alg={alg}"
                    );
                }
            }
        }
    }

    #[test]
    fn eval_applies_negation() {
        let ir = Ir::from_expr(&parse("!a")).unwrap();
        for alg in Algebra::ALL {
            for a in [Ternary::False, Ternary::Unknown, Ternary::True] {
                let assignment = assign(&[("a", a)]);
                assert_eq!(ir.eval(&assignment, alg), alg.not(a), "alg={alg} a={a:?}");
            }
        }
    }

    #[test]
    fn eval_counts_match_direct_evaluation() {
        let parsed = parse("a & (b | c)");
        let ir = Ir::from_expr(&parsed).unwrap();
        let vars = ir.variables();
        assert_eq!(vars, vec!["a", "b", "c"]);
        for alg in Algebra::ALL {
            let ir_values = eval_all(&ir, alg);
            let mut expected = Vec::new();
            for index in 0..27 {
                let mut rest = index;
                let mut assignment = Assignment::new();
                for name in &vars {
                    let value = match rest % 3 {
                        0 => Ternary::False,
                        1 => Ternary::Unknown,
                        _ => Ternary::True,
                    };
                    rest /= 3;
                    assignment.set(name.as_str(), value);
                }
                expected.push(trit_eval::eval(&parsed, &assignment, alg));
            }
            assert_eq!(ir_values, expected, "alg={alg}");
        }
    }
}
