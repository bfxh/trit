//! 直接对 CNF 打随机语料：`dpll::count_cnf` 的计数必须等于对 `2^nvars` 逐条枚举的结果。
//!
//! 这条链和 `crosscheck.rs` 不一样：那里从 `Expr` 出发，先经编码器再造 CNF，
//! 编码器一旦把公式写错就会**同时**骗到两边；这里跳过编码器，直接喂随机子句集，
//! 钉的是 DPLL 本体（规范形整理、单元传播、分量分解、缓存键）。

use trit_bench::SeedRng;
use trit_count::cnf::Cnf;
use trit_count::dpll::count_cnf;
use trit_fuzz::corpus::random_cnf;

/// 朴素枚举：对 `2^nvars` 个赋值逐条检查全部子句。
fn brute(clauses: &[Vec<i32>], nvars: usize) -> u128 {
    (0..1u128 << nvars)
        .filter(|&bits| {
            clauses.iter().all(|clause| {
                clause.iter().any(|&lit| {
                    let idx = lit.unsigned_abs() as usize - 1;
                    let value = (bits >> idx) & 1 == 1;
                    if lit > 0 { value } else { !value }
                })
            })
        })
        .count() as u128
}

#[test]
fn dpll_counts_random_cnfs_exactly() {
    let mut rng = SeedRng::new(0xC0FF_EE00_1234_5678);
    let mut mismatch = 0usize;
    let mut checked = 0usize;
    for nvars in 1..=9usize {
        for len in [1usize, 2, 3, 4] {
            for _ in 0..12 {
                let clauses = random_cnf(&mut rng, nvars, 2 * nvars, len);
                let formula = Cnf { nvars, clauses };
                let got = count_cnf(&formula).count;
                let want = brute(&formula.clauses, formula.nvars);
                checked += 1;
                if got != want {
                    mismatch += 1;
                    eprintln!(
                        "nvars={nvars} len={len} DPLL={got} 枚举={want} 子句={:?}",
                        formula.clauses
                    );
                }
            }
        }
    }
    assert_eq!(
        mismatch, 0,
        "{checked} 个随机 CNF 里有 {mismatch} 个计数不一致"
    );
    assert!(checked >= 400, "语料太少：{checked} 条");
}

/// 两端形状：空子句集 = 全部赋值都是解；含空子句 = 无解。
/// 这两个极端最容易被"顺手加个特判"改坏，单独钉住。
#[test]
fn empty_clause_sets_are_counted_at_their_extremes() {
    let all = Cnf {
        nvars: 5,
        clauses: Vec::new(),
    };
    assert_eq!(count_cnf(&all).count, 32, "空子句集应给 2^5 个解");
    assert_eq!(count_cnf(&all).count, brute(&all.clauses, all.nvars));

    let none = Cnf {
        nvars: 5,
        clauses: vec![vec![], vec![1, -2]],
    };
    assert_eq!(count_cnf(&none).count, 0, "有空子句就没有解");
    assert_eq!(count_cnf(&none).count, brute(&none.clauses, none.nvars));
}
