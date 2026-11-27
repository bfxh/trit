//! Verilog 组合网表 → 线性 IR（`direct.c` 的第二条 lowering 入口）。
//!
//! 与 `.tri` 表达式链路共用同一套 `Op`、CSE 后的渲染器与 `Algebra` 现算表。
//! 这里只做三件事：
//! 1. 从选中输出反向 DFS，只降它的扇入锥（其它输出与不可达门不进 IR）；
//! 2. 同名信号只降一次（网表扇出 → 一个临时变量）；
//! 3. 把 `trit-circuit` 的门型落到已有算子：`NAND/NOR` 落 `Not(And/Or)`，
//!    `BUF` 直接别名到输入，`XOR2` 单独成 `Xor2`（语义按 `GateKind::eval` 的表）。
//!
//! 不做的事：不重新做语法/预算校验（`trit-circuit` 已做），不做综合与门级优化。

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use trit_circuit::{Circuit, Gate, GateKind};
use trit_core::value::Ternary;

use super::ir::Op;

/// 从 `output` 反向降级，返回后序线性 IR（根在最后一条）。
pub(super) fn lower(circuit: &Circuit, output: &str, vars: &[String]) -> Result<Vec<Op>> {
    let mut drivers: HashMap<&str, &Gate> = HashMap::new();
    for gate in &circuit.gates {
        let name = gate.output.name();
        if vars.iter().any(|var| var == name) {
            bail!("输入 {name:?} 同时被门驱动，网表不合法");
        }
        if drivers.insert(name, gate).is_some() {
            bail!("信号 {name:?} 有多个驱动，网表不合法");
        }
    }
    let mut builder = Builder {
        drivers,
        vars,
        ops: Vec::new(),
        memo: HashMap::new(),
        visiting: HashSet::new(),
    };
    let root = builder.signal(output)?;
    let ops = builder.ops;
    if ops.is_empty() {
        bail!("输出 {output:?} 没有可降级的节点");
    }
    Ok(super::lower::prune_dead(ops, root))
}

struct Builder<'a> {
    drivers: HashMap<&'a str, &'a Gate>,
    vars: &'a [String],
    ops: Vec<Op>,
    memo: HashMap<&'a str, usize>,
    visiting: HashSet<&'a str>,
}

impl<'a> Builder<'a> {
    fn signal(&mut self, name: &'a str) -> Result<usize> {
        if let Some(index) = self.memo.get(name) {
            return Ok(*index);
        }
        if let Some(slot) = self.vars.iter().position(|var| var == name) {
            let index = self.push(Op::Var(slot));
            self.memo.insert(name, index);
            return Ok(index);
        }
        let gate = *self
            .drivers
            .get(name)
            .with_context(|| format!("信号 {name:?} 既不是输入也没有驱动门"))?;
        if !self.visiting.insert(name) {
            bail!("组合环：信号 {name:?} 回到自己");
        }
        let index = self.gate(gate)?;
        self.visiting.remove(name);
        self.memo.insert(name, index);
        Ok(index)
    }

    fn gate(&mut self, gate: &'a Gate) -> Result<usize> {
        let index = match gate.kind {
            GateKind::Const => {
                let value = gate.const_value.unwrap_or(Ternary::False);
                self.push(Op::Const(value))
            }
            GateKind::Buf => {
                let input = self.input(gate, 0)?;
                self.signal(input)?
            }
            GateKind::Not => {
                let inner = self.input(gate, 0)?;
                let inner = self.signal(inner)?;
                self.push(Op::Not(inner))
            }
            GateKind::And => {
                let (left, right) = self.binary_inputs(gate)?;
                self.push(Op::And(left, right))
            }
            GateKind::Or => {
                let (left, right) = self.binary_inputs(gate)?;
                self.push(Op::Or(left, right))
            }
            GateKind::Xor => {
                let (left, right) = self.binary_inputs(gate)?;
                self.push(Op::Xor(left, right))
            }
            GateKind::XorClassic => {
                let (left, right) = self.binary_inputs(gate)?;
                self.push(Op::Xor2(left, right))
            }
            GateKind::Nand => {
                let (left, right) = self.binary_inputs(gate)?;
                let and = self.push(Op::And(left, right));
                self.push(Op::Not(and))
            }
            GateKind::Nor => {
                let (left, right) = self.binary_inputs(gate)?;
                let or = self.push(Op::Or(left, right));
                self.push(Op::Not(or))
            }
        };
        Ok(index)
    }

    fn input(&self, gate: &'a Gate, slot: usize) -> Result<&'a str> {
        gate.inputs
            .get(slot)
            .map(|signal| signal.name())
            .with_context(|| {
                format!(
                    "门 {} 缺少第 {slot} 个输入（网表不合法）",
                    gate.output.name()
                )
            })
    }

    fn binary_inputs(&mut self, gate: &'a Gate) -> Result<(usize, usize)> {
        let left = self.input(gate, 0)?;
        let right = self.input(gate, 1)?;
        Ok((self.signal(left)?, self.signal(right)?))
    }

    fn push(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }
}

#[cfg(test)]
mod tests;
