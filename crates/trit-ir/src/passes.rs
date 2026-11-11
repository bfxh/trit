//! pass 框架：常量折叠 / 复用既有化简器 / CSE（共享子表达式）。
//!
//! 每个 pass 都是「输入 IR → 可选输出 IR」的纯函数；`None` 表示没有变化。
//!
//! 顺序口径：`Simplify` 复用 `trit-eval` 的表达式化简器，会先把 DAG 展开成树再重建，
//! 因此它**必须排在 `Cse` 之前**；对同一条管道重跑时，最终 IR 不变，但中间节点数可能
//! 先展开再合回（报告里的改写数不为 0 是正常的）。
//! [`Pipeline::run_verified`] 在每一步之后用**两条独立 oracle**（`trit-sat` 符号判定 +
//! `trit-bdd`）证明「新 IR ≡ 旧 IR」，任何一条判不等价、或 oracle 自己算不出来，都直接判红。

use std::collections::{HashMap, HashSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

use crate::{Ir, IrError, Node, NodeId, Op};

/// pass 的运行上下文。
#[derive(Clone, Copy, Debug)]
pub struct PassCtx {
    /// 当前语义（所有重写都必须在它下面保持等价）。
    pub alg: Algebra,
}

/// 一个 pass。
pub trait Pass {
    /// 名字（报告与错误里用）。
    fn name(&self) -> &'static str;
    /// 运行；`None` = 没有变化。
    fn apply(&self, ir: &Ir, ctx: &PassCtx) -> Result<Option<Ir>, IrError>;
}

/// 常量折叠：自底向上把「子节点全是常量」的节点折成常量。
#[derive(Debug, Default, Clone, Copy)]
pub struct ConstantFold;

impl Pass for ConstantFold {
    fn name(&self) -> &'static str {
        "constant-fold"
    }

    fn apply(&self, ir: &Ir, ctx: &PassCtx) -> Result<Option<Ir>, IrError> {
        let mut nodes: Vec<Node> = Vec::with_capacity(ir.nodes().len());
        let mut changed = false;
        for node in ir.nodes() {
            let folded = match node {
                Node::Const(_) | Node::Var(_) => node.clone(),
                Node::Unary(op, a) => match (op, &nodes[*a]) {
                    (Op::Not, Node::Const(value)) => {
                        changed = true;
                        Node::Const(ctx.alg.not(*value))
                    }
                    _ => node.clone(),
                },
                Node::Binary(op, a, b) => match (&nodes[*a], &nodes[*b]) {
                    (Node::Const(left), Node::Const(right)) => {
                        changed = true;
                        Node::Const(eval_op(ctx.alg, *op, *left, *right)?)
                    }
                    _ => node.clone(),
                },
            };
            nodes.push(folded);
        }
        if !changed {
            return Ok(None);
        }
        Ok(Some(prune_unreachable(nodes, ir.root())))
    }
}

/// 只保留根可达的节点（保持「子节点下标小于父节点」的顺序）。
fn prune_unreachable(nodes: Vec<Node>, root: NodeId) -> Ir {
    let mut keep = vec![false; nodes.len()];
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if keep[id] {
            continue;
        }
        keep[id] = true;
        for child in nodes[id].children() {
            stack.push(child);
        }
    }
    let mut remap = vec![0usize; nodes.len()];
    let mut pruned: Vec<Node> = Vec::with_capacity(nodes.len());
    for (id, node) in nodes.into_iter().enumerate() {
        if !keep[id] {
            continue;
        }
        let mapped = match node {
            Node::Const(value) => Node::Const(value),
            Node::Var(name) => Node::Var(name),
            Node::Unary(op, a) => Node::Unary(op, remap[a]),
            Node::Binary(op, a, b) => Node::Binary(op, remap[a], remap[b]),
        };
        remap[id] = pruned.len();
        pruned.push(mapped);
    }
    Ir::from_parts(pruned, remap[root])
}

