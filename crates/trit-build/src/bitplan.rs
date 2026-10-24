//! 位并行真值表（M3）：每轮同时算 64 组赋值。
//!
//! 行序与产物约定必须和标量路径一致：**code 以 3 为基，第 i 位是第 i 个变量，
//! 0→False、1→Unknown、2→True**（`trit-core::bits::Evaluator` 用的是 1↔2 对调的另一套，
//! 所以这里不复用它，见 `row_order_is_code_base_three`）。
//!
//! 门级语义**不手写公式**：先从 `Algebra` 的方法里取出该门在 3×3 输入组合上的结果表，
//! 再按最小项（minterm）把结果摊到 pos/neg 位平面上。这样 K3/Ł3/B3 三种语义
//! 各算子（尤其 Ł3 的 t-norm 与蕴含）都不会被手写公式写错。

use std::collections::HashMap;

use anyhow::{Context, Result};
use trit_core::value::Ternary;
use trit_core::{Algebra, Expr};

const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

/// 两根位平面：`pos` 位为 1 = True，`neg` 位为 1 = False，两者都 0 = Unknown。
#[derive(Clone)]
struct Planes {
    pos: Vec<u64>,
    neg: Vec<u64>,
}

/// 表的形状：真实行数与末字掩码。
#[derive(Clone, Copy)]
struct Shape {
    count: usize,
    words: usize,
}

impl Shape {
    fn new(count: usize) -> Self {
        Self {
            count,
            words: count.div_ceil(64),
        }
    }
}

impl Planes {
    fn zeros_words(words: usize) -> Self {
        Self {
            pos: vec![0; words],
            neg: vec![0; words],
        }
    }

    fn zeros(shape: Shape) -> Self {
        Self::zeros_words(shape.words)
    }

    fn constant(value: Ternary, shape: Shape) -> Self {
        let mut planes = Self::zeros(shape);
        let plane = match value {
            Ternary::True => &mut planes.pos,
            Ternary::False => &mut planes.neg,
            Ternary::Unknown => return planes,
        };
        for word in plane.iter_mut() {
            *word = u64::MAX;
        }
        mask_tail(plane, shape.count);
        planes
    }

    /// 第 `slot` 种取值（0=False / 1=Unknown / 2=True）的位平面。
    fn select(&self, slot: usize) -> Vec<u64> {
        match slot {
            0 => self.neg.clone(),
            2 => self.pos.clone(),
            _ => self
                .pos
                .iter()
                .zip(&self.neg)
                .map(|(pos, neg)| !(pos | neg))
                .collect(),
        }
    }
}

/// 末字里越界的位清零，避免 `!` 之类操作把垃圾位带进后续比较。
fn mask_tail(plane: &mut [u64], count: usize) {
    let used = count % 64;
    if used != 0
        && let Some(last) = plane.last_mut()
    {
        *last &= (1u64 << used) - 1;
    }
}

fn var_planes(vars: &[String], shape: Shape) -> Vec<Planes> {
    let mut planes: Vec<Planes> = vars.iter().map(|_| Planes::zeros(shape)).collect();
    let mut digits = vec![0u8; vars.len()];
    for code in 0..shape.count {
        let (word, bit) = (code / 64, 1u64 << (code % 64));
        for (index, digit) in digits.iter().enumerate() {
            match digit {
                0 => planes[index].neg[word] |= bit,
                2 => planes[index].pos[word] |= bit,
                _ => {}
            }
        }
        for digit in digits.iter_mut() {
            if *digit == 2 {
                *digit = 0;
            } else {
                *digit += 1;
                break;
            }
        }
    }
    planes
}

