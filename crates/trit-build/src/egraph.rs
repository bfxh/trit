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

/// e-class 编号。
type ClassId = usize;

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

    const fn leaf(self) -> u64 {
        match self {
            Self::Nodes => 1,
            Self::Gates => 0,
        }
    }

    const fn op(self, op: Op) -> u64 {
        match self {
            Self::Nodes => 1,
            Self::Gates => match op {
                Op::And | Op::Or => 1,
                Op::Xor | Op::Implies => 2,
                Op::Equiv => 3,
            },
        }
    }

    const fn not(self) -> u64 {
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

fn expr_stats(expr: &Expr, model: CostModel) -> (u64, u64) {
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
mod tests {
    use super::*;
    use crate::bitplan;
    use trit_core::Parser;

    const ALGEBRAS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

    #[test]
    fn every_registered_rule_passes_self_check_in_kleene() {
        let active = active_rules(Algebra::Kleene);
        assert!(active.len() >= 15, "K3 规则集太窄：{active:?}");
        for name in [
            "de-morgan-and",
            "factor-and-or",
            "implies-elim",
            "equiv-elim",
        ] {
            assert!(active.contains(&name), "K3 缺少 {name}：{active:?}");
        }
        assert!(
            !active.contains(&"xor-elim"),
            "Xor 是强加法，不能按布尔异或消去"
        );
    }

    #[test]
    fn loader_filters_rules_by_algebra() {
        // 蕴含消去在 K3 成立；Ł3 的蕴含是 1-a+b，`a=>b ≡ !a|b` 不成立（U=>U=1 vs U|U=U）。
        assert!(active_rules(Algebra::Kleene).contains(&"implies-elim"));
        assert!(!active_rules(Algebra::Lukasiewicz).contains(&"implies-elim"));
        // 等价消去反过来：`equiv` 本身就是这么定义的（`(a⇒b) ∧ (b⇒a)`），所以三种语义都过。
        for alg in ALGEBRAS {
            assert!(
                active_rules(alg).contains(&"equiv-elim"),
                "{alg:?} 应保留 equiv-elim"
            );
        }
    }

    #[test]
    fn broken_rule_fails_self_check() {
        // 排中律在 K3 不成立：U | !U = U ≠ 1。
        let lhs = Expr::Or(
            Box::new(Expr::Var("a".into())),
            Box::new(Expr::Not(Box::new(Expr::Var("a".into())))),
        );
        let rhs = Expr::Const(Ternary::True);
        assert!(!self_check(&lhs, &rhs, Algebra::Kleene));
        // 同样的式子在三值上不成立、不过滤就进图的话，重写会静默改变语义。
        assert!(!self_check(&lhs, &rhs, Algebra::Lukasiewicz));
    }

    #[test]
    fn factoring_reduces_node_count() {
        let expr = Parser::parse("(a & b) | (a & c)").unwrap();
        let (rewritten, fired) = rewrite(&expr, Algebra::Kleene, 32);
        assert_eq!(rewritten.to_infix(), "(a ∧ (b ∨ c))", "fired={fired:?}");
        assert!(fired.contains(&"factor-and-or"), "fired={fired:?}");
    }

    #[test]
    fn rewrite_preserves_semantics_for_every_algebra() {
        for source in [
            "(a & b) | (a & c)",
            "!!(a | b)",
            "!(a & b) | c",
            "(a => b) & (b => a)",
            "(a ^ b) | (b & c)",
            "a & (a | b)",
            "((a & b) | (a & c)) & !d",
        ] {
            let expr = Parser::parse(source).unwrap();
            let vars = expr.variables();
            for alg in ALGEBRAS {
                let (rewritten, _) = rewrite(&expr, alg, 32);
                assert_eq!(
                    bitplan::table(&rewritten, &vars, alg, 1).unwrap(),
                    bitplan::table(&expr, &vars, alg, 1).unwrap(),
                    "{source} @ {alg:?} 重写后语义变了：{source} → {}",
                    rewritten.to_infix()
                );
            }
        }
    }

    #[test]
    fn rewrite_is_deterministic_and_stable() {
        let expr = Parser::parse("((a & b) | (a & c)) & !(b & c)").unwrap();
        let (first, _) = rewrite(&expr, Algebra::Kleene, 32);
        let (again, _) = rewrite(&expr, Algebra::Kleene, 32);
        assert_eq!(
            first.to_infix(),
            again.to_infix(),
            "同一输入两次结果必须相同"
        );
        let (second_pass, _) = rewrite(&first, Algebra::Kleene, 32);
        assert_eq!(first.to_infix(), second_pass.to_infix(), "重写必须是固定点");
    }

    #[test]
    fn saturation_stops_at_a_fixed_point() {
        let expr = Parser::parse("(((a & b) | (a & c)) & !(b & c)) | (a & b)").unwrap();
        let options = RewriteOptions::default();
        let first = rewrite_with(&expr, Algebra::Kleene, options);
        assert!(first.saturated, "默认上限内必须饱和：{first:?}");
        assert!(first.e_nodes > 0, "进图后应留下 e-class");
        assert!(first.rounds <= options.max_rounds);

        let second = rewrite_with(&first.expr, Algebra::Kleene, options);
        assert_eq!(first.expr.to_infix(), second.expr.to_infix());
        assert!(second.saturated);
        assert!(second.rounds <= 2, "固定点重跑最多再多一轮归一化");
    }

    #[test]
    fn nodes_limit_marks_the_run_unsaturated() {
        let expr = Parser::parse("((a & b) | (a & c)) & ((b & c) | (b & d))").unwrap();
        let report = rewrite_with(
            &expr,
            Algebra::Kleene,
            RewriteOptions {
                node_limit: 1,
                ..RewriteOptions::default()
            },
        );
        assert!(!report.saturated, "触到节点上限不能声称饱和");
    }

    #[test]
    fn cost_model_chooses_a_different_expression() {
        // A：5 个节点、2 个门；B：3 个节点、3 个门（等价/蕴含按多门算）。
        // 节点模型选 B，门模型选 A——这正是「有代价模型」与「只数节点」的差别。
        let a = Parser::parse("(a & b) & c").unwrap();
        let b = Parser::parse("a <=> b").unwrap();
        let mut graph = EGraph::new();
        let left = graph.add(&a);
        let right = graph.add(&b);
        graph.union(left, right);
        let (by_nodes, nodes_cost, _) = graph.extract(left, CostModel::Nodes);
        let (by_gates, gates_cost, _) = graph.extract(left, CostModel::Gates);
        assert_eq!(by_nodes.to_infix(), "(a ≡ b)", "节点数 3 < 5");
        assert_eq!(by_gates.to_infix(), "((a ∧ b) ∧ c)", "门数 2 < 3");
        assert_eq!(nodes_cost, 3);
        assert_eq!(gates_cost, 2);
    }

    #[test]
    fn report_cost_matches_the_model() {
        let expr = Parser::parse("a => b").unwrap();
        for model in [CostModel::Nodes, CostModel::Gates] {
            let report = rewrite_with(
                &expr,
                Algebra::Kleene,
                RewriteOptions {
                    cost: model,
                    ..RewriteOptions::default()
                },
            );
            assert_eq!(
                (report.cost, report.nodes),
                expr_stats(&report.expr, model),
                "{model:?} 的报告代价必须与表达式重算一致"
            );
        }
    }
}
