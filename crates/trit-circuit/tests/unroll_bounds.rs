//! `SequentialCircuit::unroll(k)` 的展开步数上限：按 k 复制状态与门前拒绝。
//!
//! `MAX_UNROLL = 4096`；测试用一个 1 锁存器的最小电路，只验证越界分支。

use trit_circuit::sequential::{
    Latch, Literal, MAX_UNROLL, SequentialCircuit, SequentialError,
};

fn tiny() -> SequentialCircuit {
    SequentialCircuit {
        inputs: 0,
        latches: vec![Latch {
            init: false,
            next: Literal::var(1),
        }],
        outputs: vec![Literal::var(1)],
        ands: vec![],
    }
}

#[test]
fn absurd_unroll_is_rejected_before_building() {
    let err = tiny().unroll(MAX_UNROLL + 1).expect_err("越界必须 Err");
    assert!(matches!(err, SequentialError::TooManySteps(_, _)));
}
