//! 验证集与行为：把“网络的语义”变成可逐项比对的对象。
//!
//! M0 只支持**有界穷举域**（二进制 `2^n`、三进制 `3^n`）。这不是妥协：
//! 三值网络的行为在给定输入域上是有限的，穷举就能给出“完全等价”而不是抽样估计。

use crate::error::{Error, Result};
use crate::net::Net;
use crate::trit::Trit;

/// 输入域上限（二进制）。
pub const MAX_BOOLEAN_VARS: usize = 20;
/// 输入域上限（三进制）。
pub const MAX_TERNARY_VARS: usize = 10;

/// 一个网络在某个验证集上的完整输出（按验证集顺序）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Behavior {
    outputs: Vec<Vec<Trit>>,
}

impl Behavior {
    /// 在验证集上求行为。
    pub fn of(net: &Net, oracle: &Oracle) -> Result<Self> {
        Ok(Self {
            outputs: oracle.outputs_of(net)?,
        })
    }

    /// 逐样本输出。
    #[must_use]
    pub fn outputs(&self) -> &[Vec<Trit>] {
        &self.outputs
    }

    /// 64 位行为指纹（报告/去重用；判等请直接用 `==`）。
    #[must_use]
    pub fn signature(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for row in &self.outputs {
            hash = fnv(hash, &(row.len() as u64).to_le_bytes());
            for &value in row {
                hash = fnv(hash, &[value.to_i8() as u8]);
            }
        }
        hash
    }
}

/// 有界验证集：输入向量与期望输出向量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Oracle {
    inputs: Vec<Vec<Trit>>,
    outputs: Vec<Vec<Trit>>,
}

impl Oracle {
    /// 从输入/期望输出构造；两者非空、等长、各自维度一致。
    pub fn new(inputs: Vec<Vec<Trit>>, outputs: Vec<Vec<Trit>>) -> Result<Self> {
        if inputs.is_empty() {
            return Err(Error::EmptyOracle);
        }
        if inputs.len() != outputs.len() {
            return Err(Error::Shape {
                what: "oracle",
                expected: inputs.len(),
                got: outputs.len(),
            });
        }
        let input_dim = inputs[0].len();
        let output_dim = outputs[0].len();
        if input_dim == 0 || output_dim == 0 {
            return Err(Error::EmptySizes);
        }
        for row in &inputs {
            if row.len() != input_dim {
                return Err(Error::Shape {
                    what: "input",
                    expected: input_dim,
                    got: row.len(),
                });
            }
        }
        for row in &outputs {
            if row.len() != output_dim {
                return Err(Error::Shape {
                    what: "output",
                    expected: output_dim,
                    got: row.len(),
                });
            }
        }
        Ok(Self { inputs, outputs })
    }

    /// 二进制域 `{false, true}^n`，输出由 `f` 决定（`true → Pos`、`false → Neg`）。
    ///
    /// 输入按字典序，最后一个变量变化最快。
    pub fn boolean(n: usize, f: impl Fn(&[bool]) -> bool) -> Result<Self> {
        if n == 0 || n > MAX_BOOLEAN_VARS {
            return Err(Error::DomainTooLarge {
                vars: n,
                limit: MAX_BOOLEAN_VARS,
            });
        }
        let mut inputs = Vec::with_capacity(1 << n);
        let mut outputs = Vec::with_capacity(1 << n);
        for mask in 0..(1usize << n) {
            let row: Vec<Trit> = (0..n)
                .map(|j| {
                    if (mask >> (n - 1 - j)) & 1 == 1 {
                        Trit::Pos
                    } else {
                        Trit::Neg
                    }
                })
                .collect();
            let bits: Vec<bool> = row.iter().map(|&value| value == Trit::Pos).collect();
            outputs.push(vec![if f(&bits) { Trit::Pos } else { Trit::Neg }]);
            inputs.push(row);
        }
        Self::new(inputs, outputs)
    }

    /// 二进制域，但输出可以是 `Neg/Zero/Pos`（允许弃权）。
    pub fn boolean_trit(n: usize, f: impl Fn(&[Trit]) -> Trit) -> Result<Self> {
        if n == 0 || n > MAX_BOOLEAN_VARS {
            return Err(Error::DomainTooLarge {
                vars: n,
                limit: MAX_BOOLEAN_VARS,
            });
        }
        let mut inputs = Vec::with_capacity(1 << n);
        let mut outputs = Vec::with_capacity(1 << n);
        for mask in 0..(1usize << n) {
            let row: Vec<Trit> = (0..n)
                .map(|j| {
                    if (mask >> (n - 1 - j)) & 1 == 1 {
                        Trit::Pos
                    } else {
                        Trit::Neg
                    }
                })
                .collect();
            outputs.push(vec![f(&row)]);
            inputs.push(row);
        }
        Self::new(inputs, outputs)
    }

