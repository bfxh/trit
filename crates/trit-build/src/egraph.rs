//! e-graph 重写（M3）：**规则先自证、再加载**，提取按节点数最小。
//!
//! 自证 = 对规则里出现的 ≤3 个变量穷举 3^k 组赋值，用生产同一份
//! `trit_eval::eval` 比较左右两边；有一组不等这条规则就**不进图**。
//! 所以同一条规则表可以喂给 K3/Ł3/B3：蕴含消去在 K3 成立、在 Ł3 不成立，
//! 它会自动只出现在 K3 的规则集里（见 `loader_filters_rules_by_algebra`）。
//!
//! 为什么用 e-graph 而不是一趟重写：`factor-and-or` 这类规则会把子式复制成两份，
//! 一趟树重写会指数膨胀；e-graph 用 e-class 合并共享，最后按节点数最小提取。

use std::collections::HashMap;

use trit_core::value::Ternary;
use trit_core::{Algebra, Expr};
use trit_eval::eval::{Assignment, eval};

mod cost;

use cost::expr_stats;
pub(crate) use cost::{CostModel, RewriteOptions, RewriteReport};

/// e-class 编号。
type ClassId = usize;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Op {
    And,
    Or,
    Xor,
    Implies,
    Equiv,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Kind {
    Var(usize),
    Const(Ternary),
    Not(ClassId),
    Bin(Op, ClassId, ClassId),
}

/// 规则模式（只有我们自己的规则用它，变量名是 `&'static str`）。
enum Pat {
    Var(&'static str),
    Const(Ternary),
    Not(Box<Pat>),
    Bin(Op, Box<Pat>, Box<Pat>),
}

struct Rule {
    name: &'static str,
    lhs: Pat,
    rhs: Pat,
}

fn v(name: &'static str) -> Pat {
    Pat::Var(name)
}

fn k(value: Ternary) -> Pat {
    Pat::Const(value)
}

fn not(inner: Pat) -> Pat {
    Pat::Not(Box::new(inner))
}

fn bin(op: Op, left: Pat, right: Pat) -> Pat {
    Pat::Bin(op, Box::new(left), Box::new(right))
}

fn rule(name: &'static str, lhs: Pat, rhs: Pat) -> Rule {
    Rule { name, lhs, rhs }
}

/// 规则表（方向都朝「更小/更可匹配」；语义等效由自证保证）。
fn rule_book() -> Vec<Rule> {
    vec![
        rule("idempotent-and", bin(Op::And, v("a"), v("a")), v("a")),
        rule("idempotent-or", bin(Op::Or, v("a"), v("a")), v("a")),
        rule("double-not", not(not(v("a"))), v("a")),
        rule("and-true", bin(Op::And, v("a"), k(Ternary::True)), v("a")),
        rule("or-false", bin(Op::Or, v("a"), k(Ternary::False)), v("a")),
        rule(
            "and-false",
            bin(Op::And, v("a"), k(Ternary::False)),
            k(Ternary::False),
        ),
        rule(
            "or-true",
            bin(Op::Or, v("a"), k(Ternary::True)),
            k(Ternary::True),
        ),
        rule(
            "de-morgan-and",
            not(bin(Op::And, v("a"), v("b"))),
            bin(Op::Or, not(v("a")), not(v("b"))),
        ),
        rule(
            "de-morgan-or",
            not(bin(Op::Or, v("a"), v("b"))),
            bin(Op::And, not(v("a")), not(v("b"))),
        ),
        rule(
            "absorption-and",
            bin(Op::And, v("a"), bin(Op::Or, v("a"), v("b"))),
            v("a"),
        ),
        rule(
            "absorption-or",
            bin(Op::Or, v("a"), bin(Op::And, v("a"), v("b"))),
            v("a"),
        ),
        rule(
            "commute-and",
            bin(Op::And, v("a"), v("b")),
            bin(Op::And, v("b"), v("a")),
        ),
        rule(
            "commute-or",
            bin(Op::Or, v("a"), v("b")),
            bin(Op::Or, v("b"), v("a")),
        ),
        rule(
            "factor-and-or",
            bin(
                Op::Or,
                bin(Op::And, v("a"), v("b")),
                bin(Op::And, v("a"), v("c")),
            ),
            bin(Op::And, v("a"), bin(Op::Or, v("b"), v("c"))),
        ),
        rule(
            "factor-and-or-right",
            bin(
                Op::Or,
                bin(Op::And, v("b"), v("a")),
                bin(Op::And, v("c"), v("a")),
            ),
            bin(Op::And, bin(Op::Or, v("b"), v("c")), v("a")),
        ),
        rule(
            "implies-elim",
            bin(Op::Implies, v("a"), v("b")),
            bin(Op::Or, not(v("a")), v("b")),
        ),
        rule(
            "equiv-elim",
            bin(Op::Equiv, v("a"), v("b")),
            bin(
                Op::And,
                bin(Op::Implies, v("a"), v("b")),
                bin(Op::Implies, v("b"), v("a")),
            ),
        ),
        // 没有 xor-elim：本仓的 `Expr::Xor` 是**强加法 ⊕**（K3 下等价于 AND、Ł3 下是
        // min(1, a+b)），不是布尔异或，写不出少节点的 And/Or 展开。自证会直接否掉它。
    ]
}

fn pattern_vars(pat: &Pat) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut stack = vec![pat];
    while let Some(node) = stack.pop() {
        match node {
            Pat::Var(name) => {
                if !out.contains(name) {
                    out.push(*name);
                }
            }
            Pat::Const(_) => {}
            Pat::Not(inner) => stack.push(inner),
            Pat::Bin(_, left, right) => {
                stack.push(left);
                stack.push(right);
            }
        }
    }
    out
}

/// 把模式按给定取值实例化成常量表达式（自证用）。
fn instantiate(pat: &Pat, subst: &HashMap<&'static str, Ternary>) -> Expr {
    match pat {
        Pat::Var(name) => Expr::Const(*subst.get(name).expect("模式变量必须有取值")),
        Pat::Const(value) => Expr::Const(*value),
        Pat::Not(inner) => Expr::Not(Box::new(instantiate(inner, subst))),
        Pat::Bin(op, left, right) => {
            let left = Box::new(instantiate(left, subst));
            let right = Box::new(instantiate(right, subst));
            match op {
                Op::And => Expr::And(left, right),
                Op::Or => Expr::Or(left, right),
                Op::Xor => Expr::Xor(left, right),
                Op::Implies => Expr::Implies(left, right),
                Op::Equiv => Expr::Equiv(left, right),
            }
        }
    }
}

/// 规则是否在给定语义下恒等：≤3 变量穷举；超过 3 个变量的一律不收。
#[cfg(test)]
pub(crate) fn self_check(rule_lhs: &Expr, rule_rhs: &Expr, alg: Algebra) -> bool {
    let mut names = rule_lhs.variables();
    for name in rule_rhs.variables() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.len() > 3 {
        return false;
    }
    let count = 3usize.pow(names.len() as u32);
    for code in 0..count {
        let mut assignment = Assignment::new();
        let mut rest = code;
        for name in &names {
            let value = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
            assignment.set(name.clone(), value);
        }
        if eval(rule_lhs, &assignment, alg) != eval(rule_rhs, &assignment, alg) {
            return false;
        }
    }
    true
}

/// 一条规则对某种语义是否可用：对规则里出现的 ≤3 个变量穷举 3^k 组取值。
fn rule_holds(rule: &Rule, alg: Algebra) -> bool {
    let mut names = pattern_vars(&rule.lhs);
    for name in pattern_vars(&rule.rhs) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.len() > 3 {
        return false;
    }
    let count = 3usize.pow(names.len() as u32);
    for code in 0..count {
        let mut subst: HashMap<&'static str, Ternary> = HashMap::new();
        let mut rest = code;
        for name in &names {
            let value = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
            subst.insert(*name, value);
        }
        let lhs = instantiate(&rule.lhs, &subst);
        let rhs = instantiate(&rule.rhs, &subst);
        let empty = Assignment::new();
        if eval(&lhs, &empty, alg) != eval(&rhs, &empty, alg) {
            return false;
        }
    }
    true
}

/// 该语义下通过自证的规则名（顺序固定，便于断言）。
#[cfg(test)]
pub(crate) fn active_rules(alg: Algebra) -> Vec<&'static str> {
    rule_book()
        .iter()
        .filter(|rule| rule_holds(rule, alg))
        .map(|rule| rule.name)
        .collect()
}