/// 复用 `trit-eval` 的代数感知化简器（含语义常量检测），再重建 IR。
#[derive(Debug, Default, Clone, Copy)]
pub struct Simplify;

impl Pass for Simplify {
    fn name(&self) -> &'static str {
        "simplify"
    }

    fn apply(&self, ir: &Ir, ctx: &PassCtx) -> Result<Option<Ir>, IrError> {
        let expr = ir.to_expr()?;
        let simplified =
            trit_eval::simplify_with(&expr, ctx.alg, trit_eval::SimplifyOptions::default());
        let rebuilt = Ir::from_expr(&simplified)?;
        if rebuilt == *ir {
            return Ok(None);
        }
        Ok(Some(rebuilt))
    }
}

/// 公共子表达式消除：按结构哈希合并重复节点（DAG 共享）。
#[derive(Debug, Default, Clone, Copy)]
pub struct Cse;

impl Pass for Cse {
    fn name(&self) -> &'static str {
        "cse"
    }

    fn apply(&self, ir: &Ir, _ctx: &PassCtx) -> Result<Option<Ir>, IrError> {
        let mut nodes: Vec<Node> = Vec::with_capacity(ir.nodes().len());
        let mut interned: HashMap<Node, NodeId> = HashMap::new();
        let mut remap: Vec<NodeId> = Vec::with_capacity(ir.nodes().len());
        for node in ir.nodes() {
            let mapped = match node {
                Node::Const(value) => Node::Const(*value),
                Node::Var(name) => Node::Var(name.clone()),
                Node::Unary(op, a) => Node::Unary(*op, remap[*a]),
                Node::Binary(op, a, b) => Node::Binary(*op, remap[*a], remap[*b]),
            };
            let id = match interned.get(&mapped) {
                Some(existing) => *existing,
                None => {
                    let id = nodes.len();
                    interned.insert(mapped.clone(), id);
                    nodes.push(mapped);
                    id
                }
            };
            remap.push(id);
        }
        if nodes.len() == ir.node_count() {
            return Ok(None);
        }
        let root = remap[ir.root()];
        Ok(Some(Ir::from_parts(nodes, root)))
    }
}

fn eval_op(alg: Algebra, op: Op, left: Ternary, right: Ternary) -> Result<Ternary, IrError> {
    Ok(match op {
        Op::And => alg.and(left, right),
        Op::Or => alg.or(left, right),
        Op::Xor => alg.strong_add(left, right),
        Op::Implies => alg.implies(left, right),
        Op::Equiv => alg.equiv(left, right),
        Op::Not => {
            return Err(IrError::BadNode {
                detail: "Not 不能作为二元算子".into(),
            });
        }
    })
}

/// 单个 pass 的报告。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassReport {
    /// pass 名。
    pub name: &'static str,
    /// 运行前节点数。
    pub nodes_before: usize,
    /// 运行后节点数。
    pub nodes_after: usize,
}

impl PassReport {
    /// 节点减少量（改写量）。
    #[must_use]
    pub fn rewrites(&self) -> usize {
        self.nodes_before.saturating_sub(self.nodes_after)
    }
}

/// pass 管道。
pub struct Pipeline {
    alg: Algebra,
    passes: Vec<Box<dyn Pass>>,
}

impl Pipeline {
    /// 空管道。
    #[must_use]
    pub fn new(alg: Algebra) -> Self {
        Self {
            alg,
            passes: Vec::new(),
        }
    }

    /// 默认管道：常量折叠 → 化简 → CSE。
    #[must_use]
    pub fn default_pipeline(alg: Algebra) -> Self {
        Self::new(alg).with(ConstantFold).with(Simplify).with(Cse)
    }

