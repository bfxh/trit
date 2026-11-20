//! `synthetic_samples(count, seed)` 的样本数上限。
//!
//! `MAX_SAMPLES = 1<<20`；每个样本建一张 4 节点图，先判 count 再 `with_capacity`。

use trit_graph::classifier::{MAX_SAMPLES, synthetic_samples};

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_count_panics_before_allocating() {
    let _ = synthetic_samples(MAX_SAMPLES + 1, 1);
}
