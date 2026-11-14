//! 激活级因果追踪：corrupt 全域后逐单元 restore，量每层/每单元对输出的恢复贡献。
//!
//! 这是 ROME 的 causal tracing 在三值网络上的离散版：
//!
//! 1. 取一个样本的输入 / 期望输出；
//! 2. **corrupt 全域**（所有层的 trits 与 bias 置 0）⇒ 输出彻底错；
//! 3. **逐层 restore**（恢复该层全部权重）⇒ 看输出恢复多少位；
//! 4. **逐单元 restore**（按层序 + 行序累计）⇒ 恢复曲线。
//!
//! 与连续模型不同，这里没有「加噪」：corrupt 是确定性的（全零），restore 是逐位还原，
//! 同一份工件每次都得到同一条曲线，可以逐条复核。

use serde_json::{Value, json};

use crate::error::Result;
use crate::net::{Layer, Net};
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 单层 restore 后的读数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerTrace {
    /// 层号。
    pub layer: usize,
    /// 该层行数（= 可恢复的单元数）。
    pub rows: usize,
    /// 恢复该层后的输出。
    pub restored: Vec<Trit>,
    /// 与期望输出相同的位数。
    pub matches: usize,
}

impl LayerTrace {
    fn to_json(&self) -> Value {
        json!({"layer": self.layer, "rows": self.rows, "matches": self.matches})
    }
}

/// 逐单元 restore 的一步。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreStep {
    /// 层号。
    pub layer: usize,
    /// 行号（隐藏单元）。
    pub row: usize,
    /// 累计恢复到这一步时与期望输出相同的位数。
    pub matches: usize,
}

impl RestoreStep {
    fn to_json(&self) -> Value {
        json!({"layer": self.layer, "row": self.row, "matches": self.matches})
    }
}

/// 一个样本的因果追踪结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CausalTrace {
    /// 样本下标。
    pub sample: usize,
    /// 期望输出（oracle 记录）。
    pub target: Vec<Trit>,
    /// 完整工件的输出。
    pub intact: Vec<Trit>,
    /// 全域 corrupt 后的输出。
    pub corrupted: Vec<Trit>,
    /// 每层单独 restore 的读数。
    pub layers: Vec<LayerTrace>,
    /// 逐单元累计 restore 曲线。
    pub restore_curve: Vec<RestoreStep>,
}

impl CausalTrace {
    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.causal-trace.v1",
            "sample": self.sample,
            "target": self.target.iter().map(|trit| trit.to_i8()).collect::<Vec<_>>(),
            "intact": self.intact.iter().map(|trit| trit.to_i8()).collect::<Vec<_>>(),
            "corrupted": self.corrupted.iter().map(|trit| trit.to_i8()).collect::<Vec<_>>(),
            "layers": self.layers.iter().map(LayerTrace::to_json).collect::<Vec<_>>(),
            "restore_curve": self.restore_curve.iter().map(RestoreStep::to_json).collect::<Vec<_>>(),
        })
    }
}

fn matches(output: &[Trit], target: &[Trit]) -> usize {
    output
        .iter()
        .zip(target)
        .filter(|(left, right)| left == right)
        .count()
}

/// `keep(layer, row, col)` 决定该位置是否保留原值；`col = usize::MAX` 表示 bias。
fn rebuild(net: &Net, keep: impl Fn(usize, usize, usize) -> bool) -> Result<Net> {
    let mut layers = Vec::with_capacity(net.layers().len());
    for (index, layer) in net.layers().iter().enumerate() {
        let (rows, cols) = (layer.rows(), layer.cols());
        let mut trits = vec![0i8; rows * cols];
        for row in 0..rows {
            for col in 0..cols {
                if keep(index, row, col) {
                    trits[row * cols + col] = layer.trit(row, col).map_or(0, Trit::to_i8);
                }
            }
        }
        let mut bias = vec![0i8; rows];
        for row in 0..rows {
            if keep(index, row, usize::MAX) {
                bias[row] = layer.bias().get(row).map_or(0, |trit| trit.to_i8());
            }
        }
        layers.push(Layer::with_scales(
            rows,
            cols,
            &trits,
            &bias,
            layer.scales().to_vec(),
        )?);
    }
    Net::new(layers)
}

/// 对一个样本做因果追踪。
pub fn causal_trace(net: &Net, oracle: &Oracle, sample: usize) -> Result<CausalTrace> {
    let input = oracle
        .inputs()
        .get(sample)
        .ok_or_else(|| crate::error::Error::Wire(format!("样本 {sample} 越界")))?;
    let target = oracle
        .outputs()
        .get(sample)
        .ok_or_else(|| crate::error::Error::Wire(format!("样本 {sample} 越界")))?
        .clone();
    let intact = net.forward(input)?;
    let corrupted_net = rebuild(net, |_, _, _| false)?;
    let corrupted = corrupted_net.forward(input)?;

    let mut layers = Vec::with_capacity(net.layers().len());
    for index in 0..net.layers().len() {
        let restored_net = rebuild(net, |layer, _, _| layer == index)?;
        let restored = restored_net.forward(input)?;
        layers.push(LayerTrace {
            layer: index,
            rows: net.layers()[index].rows(),
            matches: matches(&restored, &target),
            restored,
        });
    }

    let mut restore_curve = Vec::new();
    let mut kept: Vec<(usize, usize)> = Vec::new();
    for (index, layer) in net.layers().iter().enumerate() {
        for row in 0..layer.rows() {
            kept.push((index, row));
            let restored_net = rebuild(net, |l, r, _| kept.contains(&(l, r)))?;
            let output = restored_net.forward(input)?;
            restore_curve.push(RestoreStep {
                layer: index,
                row,
                matches: matches(&output, &target),
            });
        }
    }

    Ok(CausalTrace {
        sample,
        target,
        intact,
        corrupted,
        layers,
        restore_curve,
    })
}
