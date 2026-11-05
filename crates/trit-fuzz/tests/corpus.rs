//! `trit-fuzz` 的语料生成断言：确定性、域封闭、深度上界与三值平衡。

use std::collections::BTreeSet;

use trit_bench::SeedRng;
use trit_core::ast::Expr;
use trit_core::value::Ternary;
use trit_fuzz::corpus::{bipolar, random_expr, random_trits, ternary_value, trit, var_names};

fn depth_of(expr: &Expr) -> usize {
    match expr {
        Expr::Const(_) | Expr::Var(_) => 0,
        Expr::Not(inner) => 1 + depth_of(inner),
        Expr::And(a, b)
        | Expr::Or(a, b)
        | Expr::Xor(a, b)
        | Expr::Implies(a, b)
        | Expr::Equiv(a, b) => 1 + depth_of(a).max(depth_of(b)),
    }
}

#[test]
fn same_seed_replays_the_same_corpus() {
    let names = var_names(4);
    let one: Vec<String> = {
        let mut rng = SeedRng::new(7);
        (0..12)
            .map(|_| format!("{}", random_expr(&mut rng, &names, 3)))
            .collect()
    };
    let two: Vec<String> = {
        let mut rng = SeedRng::new(7);
        (0..12)
            .map(|_| format!("{}", random_expr(&mut rng, &names, 3)))
            .collect()
    };
    assert_eq!(one, two, "同一颗种子必须能逐条重放语料");
    let distinct: BTreeSet<String> = one.iter().cloned().collect();
    assert!(distinct.len() > 1, "语料全是同一条：种子没进生成器");
}

#[test]
fn generated_expressions_stay_inside_the_declared_domain() {
    let names = var_names(3);
    let allowed: BTreeSet<String> = names.iter().cloned().collect();
    let mut rng = SeedRng::new(11);
    for i in 0..300 {
        let depth = i % 5;
        let expr = random_expr(&mut rng, &names, depth);
        for v in expr.variables() {
            assert!(allowed.contains(&v), "第 {i} 棵冒出域外变量 {v}");
            assert!(!v.is_empty(), "空变量名");
        }
        assert!(
            depth_of(&expr) <= depth,
            "第 {i} 棵深度 {} 超过声明的上界 {depth}",
            depth_of(&expr)
        );
    }
}

#[test]
fn depth_zero_only_leaves() {
    let names = var_names(2);
    let mut rng = SeedRng::new(3);
    for _ in 0..200 {
        let expr = random_expr(&mut rng, &names, 0);
        assert!(
            matches!(expr, Expr::Const(_) | Expr::Var(_)),
            "depth=0 应该只出叶子，实际 {expr:?}"
        );
    }
}

#[test]
fn trits_cover_all_three_values_in_proportion() {
    let mut rng = SeedRng::new(5);
    let got = random_trits(&mut rng, 3000);
    let count = |t: Ternary| got.iter().filter(|&&v| v == t).count();
    let (f, tt, u) = (
        count(Ternary::False),
        count(Ternary::True),
        count(Ternary::Unknown),
    );
    assert_eq!(f + tt + u, 3000, "长度对不上");
    for (name, n) in [("False", f), ("True", tt), ("Unknown", u)] {
        assert!(
            n > 800 && n < 1200,
            "{name} 只有 {n} 个，三值分布不像均匀的"
        );
    }
}

#[test]
fn empty_var_table_is_refused_not_silently_degenerate() {
    // 生成器在空变量表上没有合法叶子可选（除了常量），与其悄悄全出常量，不如直接拦。
    let r = std::panic::catch_unwind(|| {
        let mut rng = SeedRng::new(1);
        random_expr(&mut rng, &[], 2)
    });
    assert!(r.is_err(), "空变量表应当 panic，而不是静默产出常量语料");
}

#[test]
fn single_draw_helpers_are_balanced_and_typed() {
    let mut rng = SeedRng::new(77);
    let tri = (0..3000).map(|_| trit(&mut rng)).collect::<Vec<_>>();
    let vals = (0..3000)
        .map(|_| ternary_value(&mut rng))
        .collect::<Vec<_>>();
    let bi = (0..3000).map(|_| bipolar(&mut rng)).collect::<Vec<_>>();

    for t in [Ternary::False, Ternary::True, Ternary::Unknown] {
        let n = tri.iter().filter(|&&v| v == t).count();
        assert!(n > 800 && n < 1200, "{t:?} 只有 {n} 个");
    }
    assert!(
        vals.iter().all(|&v| (-1..=1).contains(&v)),
        "ternary_value 出格"
    );
    let zeros = vals.iter().filter(|&&v| v == 0).count();
    assert!(zeros > 800 && zeros < 1200, "0 档只有 {zeros} 个");
    assert!(bi.iter().all(|&v| v == 1 || v == -1), "bipolar 只能 ±1");
    let plus = bi.iter().filter(|&&v| v == 1).count();
    assert!(plus > 1300 && plus < 1700, "+1 只有 {plus}/3000");
}

#[test]
fn var_names_switches_letter_style_at_26() {
    let names = var_names(28);
    assert_eq!(names[0], "a");
    assert_eq!(names[25], "z");
    assert_eq!(names[26], "v26");
    let uniq: BTreeSet<&String> = names.iter().collect();
    assert_eq!(uniq.len(), 28, "变量名撞车");
}
