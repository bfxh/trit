//! 混淆矩阵与预测窗的上限：`classes²` / `forecast(h)` 都在分配前拒绝。
//!
//! `MAX_CLASSES = 4096`、`MAX_HORIZON = 1<<20`；两个测试的越界值都只超一位。

use trit_learn::forecast::{Ewma, MAX_HORIZON};
use trit_learn::metrics::{Confusion, MAX_CLASSES};

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_classes_panics_before_allocating() {
    let _ = Confusion::new(MAX_CLASSES + 1);
}

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_horizon_panics_before_allocating() {
    let _ = Ewma::new(0.5).forecast(MAX_HORIZON + 1);
}
