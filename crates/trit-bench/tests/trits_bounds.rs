//! `SeedRng::trits(len)` 的元素上限：按 len 分配前拒绝。
//!
//! `MAX_TRITS = 1<<24`；变异放行后分配 16.7M 个 i32（67 MB），不会崩。

use trit_bench::{MAX_TRITS, SeedRng};

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_len_panics_before_allocating() {
    let mut rng = SeedRng::new(1);
    let _ = rng.trits(MAX_TRITS + 1);
}