fn gate(alg: Algebra, op: &Expr, left: &Planes, right: &Planes) -> Planes {
    let mut table = [[Ternary::Unknown; 3]; 3];
    for (row, a) in VALUES.iter().enumerate() {
        for (column, b) in VALUES.iter().enumerate() {
            table[row][column] = match op {
                Expr::And(..) => alg.and(*a, *b),
                Expr::Or(..) => alg.or(*a, *b),
                Expr::Xor(..) => alg.strong_add(*a, *b),
                Expr::Implies(..) => alg.implies(*a, *b),
                Expr::Equiv(..) => alg.equiv(*a, *b),
                _ => unreachable!("gate 只收二元算子"),
            };
        }
    }
    let mut out = Planes::zeros_words(left.pos.len());
    let left_sel: [Vec<u64>; 3] = [left.select(0), left.select(1), left.select(2)];
    let right_sel: [Vec<u64>; 3] = [right.select(0), right.select(1), right.select(2)];
    for (row, left_bits) in left_sel.iter().enumerate() {
        for (column, right_bits) in right_sel.iter().enumerate() {
            let target = match table[row][column] {
                Ternary::True => &mut out.pos,
                Ternary::False => &mut out.neg,
                Ternary::Unknown => continue,
            };
            for (index, bits) in left_bits.iter().enumerate() {
                target[index] |= bits & right_bits[index];
            }
        }
    }
    out
}

fn eval(
    expr: &Expr,
    alg: Algebra,
    vars: &HashMap<&str, usize>,
    planes: &[Planes],
    shape: Shape,
) -> Planes {
    match expr {
        Expr::Const(value) => Planes::constant(*value, shape),
        Expr::Var(name) => match vars.get(name.as_str()) {
            Some(index) => planes[*index].clone(),
            None => Planes::zeros(shape),
        },
        Expr::Not(inner) => {
            let inner = eval(inner, alg, vars, planes, shape);
            Planes {
                pos: inner.neg,
                neg: inner.pos,
            }
        }
        Expr::And(..) | Expr::Or(..) | Expr::Xor(..) | Expr::Implies(..) | Expr::Equiv(..) => {
            let (left, right) = match expr {
                Expr::And(a, b)
                | Expr::Or(a, b)
                | Expr::Xor(a, b)
                | Expr::Implies(a, b)
                | Expr::Equiv(a, b) => (a, b),
                _ => unreachable!("已按二元算子筛选"),
            };
            let left = eval(left, alg, vars, planes, shape);
            let right = eval(right, alg, vars, planes, shape);
            gate(alg, expr, &left, &right)
        }
    }
}