    /// 追加一个 pass。
    #[must_use]
    pub fn with<P: Pass + 'static>(mut self, pass: P) -> Self {
        self.passes.push(Box::new(pass));
        self
    }

    /// 跑管道（不验证）。
    pub fn run(&self, input: &Ir) -> Result<(Ir, Vec<PassReport>), IrError> {
        self.run_inner(input, false)
    }

    /// 跑管道，并在每一步之后用双 oracle 验证等价（fail-closed）。
    pub fn run_verified(&self, input: &Ir) -> Result<(Ir, Vec<PassReport>), IrError> {
        self.run_inner(input, true)
    }

    fn run_inner(&self, input: &Ir, verified: bool) -> Result<(Ir, Vec<PassReport>), IrError> {
        let ctx = PassCtx { alg: self.alg };
        let mut current = input.clone();
        let mut reports = Vec::with_capacity(self.passes.len());
        for pass in &self.passes {
            let before = current.node_count();
            let next = pass.apply(&current, &ctx)?;
            let Some(next) = next else {
                reports.push(PassReport {
                    name: pass.name(),
                    nodes_before: before,
                    nodes_after: before,
                });
                continue;
            };
            if verified {
                verify(&current, &next, self.alg, pass.name())?;
            }
            reports.push(PassReport {
                name: pass.name(),
                nodes_before: before,
                nodes_after: next.node_count(),
            });
            current = next;
        }
        Ok((current, reports))
    }
}

/// 双 oracle 等价验证：`trit-sat` 符号判定 + `trit-bdd`。
fn verify(before: &Ir, after: &Ir, alg: Algebra, pass: &'static str) -> Result<(), IrError> {
    let left = before.to_expr()?;
    let right = after.to_expr()?;
    let symbolic =
        trit_sat::equivalent_symbolic(&left, &right, alg).map_err(|err| IrError::Oracle {
            pass,
            detail: format!("trit-sat: {err}"),
        })?;
    if !symbolic.equivalent {
        return Err(IrError::NotEquivalent {
            pass,
            oracle: "trit-sat",
        });
    }
    let bdd = trit_bdd::equivalent_bdd(&left, &right, alg, &HashSet::new()).map_err(|err| {
        IrError::Oracle {
            pass,
            detail: format!("trit-bdd: {err}"),
        }
    })?;
    if !bdd.result.equivalent {
        return Err(IrError::NotEquivalent {
            pass,
            oracle: "trit-bdd",
        });
    }
    Ok(())
}