    /// 三值域 `{-1, 0, +1}^n`。
    pub fn ternary(n: usize, f: impl Fn(&[Trit]) -> Trit) -> Result<Self> {
        if n == 0 || n > MAX_TERNARY_VARS {
            return Err(Error::DomainTooLarge {
                vars: n,
                limit: MAX_TERNARY_VARS,
            });
        }
        let count = 3usize.pow(n as u32);
        let mut inputs = Vec::with_capacity(count);
        let mut outputs = Vec::with_capacity(count);
        for index in 0..count {
            let mut rest = index;
            let mut row = vec![Trit::Zero; n];
            for slot in row.iter_mut().rev() {
                *slot = Trit::ALL[rest % 3];
                rest /= 3;
            }
            outputs.push(vec![f(&row)]);
            inputs.push(row);
        }
        Self::new(inputs, outputs)
    }

    /// 从未标注网络自身的行为生成探针集：随机三值输入 + 当前输出。
    ///
    /// `vars` 必须等于 `net.input_dim()`；`samples` 必须非零。同一 seed 同结果。
    /// 期望输出就是网络当前行为，所以 `before_loss` 恒为 0；行消融时看 `changed`。
    pub fn sampled_behavior(net: &Net, vars: usize, samples: usize, seed: u64) -> Result<Self> {
        if vars == 0 || vars > MAX_BOOLEAN_VARS {
            return Err(Error::DomainTooLarge {
                vars,
                limit: MAX_BOOLEAN_VARS,
            });
        }
        if samples == 0 {
            return Err(Error::EmptyOracle);
        }
        if net.input_dim() != vars {
            return Err(Error::Shape {
                what: "input",
                expected: vars,
                got: net.input_dim(),
            });
        }
        let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
        if state == 0 {
            state = 1;
        }
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let mut inputs = Vec::with_capacity(samples);
        for _ in 0..samples {
            let row = (0..vars)
                .map(|_| Trit::ALL[(next() % 3) as usize])
                .collect::<Vec<_>>();
            inputs.push(row);
        }
        let outputs = inputs
            .iter()
            .map(|row| net.forward(row))
            .collect::<Result<Vec<_>>>()?;
        Self::new(inputs, outputs)
    }

    /// 输入向量。
    #[must_use]
    pub fn inputs(&self) -> &[Vec<Trit>] {
        &self.inputs
    }

    /// 期望输出。
    #[must_use]
    pub fn outputs(&self) -> &[Vec<Trit>] {
        &self.outputs
    }

    /// 样本数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// 是否空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// 检查网络与验证集维度是否匹配。
    pub fn check_arch(&self, net: &Net) -> Result<()> {
        let input_dim = self.inputs[0].len();
        let output_dim = self.outputs[0].len();
        if net.input_dim() != input_dim {
            return Err(Error::Shape {
                what: "input",
                expected: input_dim,
                got: net.input_dim(),
            });
        }
        if net.output_dim() != output_dim {
            return Err(Error::Shape {
                what: "output",
                expected: output_dim,
                got: net.output_dim(),
            });
        }
        Ok(())
    }

    /// 网络在整个验证集上的输出。
    pub fn outputs_of(&self, net: &Net) -> Result<Vec<Vec<Trit>>> {
        self.check_arch(net)?;
        self.inputs.iter().map(|row| net.forward(row)).collect()
    }

    /// 失配样本下标（升序）。
    pub fn mismatches(&self, net: &Net) -> Result<Vec<usize>> {
        let actual = self.outputs_of(net)?;
        Ok(actual
            .iter()
            .zip(&self.outputs)
            .enumerate()
            .filter_map(|(index, (got, want))| (got != want).then_some(index))
            .collect())
    }

    /// 失配样本数。
    pub fn loss(&self, net: &Net) -> Result<usize> {
        Ok(self.mismatches(net)?.len())
    }

    /// 是否逐样本完全一致。
    pub fn exact(&self, net: &Net) -> Result<bool> {
        Ok(self.loss(net)? == 0)
    }

    /// 逐样本准确率。
    pub fn accuracy(&self, net: &Net) -> Result<f32> {
        let loss = self.loss(net)?;
        Ok(1.0 - loss as f32 / self.len() as f32)
    }

    /// 把期望输出包装成行为（给 [`crate::Program`] 的后置条件用）。
    #[must_use]
    pub fn behavior(&self) -> Behavior {
        Behavior {
            outputs: self.outputs.clone(),
        }
    }
}

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}