struct EGraph {
    parent: Vec<ClassId>,
    classes: Vec<Vec<Kind>>,
    hash: HashMap<Kind, ClassId>,
    names: Vec<String>,
}

impl EGraph {
    fn new() -> Self {
        Self {
            parent: Vec::new(),
            classes: Vec::new(),
            hash: HashMap::new(),
            names: Vec::new(),
        }
    }

    fn find(&mut self, mut class: ClassId) -> ClassId {
        while self.parent[class] != class {
            let grand = self.parent[self.parent[class]];
            self.parent[class] = grand;
            class = grand;
        }
        class
    }

    fn intern(&mut self, name: &str) -> usize {
        match self.names.iter().position(|known| known == name) {
            Some(index) => index,
            None => {
                self.names.push(name.to_string());
                self.names.len() - 1
            }
        }
    }

    fn canon(&mut self, kind: &Kind) -> Kind {
        match kind.clone() {
            Kind::Var(_) | Kind::Const(_) => kind.clone(),
            Kind::Not(child) => Kind::Not(self.find(child)),
            Kind::Bin(op, left, right) => {
                let (left, right) = (self.find(left), self.find(right));
                Kind::Bin(op, left, right)
            }
        }
    }

    fn add_kind(&mut self, kind: Kind) -> ClassId {
        let key = self.canon(&kind);
        if let Some(class) = self.hash.get(&key) {
            return *class;
        }
        let class = self.classes.len();
        self.parent.push(class);
        self.classes.push(vec![key.clone()]);
        self.hash.insert(key, class);
        class
    }

