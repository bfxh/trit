//! 语料生成器的元素上限：三个生成函数都按 len 分配。
//!
//! `MAX_GEN = 1<<20`；超一位在变异放行后只会分配 1M 个元素。

use trit_bench::SeedRng;
use trit_fuzz::corpus::{MAX_GEN, random_assignment, random_trits, var_names};

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_len_panics_before_allocating() {
    let mut rng = SeedRng::new(1);
    let _ = random_trits(&mut rng, MAX_GEN + 1);
}

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_nvars_panics_before_allocating() {
    let mut rng = SeedRng::new(1);
    let _ = random_assignment(&mut rng, MAX_GEN + 1);
}

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_var_count_panics() {
    let _ = var_names(MAX_GEN + 1);
}
