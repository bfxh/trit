//! Belnap 四值逻辑（FOUR）：`T` / `F` / `B`（both）/ `N`（neither）。
//!
//! 这是 **opt-in** 语义：三值的 K3/Ł3/B3 不受影响，四值只在本 crate 里出现。
//!
//! 双轨表示：`T=(1,0)`、`F=(0,1)`、`B=(1,1)`、`N=(0,0)`。三值双导轨禁止 `(1,1)`，
//! 四值正是把这条约束放开——`B` 同时为真和为假，`N` 两者都不是。
//!
//! ```text
//! ∧ : t = t₁∧t₂，f = f₁∨f₂      // N∧B = F
//! ∨ : t = t₁∨t₂，f = f₁∧f₂      // N∨B = T
//! ¬ : 交换两轨
//! ⇒ : ¬a ∨ b
//! ```
//!
//! 注意 `⊕` 在 FOUR 里没有公认定义：这里取 `¬(a ⇔ b)`，`B ⊕ B = B`，不要按经典异或理解。

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use trit_core::ast::Expr;
use trit_core::limits::{LimitError, Limits, check_expr};
use trit_core::value::Ternary;

/// Belnap 四值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum B4 {
    /// 两者都不是（neither）。
    N,
    /// 假。
    F,
    /// 真。
    T,
    /// 同时为真和为假（both）。
    B,
}

impl B4 {
    /// 由双轨 `(真, 假)` 构造。
    pub const fn from_rails(t: bool, f: bool) -> Self {
        match (t, f) {
            (true, false) => Self::T,
            (false, true) => Self::F,
            (true, true) => Self::B,
            (false, false) => Self::N,
        }
    }

    /// 拆成双轨 `(真, 假)`。
    pub const fn to_rails(self) -> (bool, bool) {
        match self {
            Self::T => (true, false),
            Self::F => (false, true),
            Self::B => (true, true),
            Self::N => (false, false),
        }
    }

    /// 否定：交换两轨。
    pub const fn not(self) -> Self {
        let (t, f) = self.to_rails();
        Self::from_rails(f, t)
    }

    /// 合取：`t = t₁∧t₂`，`f = f₁∨f₂`。
    pub const fn and(self, other: Self) -> Self {
        let (t1, f1) = self.to_rails();
        let (t2, f2) = other.to_rails();
        Self::from_rails(t1 && t2, f1 || f2)
    }

    /// 析取：`t = t₁∨t₂`，`f = f₁∧f₂`。
    pub const fn or(self, other: Self) -> Self {
        let (t1, f1) = self.to_rails();
        let (t2, f2) = other.to_rails();
        Self::from_rails(t1 || t2, f1 && f2)
    }

    /// 蕴含：`¬a ∨ b`。
    pub const fn implies(self, other: Self) -> Self {
        self.not().or(other)
    }

    /// 等价：`(a ⇒ b) ∧ (b ⇒ a)`。
    pub const fn equiv(self, other: Self) -> Self {
        self.implies(other).and(other.implies(self))
    }

    /// 异或：`¬(a ⇔ b)`。FOUR 下不是经典异或。
    pub const fn xor(self, other: Self) -> Self {
        self.equiv(other).not()
    }

    /// 知识序合取（共识）：两轨各自 AND。
    ///
    /// 信息口径的“共识”：`T ⊗ B = T`、`T ⊗ F = N`、`B ⊗ B = B`。
    pub const fn consensus(self, other: Self) -> Self {
        let (t1, f1) = self.to_rails();
        let (t2, f2) = other.to_rails();
        Self::from_rails(t1 && t2, f1 && f2)
    }

    /// 知识序析取（轻信）：两轨各自 OR。
    ///
    /// `T ⊕ F = B`、`N ⊕ x = x`。
    pub const fn gullibility(self, other: Self) -> Self {
        let (t1, f1) = self.to_rails();
        let (t2, f2) = other.to_rails();
        Self::from_rails(t1 || t2, f1 || f2)
    }