    fn add(&mut self, expr: &Expr) -> ClassId {
        let kind = match expr {
            Expr::Const(value) => Kind::Const(*value),
            Expr::Var(name) => {
                let id = self.intern(name);
                Kind::Var(id)
            }
            Expr::Not(inner) => {
                let child = self.add(inner);
                Kind::Not(child)
            }
            Expr::And(left, right) => {
                let (left, right) = (self.add(left), self.add(right));
                Kind::Bin(Op::And, left, right)
            }
            Expr::Or(left, right) => {
                let (left, right) = (self.add(left), self.add(right));
                Kind::Bin(Op::Or, left, right)
            }
            Expr::Xor(left, right) => {
                let (left, right) = (self.add(left), self.add(right));
                Kind::Bin(Op::Xor, left, right)
            }
            Expr::Implies(left, right) => {
                let (left, right) = (self.add(left), self.add(right));
                Kind::Bin(Op::Implies, left, right)
            }
            Expr::Equiv(left, right) => {
                let (left, right) = (self.add(left), self.add(right));
                Kind::Bin(Op::Equiv, left, right)
            }
        };
        self.add_kind(kind)
    }

    fn union_raw(&mut self, a: ClassId, b: ClassId) {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        let (keep, drop) = if self.classes[a].len() >= self.classes[b].len() {
            (a, b)
        } else {
            (b, a)
        };
        self.parent[drop] = keep;
        let moved = std::mem::take(&mut self.classes[drop]);
        self.classes[keep].extend(moved);
    }

    /// 合并后重建同余闭包：孩子 id 规范相同的两个节点必须同 class。
    fn rebuild(&mut self) {
        for _ in 0..self.classes.len().max(1) * 2 {
            let roots: Vec<ClassId> = (0..self.classes.len())
                .filter(|class| self.find(*class) == *class)
                .collect();
            let mut map: HashMap<Kind, ClassId> = HashMap::new();
            let mut merges: Vec<(ClassId, ClassId)> = Vec::new();
            for class in roots {
                for kind in self.classes[class].clone() {
                    let key = self.canon(&kind);
                    match map.get(&key) {
                        Some(&other) if other != class => merges.push((class, other)),
                        Some(_) => {}
                        None => {
                            map.insert(key, class);
                        }
                    }
                }
            }
            if merges.is_empty() {
                self.hash = map;
                return;
            }
            for (left, right) in merges {
                self.union_raw(left, right);
            }
        }
    }

    fn union(&mut self, a: ClassId, b: ClassId) {
        self.union_raw(a, b);
        self.rebuild();
    }

