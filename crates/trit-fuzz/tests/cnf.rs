//! CNF 语料的形状断言：DIMACS 口径、无重字、无永真、无空子句，以及退化输入。

use std::collections::BTreeSet;

use trit_bench::SeedRng;
use trit_fuzz::corpus::{random_assignment, random_cnf};

#[test]
fn cnf_clauses_are_dimacs_shaped_and_never_degenerate() {
    let mut rng = SeedRng::new(31);
    for (nvars, len) in [(1usize, 3usize), (2, 3), (3, 4), (8, 3), (30, 5)] {
        for clause in random_cnf(&mut rng, nvars, 40, len) {
            assert!(
                !clause.is_empty(),
                "nvars={nvars} 冒出空子句（会把公式直接判死）"
            );
            assert!(
                clause.len() <= len,
                "nvars={nvars} 子句长 {} 超过上界 {len}",
                clause.len()
            );
            let mut seen = BTreeSet::new();
            for &lit in &clause {
                assert!(lit != 0, "文字不能是 0");
                let var = lit.unsigned_abs();
                assert!(
                    var >= 1 && var <= nvars as u32,
                    "文字 {lit} 越界（nvars={nvars}）"
                );
                assert!(seen.insert(var), "同一子句里 {var} 出现两次（重字或永真）");
            }
        }
    }
}

#[test]
fn cnf_replays_from_the_same_seed() {
    let a: Vec<Vec<i32>> = {
        let mut rng = SeedRng::new(101);
        random_cnf(&mut rng, 6, 25, 3)
    };
    let b = {
        let mut rng = SeedRng::new(101);
        random_cnf(&mut rng, 6, 25, 3)
    };
    assert_eq!(a, b, "同种子必须给同一份 CNF");
    let distinct: BTreeSet<Vec<i32>> = a.iter().cloned().collect();
    assert!(distinct.len() > 1, "25 条子句全同 ⇒ 生成器没吃随机数");
}

#[test]
fn degenerate_inputs_return_an_empty_formula_not_a_hidden_unit() {
    let mut rng = SeedRng::new(7);
    assert!(
        random_cnf(&mut rng, 0, 10, 3).is_empty(),
        "nvars=0 该给空表"
    );
    assert!(
        random_cnf(&mut rng, 5, 10, 0).is_empty(),
        "clause_len=0 该给空表"
    );
    assert_eq!(
        random_cnf(&mut rng, 5, 0, 3).len(),
        0,
        "nclauses=0 该给空表"
    );
    assert!(
        random_assignment(&mut rng, 0).is_empty(),
        "0 个变量该给空赋值"
    );
}

#[test]
fn assignments_are_balanced_over_many_draws() {
    let mut rng = SeedRng::new(13);
    let rows = 2000;
    let ones: usize = (0..rows)
        .flat_map(|_| random_assignment(&mut rng, 1))
        .filter(|&v| v)
        .count();
    assert!(
        ones > 800 && ones < 1200,
        "{rows} 次里 true 只有 {ones}，1 位分配不像公平的"
    );
}
