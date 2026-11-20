//! `ternary_weights(rows, cols)` 的形状上限：rows×cols 先判再分配。
//!
//! `MAX_ELEMS = 1<<24`；测试值只超一位，变异放行后分配约 67 MB。

use trit_ops::bench::{MAX_ELEMS, ternary_weights};

#[test]
#[should_panic(expected = "超过上限")]
fn absurd_shape_panics_before_allocating() {
    let _ = ternary_weights(MAX_ELEMS / 2 + 1, 2, 1);
}