    fn match_pattern(
        &mut self,
        pat: &Pat,
        class: ClassId,
        subst: &mut HashMap<&'static str, ClassId>,
    ) -> bool {
        let class = self.find(class);
        match pat {
            Pat::Var(name) => match subst.get(name) {
                Some(bound) => self.find(*bound) == class,
                None => {
                    subst.insert(name, class);
                    true
                }
            },
            Pat::Const(value) => self.classes[class]
                .iter()
                .any(|kind| matches!(kind, Kind::Const(other) if other == value)),
            Pat::Not(inner) => {
                let children: Vec<ClassId> = self.classes[class]
                    .iter()
                    .filter_map(|kind| match kind {
                        Kind::Not(child) => Some(*child),
                        _ => None,
                    })
                    .collect();
                children.into_iter().any(|child| {
                    let mut trial = subst.clone();
                    if self.match_pattern(inner, child, &mut trial) {
                        *subst = trial;
                        true
                    } else {
                        false
                    }
                })
            }
            Pat::Bin(op, left, right) => {
                let pairs: Vec<(ClassId, ClassId)> = self.classes[class]
                    .iter()
                    .filter_map(|kind| match kind {
                        Kind::Bin(kind_op, l, r) if kind_op == op => Some((*l, *r)),
                        _ => None,
                    })
                    .collect();
                pairs.into_iter().any(|(l, r)| {
                    let mut trial = subst.clone();
                    if self.match_pattern(left, l, &mut trial)
                        && self.match_pattern(right, r, &mut trial)
                    {
                        *subst = trial;
                        true
                    } else {
                        false
                    }
                })
            }
        }
    }

    /// 规则右侧在图上落到哪个 e-class：变量取已绑定的 class，其余按算子新建/复用。
    fn rhs_class(&mut self, pat: &Pat, subst: &HashMap<&'static str, ClassId>) -> ClassId {
        match pat {
            Pat::Var(name) => {
                let bound = *subst.get(name).expect("匹配成功的模式变量必须有绑定");
                self.find(bound)
            }
            Pat::Const(value) => self.add_kind(Kind::Const(*value)),
            Pat::Not(inner) => {
                let child = self.rhs_class(inner, subst);
                self.add_kind(Kind::Not(child))
            }
            Pat::Bin(op, left, right) => {
                let (left, right) = (self.rhs_class(left, subst), self.rhs_class(right, subst));
                self.add_kind(Kind::Bin(*op, left, right))
            }
        }
    }

    fn kind_stats(
        &self,
        kind: &Kind,
        best: &HashMap<ClassId, (u64, u64)>,
        model: CostModel,
    ) -> Option<(u64, u64)> {
        match kind {
            Kind::Var(_) | Kind::Const(_) => Some((model.leaf(), 1)),
            Kind::Not(child) => best
                .get(child)
                .map(|(cost, nodes)| (cost.saturating_add(model.not()), nodes + 1)),
            Kind::Bin(op, left, right) => {
                let (left_cost, left_nodes) = *best.get(left)?;
                let (right_cost, right_nodes) = *best.get(right)?;
                Some((
                    left_cost
                        .saturating_add(right_cost)
                        .saturating_add(model.op(*op)),
                    left_nodes + right_nodes + 1,
                ))
            }
        }
    }