/// 测试与诊断用：把表达式按变量全集评估一遍（`3^vars`）。
#[doc(hidden)]
#[must_use]
pub fn eval_matrix(expr: &Expr, alg: Algebra) -> Vec<Ternary> {
    let mut vars = expr.variables();
    vars.sort();
    vars.dedup();
    let total = 3usize.pow(vars.len() as u32);
    let mut out = Vec::with_capacity(total);
    for index in 0..total {
        let mut rest = index;
        let mut assignment = trit_eval::Assignment::new();
        for name in &vars {
            let value = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
            assignment.set(name.as_str(), value);
        }
        out.push(trit_eval::eval(expr, &assignment, alg));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_core::parser::Parser;

    fn parse(text: &str) -> Expr {
        Parser::parse(text).unwrap()
    }

    #[test]
    fn constant_fold_collapses_constant_subtrees() {
        for alg in Algebra::ALL {
            let ir = Ir::from_expr(&parse("(1 & X) | 0")).unwrap();
            let (folded, reports) = Pipeline::new(alg)
                .with(ConstantFold)
                .run_verified(&ir)
                .unwrap();
            assert_eq!(folded.node_count(), 1, "alg={alg} reports={reports:?}");
            let value = folded.eval(&trit_eval::Assignment::new(), alg);
            let expected =
                trit_eval::eval(&parse("(1 & X) | 0"), &trit_eval::Assignment::new(), alg);
            assert_eq!(value, expected, "alg={alg}");
        }
    }

    #[test]
    fn constant_fold_folds_not() {
        for alg in Algebra::ALL {
            let ir = Ir::from_expr(&parse("!(1)")).unwrap();
            let (folded, _) = Pipeline::new(alg)
                .with(ConstantFold)
                .run_verified(&ir)
                .unwrap();
            assert_eq!(folded.node_count(), 1, "alg={alg}");
            assert_eq!(
                folded.eval(&trit_eval::Assignment::new(), alg),
                alg.not(Ternary::True),
                "alg={alg}"
            );
        }
    }

    #[test]
    fn cse_shares_repeated_subexpressions() {
        let ir = Ir::from_expr(&parse("(a & b) | (a & b)")).unwrap();
        assert_eq!(ir.node_count(), 7, "树形：a,b,& 各出现两遍 + 根");
        let (shared, reports) = Pipeline::new(Algebra::Kleene).with(Cse).run(&ir).unwrap();
        assert_eq!(
            shared.node_count(),
            4,
            "a,b,&,| 各一份：reports={reports:?}"
        );
        assert_eq!(reports[0].rewrites(), 3);
        // 共享后语义不变。
        assert_eq!(
            eval_matrix(&ir.to_expr().unwrap(), Algebra::Kleene),
            eval_matrix(&shared.to_expr().unwrap(), Algebra::Kleene)
        );
    }

    #[test]
    fn simplify_reduces_and_is_verified() {
        let ir = Ir::from_expr(&parse("!(!a) & (b | 0) & (a | a)")).unwrap();
        for alg in Algebra::ALL {
            let (out, reports) = Pipeline::new(alg).with(Simplify).run_verified(&ir).unwrap();
            assert!(out.node_count() < ir.node_count(), "alg={alg} {reports:?}");
            assert_eq!(
                eval_matrix(&ir.to_expr().unwrap(), alg),
                eval_matrix(&out.to_expr().unwrap(), alg),
                "alg={alg}"
            );
        }
    }

    #[test]
    fn default_pipeline_reaches_a_fixpoint() {
        let ir = Ir::from_expr(&parse("!(!a) & (b | 0) & (a | a)")).unwrap();
        for alg in Algebra::ALL {
            let pipeline = Pipeline::default_pipeline(alg);
            let (first, _) = pipeline.run_verified(&ir).unwrap();
            // Simplify 会把 DAG 展开成树、CSE 再合回，所以第二遍的中间计数会波动；
            // 判定标准是最终 IR 不变（收敛点）。
            let (second, _) = pipeline.run_verified(&first).unwrap();
            assert_eq!(first, second, "alg={alg}");
            let (third, _) = pipeline.run_verified(&second).unwrap();
            assert_eq!(second, third, "alg={alg}");
        }
    }

    /// 故意写坏的 pass：把根换成无关变量。
    struct FakePass;

    impl Pass for FakePass {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn apply(&self, _ir: &Ir, _ctx: &PassCtx) -> Result<Option<Ir>, IrError> {
            let ir = Ir::from_expr(&Expr::Var("zzz".into()))?;
            Ok(Some(ir))
        }
    }

    #[test]
    fn verified_pipeline_rejects_a_broken_pass() {
        let ir = Ir::from_expr(&parse("a & b")).unwrap();
        let err = Pipeline::new(Algebra::Kleene)
            .with(FakePass)
            .run_verified(&ir)
            .unwrap_err();
        assert!(
            matches!(err, IrError::NotEquivalent { pass: "fake", .. }),
            "{err}"
        );
        // 未验证模式不会拦（用于对比：验证是有成本的）。
        assert!(
            Pipeline::new(Algebra::Kleene)
                .with(FakePass)
                .run(&ir)
                .is_ok()
        );
    }

    #[test]
    fn pipeline_reports_node_counts() {
        let ir = Ir::from_expr(&parse("(a & b) | (a & b)")).unwrap();
        let (out, reports) = Pipeline::default_pipeline(Algebra::Kleene)
            .run_verified(&ir)
            .unwrap();
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].name, "constant-fold");
        assert_eq!(reports[2].name, "cse");
        assert_eq!(reports[2].nodes_after, out.node_count());
        assert!(out.node_count() < ir.node_count(), "管道应减少节点");
    }
}
