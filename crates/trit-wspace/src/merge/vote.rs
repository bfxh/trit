//! 两个网络的逐点符号投票（含可选 f32 bias 平均）。

use crate::error::{Error, Result};
use crate::net::{Layer, Net};
use crate::trit::Trit;

/// 逐点符号投票：`+1` 记 `+1`、`-1` 记 `-1`、正负抵消记 `0`。
pub fn merge_vote(left: &Net, right: &Net) -> Result<Net> {
    if left.arch() != right.arch() {
        return Err(Error::ArchMismatch);
    }
    let mut layers = Vec::with_capacity(left.depth());
    for (a, b) in left.layers().iter().zip(right.layers()) {
        let trits: Vec<i8> = a
            .trits()
            .iter()
            .zip(b.trits())
            .map(|(&x, &y)| Trit::sign_of(i32::from(x.to_i8()) + i32::from(y.to_i8())).to_i8())
            .collect();
        let bias: Vec<i8> = a
            .bias()
            .iter()
            .zip(b.bias())
            .map(|(&x, &y)| Trit::sign_of(i32::from(x.to_i8()) + i32::from(y.to_i8())).to_i8())
            .collect();
        let scales: Vec<f32> = a
            .scales()
            .iter()
            .zip(b.scales())
            .map(|(&x, &y)| (x + y) * 0.5)
            .collect();
        let bias_f32 = if !a.bias_f32().is_empty() && !b.bias_f32().is_empty() {
            Some(
                a.bias_f32()
                    .iter()
                    .zip(b.bias_f32())
                    .map(|(&x, &y)| (x + y) * 0.5)
                    .collect::<Vec<f32>>(),
            )
        } else {
            None
        };
        let layer = Layer::with_scales(a.rows(), a.cols(), &trits, &bias, scales)?;
        layers.push(match bias_f32 {
            Some(values) => layer.with_bias_f32(values)?,
            None => layer,
        });
    }
    Net::new(layers)
}
