//! 组合电路等价判定（穷举）。
//!
//! 输入数 ≤ [`MAX_EXHAUSTIVE_INPUTS`] 时**穷举**所有输入组合，逐个比较主输出——
//! 这是完全等价，不是抽样。
//!
//! 为什么先做穷举而不是 SAT：`sat_cnf` 里的 `Enc` / `encode_gate` 是为 ATPG
//! （good/faulty 两份 + 差异约束）设计的私有结构，做独立的 miter 需要先开放可见性
//! 再写一份编码，属于另一个工程；而 M0 规模的电路输入很少，穷举本身就是完备判据。
//! SAT 版留给「输入域大到不能穷举」的场景，届时再单独立项。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::{Circuit, CircuitError, propagate};

/// 穷举上限（输入数）。`2^16 = 65536` 次传播，单测里仍是毫秒级。
pub const MAX_EXHAUSTIVE_INPUTS: usize = 16;

/// 等价判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Equiv {
    /// 所有输入组合下主输出都相同。
    Equivalent,
    /// 找到一组使输出不同的输入（按 `a` 的输入名给出 0/1）。
    Different(BTreeMap<String, bool>),
}

/// 判定错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EquivError {
    /// 两个电路的主输入数量不同。
    #[error("主输入数量不同：{0} vs {1}")]
    InputCount(usize, usize),
    /// 两个电路的主输出数量不同。
    #[error("主输出数量不同：{0} vs {1}")]
    OutputCount(usize, usize),
    /// 输入数超过穷举上限。
    #[error("输入数 {0} 超过穷举上限 {MAX_EXHAUSTIVE_INPUTS}")]
    TooManyInputs(usize),
    /// 传播失败（电路不合法等）。
    #[error(transparent)]
    Propagate(#[from] CircuitError),
}

/// 穷举判定两个组合电路是否等价。
///
/// 输入按**位置**对应（名字可以不同）；输出也按位置对应。
pub fn equivalent(a: &Circuit, b: &Circuit) -> Result<Equiv, EquivError> {
    if a.inputs.len() != b.inputs.len() {
        return Err(EquivError::InputCount(a.inputs.len(), b.inputs.len()));
    }
    if a.outputs.len() != b.outputs.len() {
        return Err(EquivError::OutputCount(a.outputs.len(), b.outputs.len()));
    }
    let n = a.inputs.len();
    if n > MAX_EXHAUSTIVE_INPUTS {
        return Err(EquivError::TooManyInputs(n));
    }

    for mask in 0u32..(1u32 << n) {
        let mut assign_a = BTreeMap::new();
        let mut assign_b = BTreeMap::new();
        let mut example = BTreeMap::new();
        for (i, sig) in a.inputs.iter().enumerate() {
            let value = if (mask >> i) & 1 == 1 {
                Ternary::True
            } else {
                Ternary::False
            };
            assign_a.insert(sig.name().to_string(), value);
            example.insert(sig.name().to_string(), value == Ternary::True);
            if let Some(sig_b) = b.inputs.get(i) {
                assign_b.insert(sig_b.name().to_string(), value);
            }
        }

        let pa = propagate(a, &assign_a, Algebra::Kleene)?;
        let pb = propagate(b, &assign_b, Algebra::Kleene)?;
        for (i, out_a) in a.outputs.iter().enumerate() {
            let out_b = &b.outputs[i];
            let va = pa.outputs.get(out_a.name()).copied();
            let vb = pb.outputs.get(out_b.name()).copied();
            if va != vb {
                return Ok(Equiv::Different(example));
            }
        }
    }
    Ok(Equiv::Equivalent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Gate, GateKind, Signal};

    /// 一个半加器：`sum = a ⊕ b`（用 AIG 展开）、`carry = a ∧ b`。
    fn half_adder(name: &str, rename: bool) -> Circuit {
        let mut c = Circuit::new(name);
        let (a, b) = if rename {
            (Signal::new("x"), Signal::new("y"))
        } else {
            (Signal::new("a"), Signal::new("b"))
        };
        c.add_input(a.clone());
        c.add_input(b.clone());
        // AND 门：carry = a ∧ b
        c.gates.push(Gate::new(
            GateKind::And,
            vec![a.clone(), b.clone()],
            Signal::new("carry"),
        ));
        // XOR：¬(¬(a∧¬b) ∧ ¬(¬a∧b))
        c.gates.push(Gate::new(
            GateKind::And,
            vec![a.clone(), b.clone()],
            Signal::new("and_ab"),
        ));
        c.gates.push(Gate::new(
            GateKind::Not,
            vec![Signal::new("and_ab")],
            Signal::new("nand_ab"),
        ));
        c.add_output(Signal::new("carry"));
        c.add_output(Signal::new("nand_ab"));
        c
    }

    #[test]
    fn identical_circuits_are_equivalent() {
        let a = half_adder("a", false);
        let b = half_adder("b", false);
        assert_eq!(equivalent(&a, &b).unwrap(), Equiv::Equivalent);
    }

    #[test]
    fn renamed_inputs_are_equivalent_by_position() {
        let a = half_adder("a", false);
        let b = half_adder("b", true);
        assert_eq!(equivalent(&a, &b).unwrap(), Equiv::Equivalent);
    }

    #[test]
    fn different_output_count_is_rejected() {
        let mut a = half_adder("a", false);
        let b = half_adder("b", false);
        a.outputs.pop();
        assert_eq!(
            equivalent(&a, &b).unwrap_err(),
            EquivError::OutputCount(1, 2)
        );
    }

    #[test]
    fn difference_returns_counterexample() {
        let a = half_adder("a", false);
        let mut b = half_adder("b", false);
        // 把 b 的第一个输出改成恒假。
        b.gates
            .push(Gate::constant(Ternary::False, Signal::new("carry_fixed")));
        b.outputs[0] = Signal::new("carry_fixed");
        match equivalent(&a, &b).unwrap() {
            Equiv::Different(example) => {
                assert_eq!(example.len(), 2, "两个输入都要给出取值");
            }
            Equiv::Equivalent => panic!("改了一个输出仍然等价，判定器有问题"),
        }
    }

    #[test]
    fn too_many_inputs_is_rejected() {
        let mut a = Circuit::new("big");
        for i in 0..=MAX_EXHAUSTIVE_INPUTS {
            a.add_input(Signal::new(format!("i{i}")));
        }
        a.add_output(Signal::new("o"));
        let b = a.clone();
        assert_eq!(
            equivalent(&a, &b).unwrap_err(),
            EquivError::TooManyInputs(MAX_EXHAUSTIVE_INPUTS + 1)
        );
    }
}