    /// 共轭（conflation）`¬ₖ`：交换知识序的两端（`N↔B`，`T`/`F` 不动）。
    ///
    /// Belnap 否定只对**真序**是 De Morgan 否定；知识序的 De Morgan 要用它。
    pub const fn conflation(self) -> Self {
        let (t, f) = self.to_rails();
        Self::from_rails(!f, !t)
    }

    /// 真序 `≤_t`：`a ≤_t b ⇔ a ∧ b = a`。
    pub fn leq_truth(self, other: Self) -> bool {
        self.and(other) == self
    }

    /// 知识序 `≤_k`：`a ≤_k b ⇔ a ⊗ b = a`。
    pub fn leq_knowledge(self, other: Self) -> bool {
        self.consensus(other) == self
    }

    /// 是否为“确定为真”的值（`T` 或 `B`）。
    pub const fn is_designated(self) -> bool {
        matches!(self, Self::T | Self::B)
    }

    /// 单字符表示：`N` / `F` / `T` / `B`。
    pub const fn as_char(self) -> char {
        match self {
            Self::N => 'N',
            Self::F => 'F',
            Self::T => 'T',
            Self::B => 'B',
        }
    }

    /// 解析单字符：`0/f`→F，`1/t`→T，`?/x`→N，`*/b`→B。
    pub fn parse_char(c: char) -> Option<Self> {
        match c {
            '0' | 'f' | 'F' => Some(Self::F),
            '1' | 't' | 'T' => Some(Self::T),
            '?' | 'x' | 'X' | 'n' | 'N' => Some(Self::N),
            '*' | 'b' | 'B' => Some(Self::B),
            _ => None,
        }
    }
}

/// 按**真序**合并多个来源（`∨_t` 折叠；空集合 ⇒ `N`）。
pub fn merge_truth<I: IntoIterator<Item = B4>>(sources: I) -> B4 {
    let mut sources = sources.into_iter();
    match sources.next() {
        None => B4::N,
        Some(first) => sources.fold(first, B4::or),
    }
}

/// 按**知识序**合并多个来源（共识 `⊗` 折叠；空集合 ⇒ `N`）。
pub fn merge_knowledge<I: IntoIterator<Item = B4>>(sources: I) -> B4 {
    let mut sources = sources.into_iter();
    match sources.next() {
        None => B4::N,
        Some(first) => sources.fold(first, B4::consensus),
    }
}

impl std::fmt::Display for B4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_char().to_string())
    }
}

impl From<Ternary> for B4 {
    /// 三值常量映射：`False→F`、`True→T`、`Unknown→N`。
    fn from(value: Ternary) -> Self {
        match value {
            Ternary::False => Self::F,
            Ternary::True => Self::T,
            Ternary::Unknown => Self::N,
        }
    }
}

/// 在给定赋值下求值；未出现的变量按 `N` 处理。
pub fn eval(expr: &Expr, assignment: &BTreeMap<String, B4>) -> B4 {
    match expr {
        Expr::Const(value) => B4::from(*value),
        Expr::Var(name) => assignment.get(name).copied().unwrap_or(B4::N),
        Expr::Not(inner) => eval(inner, assignment).not(),
        Expr::And(a, b) => eval(a, assignment).and(eval(b, assignment)),
        Expr::Or(a, b) => eval(a, assignment).or(eval(b, assignment)),
        Expr::Xor(a, b) => eval(a, assignment).xor(eval(b, assignment)),
        Expr::Implies(a, b) => eval(a, assignment).implies(eval(b, assignment)),
        Expr::Equiv(a, b) => eval(a, assignment).equiv(eval(b, assignment)),
    }
}

/// 一行真值表：按 `vars` 顺序的赋值与结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// 每个变量的取值。
    pub values: Vec<B4>,
    /// 表达式结果。
    pub value: B4,
}

/// 4^n 真值表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// 变量名（首次出现顺序）。
    pub vars: Vec<String>,
    /// 全部 `4^n` 行。
    pub rows: Vec<Row>,
}

/// 生成 4^n 真值表（默认预算）。
///
/// 变量数、`4^n` 容量与行数都在分配前按 [`Limits`] 检查；超出时返回错误。
pub fn table(expr: &Expr) -> Result<Table, LimitError> {
    table_within(expr, Limits::default())
}

