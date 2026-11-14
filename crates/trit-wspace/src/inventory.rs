//! 神经组件清单：有哪些层、多少个神经元、多少 trit、哪些行是死的。
//!
//! 这是「一读就知道有什么」的静态层：不跑前向，只数结构。

use crate::net::Net;
use crate::trit::Trit;

/// 一层的组件统计。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerInfo {
    /// 层号。
    pub layer: usize,
    /// 神经元数（输出维）。
    pub rows: usize,
    /// 每行 fan-in（输入维）。
    pub cols: usize,
    /// 权重 trit 数（`rows × cols`）。
    pub trits: usize,
    /// 非零偏置数。
    pub bias_nonzero: usize,
    /// 全零行数（权重全零 = 死单元）。
    pub zero_rows: usize,
}

/// 整个网络的组件清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    /// 架构。
    pub arch: Vec<usize>,
    /// 每层统计。
    pub layers: Vec<LayerInfo>,
    /// 神经元总数（所有层的行数之和）。
    pub neurons: usize,
    /// 权重 trit 总数。
    pub trits: usize,
    /// 偏置 trit 总数。
    pub bias: usize,
    /// 死行总数。
    pub zero_rows: usize,
}

/// 数一个网络的结构。
#[must_use]
pub fn inventory(net: &Net) -> Inventory {
    let mut layers = Vec::new();
    let mut neurons = 0usize;
    let mut trits = 0usize;
    let mut bias = 0usize;
    let mut zero_rows = 0usize;
    for (index, layer) in net.layers().iter().enumerate() {
        let rows = layer.rows();
        let cols = layer.cols();
        let dead = (0..rows)
            .filter(|&row| (0..cols).all(|col| layer.trit(row, col) == Some(Trit::Zero)))
            .count();
        let bias_nonzero = layer
            .bias()
            .iter()
            .filter(|trit| **trit != Trit::Zero)
            .count();
        layers.push(LayerInfo {
            layer: index,
            rows,
            cols,
            trits: rows * cols,
            bias_nonzero,
            zero_rows: dead,
        });
        neurons += rows;
        trits += rows * cols;
        bias += layer.bias().len();
        zero_rows += dead;
    }
    Inventory {
        arch: net.arch(),
        layers,
        neurons,
        trits,
        bias,
        zero_rows,
    }
}
