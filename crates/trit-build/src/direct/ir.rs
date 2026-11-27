//! 直算后端的线性 IR 指令定义；表达式链路（`lower.rs`）与网表链路（`netlist.rs`）
//! 共用，指令里的下标都是「临时变量编号」，按后序排列，根在最后一条。

mod remap;

use trit_core::value::Ternary;

/// 线性 IR 的一条指令：结果是第 `index` 个临时变量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Const(Ternary),
    Var(usize),
    Not(usize),
    And(usize, usize),
    Or(usize, usize),
    Xor(usize, usize),
    /// 经典异或（网表 `XOR2` 门）：任一输入 `X` 则 `X`，否则布尔异或。
    Xor2(usize, usize),
    Implies(usize, usize),
    Equiv(usize, usize),
}
