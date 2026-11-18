//! M3：循环执行——同一 cell 权重重复用，轨迹是真正的串行状态序列。
//!
//! 与 `compile`（展开 `max_len` 步、层数随步数增长）不同，这里每一步都调用同一个
//! `cell`，状态由外部携带。这才是「思考链」的天然载体：轨迹里每个元素都是同一组
//! 权重在一次执行后的状态。

use trit_wspace::{Net, Trit};

use crate::compile;
use crate::dfa::Dfa;

/// 一次循环执行的读数。
pub struct Run {
    /// 每步之后的状态（不含初始状态），长度 = `max_len`。
    pub trace: Vec<Vec<Trit>>,
    pub accept: bool,
}

/// 用 `cell` 在串 `w` 上跑 `max_len` 步。
#[must_use]
pub fn run(cell: &Net, dfa: &Dfa, w: &[Trit], max_len: usize) -> Run {
    let k = dfa.states;
    let mut state = compile::initial_state(k);
    let syms = compile::encode3(w, max_len);
    let mut trace = Vec::with_capacity(max_len);
    for step in 0..max_len {
        let mut input = state.clone();
        input.extend_from_slice(&syms[step * 3..step * 3 + 3]);
        state = cell.forward(&input).expect("cell 维度一致");
        trace.push(state.clone());
    }
    let accept = (0..k).any(|j| dfa.accept[j] && state[j] == Trit::Pos);
    Run { trace, accept }
}