    /// 最小 (代价, 节点数) 提取；固定点迭代 + 严格小于，保证确定性。
    fn extract(&mut self, root: ClassId, model: CostModel) -> (Expr, u64, u64) {
        let roots: Vec<ClassId> = (0..self.classes.len())
            .filter(|class| self.find(*class) == *class)
            .collect();
        let mut best: HashMap<ClassId, (u64, u64)> = HashMap::new();
        // 每条边至少把子节点信息传一层；|roots| 轮一定覆盖最长链。
        for _ in 0..=roots.len() {
            let mut changed = false;
            for class in &roots {
                let mut choice: Option<(u64, u64)> = None;
                for kind in &self.classes[*class] {
                    let Some(candidate) = self.kind_stats(kind, &best, model) else {
                        continue;
                    };
                    if choice.is_none_or(|current| candidate < current) {
                        choice = Some(candidate);
                    }
                }
                if let Some(candidate) = choice
                    && best.get(class) != Some(&candidate)
                {
                    best.insert(*class, candidate);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let root = self.find(root);
        let (cost, nodes) = best.get(&root).copied().unwrap_or((u64::MAX, u64::MAX));
        (self.build(root, &best, model, 0), cost, nodes)
    }

    fn choose_kind(
        &self,
        class: ClassId,
        best: &HashMap<ClassId, (u64, u64)>,
        model: CostModel,
    ) -> Option<(u64, u64, &Kind)> {
        let mut chosen: Option<(u64, u64, &Kind)> = None;
        for kind in &self.classes[class] {
            let Some((cost, nodes)) = self.kind_stats(kind, best, model) else {
                continue;
            };
            if chosen
                .is_none_or(|(best_cost, best_nodes, _)| (cost, nodes) < (best_cost, best_nodes))
            {
                chosen = Some((cost, nodes, kind));
            }
        }
        chosen
    }

    fn build(
        &self,
        class: ClassId,
        best: &HashMap<ClassId, (u64, u64)>,
        model: CostModel,
        depth: usize,
    ) -> Expr {
        if depth > 256 {
            return Expr::Const(Ternary::Unknown);
        }
        let Some((_, _, kind)) = self.choose_kind(class, best, model) else {
            return Expr::Const(Ternary::Unknown);
        };
        match kind {
            Kind::Var(index) => Expr::Var(self.names[*index].clone()),
            Kind::Const(value) => Expr::Const(*value),
            Kind::Not(child) => Expr::Not(Box::new(self.follow(*child, best, model, depth + 1))),
            Kind::Bin(op, left, right) => {
                let left = Box::new(self.follow(*left, best, model, depth + 1));
                let right = Box::new(self.follow(*right, best, model, depth + 1));
                match op {
                    Op::And => Expr::And(left, right),
                    Op::Or => Expr::Or(left, right),
                    Op::Xor => Expr::Xor(left, right),
                    Op::Implies => Expr::Implies(left, right),
                    Op::Equiv => Expr::Equiv(left, right),
                }
            }
        }
    }

    fn follow(
        &self,
        class: ClassId,
        best: &HashMap<ClassId, (u64, u64)>,
        model: CostModel,
        depth: usize,
    ) -> Expr {
        // 子节点 id 在 rebuild 后可能指向非根，这里沿 parent 走到根。
        let mut class = class;
        while self.parent[class] != class {
            class = self.parent[class];
        }
        self.build(class, best, model, depth)
    }
}

/// 重写入口：返回（重写后的表达式，实际触发过的规则名）。
#[cfg(test)]
pub(crate) fn rewrite(expr: &Expr, alg: Algebra, max_rounds: usize) -> (Expr, Vec<&'static str>) {
    let report = rewrite_with(
        expr,
        alg,
        RewriteOptions {
            max_rounds,
            ..RewriteOptions::default()
        },
    );
    (report.expr, report.rules)
}

/// 饱和重写：每轮应用所有规则，直到没有新的合并（不动点）或触到轮数/节点上限。
pub(crate) fn rewrite_with(expr: &Expr, alg: Algebra, options: RewriteOptions) -> RewriteReport {
    let rules: Vec<Rule> = rule_book()
        .into_iter()
        .filter(|rule| rule_holds(rule, alg))
        .collect();
    if rules.is_empty() {
        let (cost, nodes) = expr_stats(expr, options.cost);
        return RewriteReport {
            expr: expr.clone(),
            rules: Vec::new(),
            rounds: 0,
            e_nodes: 0,
            saturated: true,
            cost,
            nodes,
        };
    }
    let mut graph = EGraph::new();
    let root = graph.add(expr);
    let mut fired: Vec<&'static str> = Vec::new();
    let mut rounds = 0;
    let mut saturated = false;
    for _ in 0..options.max_rounds {
        rounds += 1;
        let mut changed = false;
        let classes: Vec<ClassId> = (0..graph.classes.len())
            .filter(|class| graph.find(*class) == *class)
            .collect();
        for class in classes {
            for rule in &rules {
                let mut subst = HashMap::new();
                if !graph.match_pattern(&rule.lhs, class, &mut subst) {
                    continue;
                }
                let target = graph.rhs_class(&rule.rhs, &subst);
                if graph.find(class) != graph.find(target) {
                    graph.union(class, target);
                    fired.push(rule.name);
                    changed = true;
                }
            }
            if graph.classes.len() > options.node_limit {
                break;
            }
        }
        if graph.classes.len() > options.node_limit {
            break;
        }
        if !changed {
            saturated = true;
            break;
        }
    }
    let e_nodes = (0..graph.classes.len())
        .filter(|class| graph.find(*class) == *class)
        .count();
    let (rewritten, cost, nodes) = graph.extract(root, options.cost);
    fired.sort_unstable();
    fired.dedup();
    RewriteReport {
        expr: rewritten,
        rules: fired,
        rounds,
        e_nodes,
        saturated,
        cost,
        nodes,
    }
}

#[cfg(test)]
mod tests;
