//! 内联锥的 C 表达式展开：用显式栈走后序，避免深链耗尽调用栈。

use super::super::super::code;
use super::super::super::ir::Op;
use super::call;

enum Frame {
    Visit(usize),
    Bin(&'static str),
    Not,
}
/// 展开一个可能已内联的节点；未内联节点取已经发射的 `tN`。
pub(crate) fn expr(
    ops: &[Op],
    names: &[Option<usize>],
    inlined: &[bool],
    index: usize,
    inline_tables: bool,
) -> String {
    let mut work = vec![Frame::Visit(index)];
    let mut out = Vec::new();
    while let Some(frame) = work.pop() {
        match frame {
            Frame::Visit(index) => {
                if !inlined[index] {
                    out.push(format!(
                        "t{}",
                        names[index].expect("未内联的操作数先于父节点发射")
                    ));
                    continue;
                }
                match ops[index] {
                    Op::Const(value) => out.push(code(value).to_string()),
                    Op::Var(slot) => out.push(format!("vars[{slot}]")),
                    Op::Not(inner) => {
                        work.push(Frame::Not);
                        work.push(Frame::Visit(inner));
                    }
                    Op::And(left, right) => push_bin("T_AND", left, right, &mut work),
                    Op::Or(left, right) => push_bin("T_OR", left, right, &mut work),
                    Op::Xor(left, right) => push_bin("T_XOR", left, right, &mut work),
                    Op::Xor2(left, right) => push_bin("T_XOR2", left, right, &mut work),
                    Op::Implies(left, right) => push_bin("T_IMPL", left, right, &mut work),
                    Op::Equiv(left, right) => push_bin("T_EQUIV", left, right, &mut work),
                }
            }
            Frame::Bin(table) => {
                let right = out.pop().expect("二元表达式缺右操作数");
                let left = out.pop().expect("二元表达式缺左操作数");
                out.push(call(table, left, right, inline_tables));
            }
            Frame::Not => {
                let inner = out.pop().expect("取反表达式缺操作数");
                out.push(format!("-({inner})"));
            }
        }
    }
    out.pop().expect("表达式栈为空")
}

fn push_bin(table: &'static str, left: usize, right: usize, work: &mut Vec<Frame>) {
    work.push(Frame::Bin(table));
    work.push(Frame::Visit(right));
    work.push(Frame::Visit(left));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct::ir::Op;
    use trit_core::value::Ternary;

    /// 两万层单用链只走显式栈，不靠递归。
    #[test]
    fn deep_inline_chain_does_not_recurse() {
        let mut ops = vec![Op::Const(Ternary::False)];
        for _ in 0..20_000 {
            let last = ops.len() - 1;
            ops.push(Op::And(last, 0));
        }
        let inlined = vec![true; ops.len()];
        assert!(super::super::plan::targets(&ops).iter().all(|flag| *flag));
        let names = vec![None; ops.len()];
        let source = expr(&ops, &names, &inlined, ops.len() - 1, true);
        assert!(source.starts_with("T_AND[") && source.ends_with(']'));
    }
}