/// 位并行求全表；行序与 `enumerate`（标量路径）一致。
pub(crate) fn table(expr: &Expr, vars: &[String], alg: Algebra) -> Result<Vec<Ternary>> {
    let count = 3u64
        .checked_pow(vars.len() as u32)
        .context("变量太多，3^n 溢出")?;
    let count = usize::try_from(count).context("3^n 超出本机地址空间")?;
    let shape = Shape::new(count);
    let planes = var_planes(vars, shape);
    let index: HashMap<&str, usize> = vars
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let result = eval(expr, alg, &index, &planes, shape);
    let mut out = Vec::with_capacity(count);
    for code in 0..count {
        let (word, bit) = (code / 64, 1u64 << (code % 64));
        out.push(if result.pos[word] & bit != 0 {
            Ternary::True
        } else if result.neg[word] & bit != 0 {
            Ternary::False
        } else {
            Ternary::Unknown
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enumerate;
    use trit_core::Parser;

    const ALGEBRAS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

    fn check(source: &str) {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        for alg in ALGEBRAS {
            let fast = table(&expr, &vars, alg).expect("位并行");
            let slow = enumerate(&expr, &vars, alg).expect("标量");
            assert_eq!(fast, slow, "{source} @ {alg:?} 位并行与标量不一致");
        }
    }

    #[test]
    fn row_order_is_code_base_three() {
        let expr = Parser::parse("a").unwrap();
        let vars = expr.variables();
        let values = table(&expr, &vars, Algebra::Kleene).unwrap();
        assert_eq!(
            values,
            vec![Ternary::False, Ternary::Unknown, Ternary::True],
            "code 0/1/2 必须是 F/U/T（与 truth.csv 的 -1/0/1 对齐）"
        );
    }

    #[test]
    fn every_operator_matches_scalar() {
        for source in [
            "!a",
            "a & b",
            "a | b",
            "a ^ b",
            "a => b",
            "a <=> b",
            "(a & b) | !c",
            "(a => b) <=> (!b => !a)",
            "!(a ^ b) & (c | !a)",
            "1",
            "0",
            "a & 1",
        ] {
            check(source);
        }
    }

    #[test]
    fn unknown_heavy_case_matches_scalar() {
        // Ł3 的 t-norm 与蕴含在 Unknown 上最容易写错，这里专门压 U 的组合。
        for source in [
            "a & b & c",
            "(a => b) & (b => a)",
            "a <=> b <=> c",
            "(a ^ b) ^ c",
            "!((a | b) & (c => a))",
        ] {
            check(source);
        }
    }

    /// 算表本身的读数（不含 CSV/IO）：`cargo test --release -p trit-build --bin trit-build -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark，不进默认门禁"]
    fn bench_table_12_vars() {
        let expr = Parser::parse("(v0&v1)|(v2&v3)|(v4&v5)|(v6&v7)|(v8&v9)|(v10&v11)").unwrap();
        let vars = expr.variables();
        assert_eq!(vars.len(), 12);
        let started = std::time::Instant::now();
        let slow = enumerate(&expr, &vars, Algebra::Kleene).unwrap();
        let scalar = started.elapsed();
        let started = std::time::Instant::now();
        let fast = table(&expr, &vars, Algebra::Kleene).unwrap();
        let parallel = started.elapsed();
        eprintln!("12 vars / 531441 rows: scalar {scalar:?} → bit-parallel {parallel:?}");
        assert_eq!(slow, fast);

        // 构建其余阶段的占比：化简 + e-graph 重写 + 等价复核。
        use crate::egraph;
        use trit_eval::simplify::{SimplifyOptions, simplify_with};
        let started = std::time::Instant::now();
        let simplified = simplify_with(&expr, Algebra::Kleene, SimplifyOptions::default());
        let simplify = started.elapsed();
        let started = std::time::Instant::now();
        let (rewritten, rules) = egraph::rewrite(&simplified, Algebra::Kleene, 32);
        let rewrite = started.elapsed();
        let started = std::time::Instant::now();
        let equivalent = trit_eval::eval::equivalent_within(
            &expr,
            &rewritten,
            Algebra::Kleene,
            trit_core::limits::Limits::default(),
        )
        .unwrap();
        let check = started.elapsed();
        assert!(equivalent.equivalent);
        eprintln!(
            "12 vars: simplify {simplify:?} + rewrite {rewrite:?}（规则 {} 条）+ equivalence {check:?}",
            rules.len()
        );

        // 序列化/哈希：算表变快后它们才是大头。
        let started = std::time::Instant::now();
        let csv = crate::truth_csv(&vars, &fast);
        let csv_time = started.elapsed();
        let started = std::time::Instant::now();
        let _lookup = crate::lookup_c(&vars, &fast, Algebra::Kleene);
        let lookup_time = started.elapsed();
        eprintln!(
            "12 vars: truth_csv {csv_time:?}（{} B）+ lookup.c {lookup_time:?}",
            csv.len()
        );
    }

    #[test]
    fn wider_tables_match_scalar() {
        let source = "(v0 & v1) | (v2 ^ v3) | !(v4 => v5)";
        let expr = Parser::parse(source).unwrap();
        let vars = expr.variables();
        assert_eq!(vars.len(), 6);
        for alg in ALGEBRAS {
            assert_eq!(
                table(&expr, &vars, alg).unwrap(),
                enumerate(&expr, &vars, alg).unwrap(),
                "{source} @ {alg:?}"
            );
        }
    }
}