/// 同 [`table`]，但可指定预算。
pub fn table_within(expr: &Expr, limits: Limits) -> Result<Table, LimitError> {
    check_expr(expr, limits)?;
    let vars = expr.variables();
    if vars.len() > limits.max_vars {
        return Err(LimitError::Vars {
            limit: limits.max_vars,
        });
    }
    let exp = u32::try_from(vars.len()).unwrap_or(u32::MAX);
    let count = 4u64
        .checked_pow(exp)
        .ok_or(LimitError::CountOverflow { vars: vars.len() })?;
    if count > limits.max_rows as u64 {
        return Err(LimitError::Rows {
            limit: limits.max_rows,
        });
    }
    let count = usize::try_from(count).map_err(|_| LimitError::Rows {
        limit: limits.max_rows,
    })?;
    let mut rows = Vec::with_capacity(count.min(1 << 16));
    for combo in 0..count {
        let mut index = combo;
        let mut assignment = BTreeMap::new();
        let mut values = Vec::with_capacity(vars.len());
        for var in &vars {
            let value = match index % 4 {
                0 => B4::N,
                1 => B4::F,
                2 => B4::T,
                _ => B4::B,
            };
            index /= 4;
            values.push(value);
            assignment.insert(var.clone(), value);
        }
        rows.push(Row {
            values,
            value: eval(expr, &assignment),
        });
    }
    Ok(Table { vars, rows })
}

/// 在全部 `4^n` 赋值上判定两个表达式是否逐点相等（默认预算）。
///
/// `None` 表示等价；`Some(assignment)` 是一个反例。超出预算时返回错误。
pub fn equivalent(left: &Expr, right: &Expr) -> Result<Option<BTreeMap<String, B4>>, LimitError> {
    equivalent_within(left, right, Limits::default())
}

