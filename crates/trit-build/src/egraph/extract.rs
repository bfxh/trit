//! e-graph 的最小代价提取：代价表按根 class 建键，沿 parent 走根。

use super::*;

impl EGraph {
    fn kind_stats(
        &self,
        kind: &Kind,
        best: &HashMap<ClassId, (u64, u64)>,
        model: CostModel,
    ) -> Option<(u64, u64)> {
        match kind {
            Kind::Var(_) | Kind::Const(_) => Some((model.leaf(), 1)),
            Kind::Not(child) => best
                .get(&self.root(*child))
                .map(|(cost, nodes)| (cost.saturating_add(model.not()), nodes + 1)),
            Kind::Bin(op, left, right) => {
                let (left, right) = (self.root(*left), self.root(*right));
                let (left_cost, left_nodes) = *best.get(&left)?;
                let (right_cost, right_nodes) = *best.get(&right)?;
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
    pub(super) fn extract(&mut self, root: ClassId, model: CostModel) -> (Expr, u64, u64) {
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
