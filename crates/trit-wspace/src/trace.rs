//! 前向时序：逐层逐行的 pre-activation 与输出。
//!
//! 三值网络的前向是逐层的；这里把每层每行的整数累加值（`acc`）与输出 trit 都留下来，
//! 用来看「信号在哪一层衰减/饱和」——`acc` 接近 0 的行就是裕量最小的行。

use crate::error::{Error, Result};
use crate::net::Net;
use crate::trit::Trit;

/// 一行在某一层的 trace。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowTrace {
    /// 行号。
    pub row: usize,
    /// 整数累加值（pre-activation）。
    pub acc: i32,
    /// 输出。
    pub out: Trit,
}

/// 一层的 trace。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerTrace {
    /// 层号。
    pub layer: usize,
    /// 逐行结果。
    pub rows: Vec<RowTrace>,
    /// 非零输出行数。
    pub nonzero: usize,
}

/// 单个样本的完整前向 trace。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardTrace {
    /// 输入。
    pub input: Vec<Trit>,
    /// 逐层结果。
    pub layers: Vec<LayerTrace>,
    /// 最终输出。
    pub output: Vec<Trit>,
}

/// 跑一次逐层前向。
pub fn trace(net: &Net, input: &[Trit]) -> Result<ForwardTrace> {
    let mut current = input.to_vec();
    let mut layers = Vec::with_capacity(net.layers().len());
    for (index, layer) in net.layers().iter().enumerate() {
        if current.len() != layer.cols() {
            return Err(Error::Shape {
                what: "trace input",
                expected: layer.cols(),
                got: current.len(),
            });
        }
        let mut rows = Vec::with_capacity(layer.rows());
        let mut next = Vec::with_capacity(layer.rows());
        for row in 0..layer.rows() {
            let mut acc = i32::from(layer.bias_trit(row).unwrap_or(Trit::Zero).to_i8());
            for (col, x) in current.iter().enumerate() {
                let weight = layer.trit(row, col).unwrap_or(Trit::Zero).to_i8();
                acc += i32::from(weight) * i32::from(x.to_i8());
            }
            let out = Trit::sign_of(acc);
            rows.push(RowTrace { row, acc, out });
            next.push(out);
        }
        layers.push(LayerTrace {
            layer: index,
            nonzero: next.iter().filter(|trit| **trit != Trit::Zero).count(),
            rows,
        });
        current = next;
    }
    Ok(ForwardTrace {
        input: input.to_vec(),
        layers,
        output: current,
    })
}