/// 同 [`equivalent`]，但可指定预算。
pub fn equivalent_within(
    left: &Expr,
    right: &Expr,
    limits: Limits,
) -> Result<Option<BTreeMap<String, B4>>, LimitError> {
    check_expr(left, limits)?;
    check_expr(right, limits)?;
    let mut vars = left.variables();
    for var in right.variables() {
        if !vars.contains(&var) {
            vars.push(var);
        }
    }
    if vars.len() > limits.max_vars {
        return Err(LimitError::Vars {
            limit: limits.max_vars,
        });
    }
    let exp = u32::try_from(vars.len()).unwrap_or(u32::MAX);
    let count = 4u64
        .checked_pow(exp)
        .ok_or(LimitError::CountOverflow { vars: vars.len() })?;
    if count > limits.max_rows as u64 {
        return Err(LimitError::Rows {
            limit: limits.max_rows,
        });
    }
    let count = usize::try_from(count).map_err(|_| LimitError::Rows {
        limit: limits.max_rows,
    })?;
    for combo in 0..count {
        let mut index = combo;
        let mut assignment = BTreeMap::new();
        for var in &vars {
            let value = match index % 4 {
                0 => B4::N,
                1 => B4::F,
                2 => B4::T,
                _ => B4::B,
            };
            index /= 4;
            assignment.insert(var.clone(), value);
        }
        if eval(left, &assignment) != eval(right, &assignment) {
            return Ok(Some(assignment));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四值全集（表的顺序：N/F/T/B）。
    const B4_VALUES: [B4; 4] = [B4::N, B4::F, B4::T, B4::B];
    use trit_core::parser::Parser;

    fn p(src: &str) -> Expr {
        Parser::parse(src).unwrap()
    }

    fn asg(pairs: &[(&str, B4)]) -> BTreeMap<String, B4> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn rails_roundtrip() {
        for v in [B4::N, B4::F, B4::T, B4::B] {
            let (t, f) = v.to_rails();
            assert_eq!(B4::from_rails(t, f), v);
        }
    }

    #[test]
    fn truth_tables_have_the_expected_conflicts() {
        assert_eq!(B4::N.and(B4::B), B4::F, "N∧B = F");
        assert_eq!(B4::N.or(B4::B), B4::T, "N∨B = T");
        assert_eq!(B4::B.not(), B4::B);
        assert_eq!(B4::B.implies(B4::F), B4::B);
        assert_eq!(B4::T.implies(B4::B), B4::B);
        assert_eq!(B4::T.and(B4::B), B4::B);
        assert_eq!(B4::F.or(B4::B), B4::B);
    }

    #[test]
    fn de_morgan_holds_on_all_sixteen_assignments() {
        let left = p("!(a & b)");
        let right = p("!a | !b");
        assert!(equivalent(&left, &right).unwrap().is_none());
    }

    #[test]
    fn paraconsistent_pair_is_not_constant() {
        // a ∧ ¬a 在 a=B 时为 B、在 a=N 时为 N，所以与 0 不等价。
        let counterexample = equivalent(&p("a & !a"), &p("0"))
            .unwrap()
            .expect("应当有反例");
        assert!(matches!(counterexample["a"], B4::N | B4::B));
        assert_eq!(B4::B.and(B4::B.not()), B4::B);
    }

    #[test]
    fn table_enumerates_four_to_the_n() {
        let t = table(&p("a & b")).unwrap();
        assert_eq!(t.vars, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(t.rows.len(), 16);
    }

    #[test]
    fn budgets_are_enforced_before_enumeration() {
        let two_vars = p("a & b");
        let row_limited = Limits {
            max_rows: 15,
            ..Limits::default()
        };
        assert!(matches!(
            table_within(&two_vars, row_limited),
            Err(LimitError::Rows { limit: 15 })
        ));
        let var_limited = Limits {
            max_vars: 1,
            ..Limits::default()
        };
        assert!(matches!(
            table_within(&two_vars, var_limited),
            Err(LimitError::Vars { limit: 1 })
        ));
        assert!(matches!(
            equivalent_within(&two_vars, &p("b & a"), var_limited),
            Err(LimitError::Vars { limit: 1 })
        ));
    }

    #[test]
    fn eval_defaults_missing_variables_to_neither() {
        assert_eq!(eval(&p("a | b"), &asg(&[("a", B4::F)])), B4::N);
        assert_eq!(
            eval(&p("a & b"), &asg(&[("a", B4::N), ("b", B4::B)])),
            B4::F
        );
        assert_eq!(
            eval(&p("a | b"), &asg(&[("a", B4::N), ("b", B4::B)])),
            B4::T
        );
    }

    #[test]
    fn truth_lattice_axioms_hold() {
        for a in B4_VALUES {
            for b in B4_VALUES {
                assert_eq!(a.and(b), b.and(a), "交换律 a={a} b={b}");
                assert_eq!(a.or(b), b.or(a), "交换律 a={a} b={b}");
                assert_eq!(a.and(a), a);
                assert_eq!(a.or(a), a);
                // 吸收律
                assert_eq!(a.and(a.or(b)), a, "吸收律 a={a} b={b}");
                assert_eq!(a.or(a.and(b)), a, "吸收律 a={a} b={b}");
                for c in B4_VALUES {
                    assert_eq!(a.and(b).and(c), a.and(b.and(c)), "结合律");
                    assert_eq!(a.or(b).or(c), a.or(b.or(c)), "结合律");
                    // 分配律
                    assert_eq!(
                        a.and(b.or(c)),
                        a.and(b).or(a.and(c)),
                        "真序分配律 a={a} b={b} c={c}"
                    );
                    assert_eq!(
                        a.or(b.and(c)),
                        a.or(b).and(a.or(c)),
                        "真序分配律 a={a} b={b} c={c}"
                    );
                }
            }
        }
    }

    #[test]
    fn knowledge_lattice_axioms_hold() {
        for a in B4_VALUES {
            for b in B4_VALUES {
                assert_eq!(a.consensus(b), b.consensus(a), "交换律 a={a} b={b}");
                assert_eq!(a.gullibility(b), b.gullibility(a), "交换律 a={a} b={b}");
                assert_eq!(a.consensus(a), a);
                assert_eq!(a.gullibility(a), a);
                assert_eq!(a.consensus(a.gullibility(b)), a, "吸收律 a={a} b={b}");
                assert_eq!(a.gullibility(a.consensus(b)), a, "吸收律 a={a} b={b}");
                for c in B4_VALUES {
                    assert_eq!(
                        a.consensus(b).consensus(c),
                        a.consensus(b.consensus(c)),
                        "结合律"
                    );
                    assert_eq!(
                        a.gullibility(b).gullibility(c),
                        a.gullibility(b.gullibility(c)),
                        "结合律"
                    );
                    assert_eq!(
                        a.consensus(b.gullibility(c)),
                        a.consensus(b).gullibility(a.consensus(c)),
                        "知识序分配律 a={a} b={b} c={c}"
                    );
                    assert_eq!(
                        a.gullibility(b.consensus(c)),
                        a.gullibility(b).consensus(a.gullibility(c)),
                        "知识序分配律 a={a} b={b} c={c}"
                    );
                }
            }
        }
    }

    #[test]
    fn orders_match_known_relations() {
        assert!(B4::F.leq_truth(B4::N));
        assert!(B4::N.leq_truth(B4::T));
        assert!(B4::B.leq_truth(B4::T));
        assert!(B4::F.leq_truth(B4::B));
        assert!(!B4::N.leq_truth(B4::B));
        assert!(!B4::B.leq_truth(B4::N));

        assert!(B4::N.leq_knowledge(B4::F));
        assert!(B4::N.leq_knowledge(B4::T));
        assert!(B4::T.leq_knowledge(B4::B));
        assert!(B4::F.leq_knowledge(B4::B));
        assert!(!B4::B.leq_knowledge(B4::T));
        assert!(!B4::F.leq_knowledge(B4::T));
    }

    #[test]
    fn conflation_table_is_pinned() {
        // 知识序的两端互换，T/F 不动：这正是“只看信息量、不看真假方向”的共轭。
        assert_eq!(B4::T.conflation(), B4::T);
        assert_eq!(B4::F.conflation(), B4::F);
        assert_eq!(B4::B.conflation(), B4::N);
        assert_eq!(B4::N.conflation(), B4::B);
        for a in B4_VALUES {
            assert_eq!(a.conflation().conflation(), a, "二次共轭还原 a={a}");
        }
    }

    #[test]
    fn negation_is_de_morgan_for_both_orders() {
        for a in B4_VALUES {
            assert_eq!(a.not().not(), a, "双重否定 a={a}");
            for b in B4_VALUES {
                assert_eq!(a.and(b).not(), a.not().or(b.not()));
                assert_eq!(a.or(b).not(), a.not().and(b.not()));
                // 知识序的 De Morgan 用 conflation（Belnap 否定对知识序不成立）。
                assert_eq!(
                    a.consensus(b).conflation(),
                    a.conflation().gullibility(b.conflation())
                );
                assert_eq!(
                    a.gullibility(b).conflation(),
                    a.conflation().consensus(b.conflation())
                );
            }
        }
    }

    #[test]
    fn merge_folds_sources_in_both_orders() {
        // 真序 join：任一来源说真就是真；T 与 F 冲突时仍是 T（B 属于知识序的“矛盾”）。
        assert_eq!(merge_truth([B4::T, B4::F]), B4::T);
        assert_eq!(merge_truth([B4::T, B4::N]), B4::T);
        assert_eq!(merge_truth([B4::F, B4::N]), B4::N);
        assert_eq!(merge_truth([]), B4::N);
        // 知识序 meet：只有一致的信息才留下；冲突退化成 N。
        assert_eq!(merge_knowledge([B4::T, B4::F]), B4::N);
        assert_eq!(merge_knowledge([B4::T, B4::N]), B4::N);
        assert_eq!(merge_knowledge([B4::T, B4::B]), B4::T);
        assert_eq!(merge_knowledge([B4::F, B4::B]), B4::F);
        assert_eq!(merge_knowledge([]), B4::N);
    }

    #[test]
    fn unknown_constant_maps_to_neither() {
        assert_eq!(B4::from(Ternary::Unknown), B4::N);
        assert_eq!(eval(&p("X | 1"), &BTreeMap::new()), B4::T);
    }
}
