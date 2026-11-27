//! 内联锥的 C 表达式展开：用显式栈走后序，避免深链耗尽调用栈。

use super::super::super::code;
use super::super::super::ir::Op;
use super::{Table, Tables, call};

enum Frame {
    Visit(usize),
    Bin(Table),
    Not,
}
/// 展开一个可能已内联的节点；未内联节点取已经发射的 `tN`。
pub(crate) fn expr(
    ops: &[Op],
    names: &[Option<usize>],
    inlined: &[bool],
    index: usize,
    tables: &Tables,
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
                    Op::Var(slot) => out.push(format!("v[{slot}]")),
                    Op::Not(inner) => {
                        work.push(Frame::Not);
                        work.push(Frame::Visit(inner));
                    }
                    Op::And(left, right) => push_bin(tables.and(), left, right, &mut work),
                    Op::Or(left, right) => push_bin(tables.or(), left, right, &mut work),
                    Op::Xor(left, right) => push_bin(tables.xor(), left, right, &mut work),
                    Op::Xor2(left, right) => push_bin(tables.xor2(), left, right, &mut work),
                    Op::Implies(left, right) => push_bin(tables.implies(), left, right, &mut work),
                    Op::Equiv(left, right) => push_bin(tables.equiv(), left, right, &mut work),
                }
            }
            Frame::Bin(table) => {
                let right = out.pop().expect("二元表达式缺右操作数");
                let left = out.pop().expect("二元表达式缺左操作数");
                out.push(call(table, left, right));
            }
            Frame::Not => {
                let inner = out.pop().expect("取反表达式缺操作数");
                out.push(super::super::format::neg(inner));
            }
        }
    }
    out.pop().expect("表达式栈为空")
}

fn push_bin(table: Table, left: usize, right: usize, work: &mut Vec<Frame>) {
    work.push(Frame::Bin(table));
    work.push(Frame::Visit(right));
    work.push(Frame::Visit(left));
}

#[cfg(test)]
mod tests;
