//! TTIR → C 直算后端（`--backend direct|both` 产出 `direct.c`）。
//!
//! 和 `lookup.c` 是同一条边界上的两种产物：
//! - `lookup.c` 把 3^n 真值表整块塞进 C，运行时一次索引；
//! - `direct.c` 把表达式或 Verilog 组合网表**降成线性 IR**（每个节点一条 `C tN=...;`），
//!   常量折叠 / 恒等式化简（含格吸收链、公因子提取、德摩根链、常量否定、Xor2 结合链与 B3 补元/毒值吸收）后再做 CSE，出口过一遍死代码消除；运行时按节点顺序算，不展开 3^n 表。取反直接写一元负号 `-(...)`；单用节点在渲染时沿单用边并进父语句（不再封深度），扇出即停。二元调用一律直接展开 `T[3*l+r+4]` 表索引；紧凑索引恒比 `trit_bin` helper 调用短，helper 已删除。
//!
//! 语义不靠手写 C 算子保证：取反用 `-x` 是因为三种代数都有 `¬F=T / ¬X=X / ¬T=F`，
//! 六个二元 3×3 表由 [`Algebra`] 现算，测试再拿 `trit_eval::eval` 逐赋值对拍。
//! 生成物仍需 C 工具链编译 —— 这与
//! `docs/reference/TERNARY-TOOLCHAIN.md` §7「不做二进制后端」一致。
//!
//! 函数签名与 `lookup.c` 的 `trit_lookup` 平行：
//! `C f(C*v,int n)`（`C` 即 `signed char`）。
//! 入参不足（`n < 变量数`）一律返回 `0`（X），fail-closed，不读越界内存。

use anyhow::Result;
use trit_circuit::{Circuit, GateKind};
use trit_core::{Algebra, Expr, value::Ternary};

/// 三值枚举顺序 = C 侧 `-1/0/1` 的顺序，也是表的下标顺序。
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

mod emit;
mod fold;
mod ir;
mod lower;
mod netlist;
mod verify;
use lower::lower;
pub(crate) use verify::verify;

fn code(value: Ternary) -> i8 {
    match value {
        Ternary::False => -1,
        Ternary::Unknown => 0,
        Ternary::True => 1,
    }
}

fn binary_table(alg: Algebra, function: fn(Algebra, Ternary, Ternary) -> Ternary) -> [i8; 9] {
    let mut table = [0i8; 9];
    for (row, left) in VALUES.iter().enumerate() {
        for (column, right) in VALUES.iter().enumerate() {
            table[row * 3 + column] = code(function(alg, *left, *right));
        }
    }
    table
}

/// 网表门型的 3×3 表：直接用 `trit-circuit` 的 `GateKind::eval`，与仿真同一口径。
fn gate_table(alg: Algebra, kind: GateKind) -> [i8; 9] {
    let mut table = [0i8; 9];
    for (row, left) in VALUES.iter().enumerate() {
        for (column, right) in VALUES.iter().enumerate() {
            table[row * 3 + column] = code(kind.eval(&[*left, *right], alg));
        }
    }
    table
}

/// 表达式 → `direct.c` 全文。
pub(crate) fn emit(expr: &Expr, vars: &[String], alg: Algebra) -> Result<String> {
    let ops = fold::fold(lower(expr, vars)?, alg);
    Ok(emit::render(ops, vars, alg))
}

/// Verilog 组合网表 → `direct.c` 全文（只降选中输出的扇入锥）。
pub(crate) fn emit_netlist(
    circuit: &Circuit,
    output: &str,
    vars: &[String],
    alg: Algebra,
) -> Result<String> {
    let ops = fold::fold(netlist::lower(circuit, output, vars)?, alg);
    Ok(emit::render(ops, vars, alg))
}

#[cfg(test)]
mod tests;
