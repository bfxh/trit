//! 计算量/延迟雷：逐样本乘法次数与热点层。
//!
//! 三值网络每层的乘法数 = 非零输出行数 × 该层 `cols`（零输出行可跳过）。
//! 逐样本统计后，乘法最多的样本就是「延迟雷」；逐层汇总就是热点层。

use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 一个样本在某一层的计算量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerCost {
    /// 层号。
    pub layer: usize,
    /// 输入维。
    pub cols: usize,
    /// 非零输出行数。
    pub nonzero: usize,
    /// 乘法次数（`nonzero × cols`）。
    pub multiplies: usize,
}

/// 一个样本的总计算量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleCost {
    /// 样本下标。
    pub sample: usize,
    /// 总乘法次数。
    pub multiplies: usize,
    /// 逐层拆分。
    pub per_layer: Vec<LayerCost>,
}

/// 计算量报告。
#[derive(Clone, Debug, PartialEq)]
pub struct CostReport {
    /// 样本数。
    pub samples: usize,
    /// 全部样本的总乘法次数。
    pub total: usize,
    /// 每样本平均乘法次数。
    pub mean: f64,
    /// 最大样本乘法次数（延迟雷）。
    pub max: usize,
    /// 最小样本乘法次数。
    pub min: usize,
    /// 逐层总乘法次数。
    pub per_layer: Vec<usize>,
    /// 乘法最多的样本（最多 `top` 条）。
    pub top: Vec<SampleCost>,
}

/// 逐样本统计计算量。
pub fn cost(net: &Net, oracle: &Oracle, top: usize) -> Result<CostReport> {
    oracle.check_arch(net)?;
    let mut per_layer = vec![0usize; net.layers().len()];
    let mut samples = Vec::with_capacity(oracle.len());
    for (sample, input) in oracle.inputs().iter().enumerate() {
        let mut current = input.clone();
        let mut total = 0usize;
        let mut layers = Vec::with_capacity(net.layers().len());
        for (index, layer) in net.layers().iter().enumerate() {
            let mut next = Vec::with_capacity(layer.rows());
            let mut nonzero = 0usize;
            for row in 0..layer.rows() {
                let mut acc = i32::from(layer.bias_trit(row).unwrap_or(Trit::Zero).to_i8());
                for (col, x) in current.iter().enumerate() {
                    let weight = layer.trit(row, col).unwrap_or(Trit::Zero).to_i8();
                    acc += i32::from(weight) * i32::from(x.to_i8());
                }
                let out = Trit::sign_of(acc);
                if out != Trit::Zero {
                    nonzero += 1;
                }
                next.push(out);
            }
            let multiplies = nonzero * layer.cols();
            total += multiplies;
            per_layer[index] += multiplies;
            layers.push(LayerCost {
                layer: index,
                cols: layer.cols(),
                nonzero,
                multiplies,
            });
            current = next;
        }
        samples.push(SampleCost {
            sample,
            multiplies: total,
            per_layer: layers,
        });
    }
    let total: usize = samples.iter().map(|sample| sample.multiplies).sum();
    let max = samples
        .iter()
        .map(|sample| sample.multiplies)
        .max()
        .unwrap_or(0);
    let min = samples
        .iter()
        .map(|sample| sample.multiplies)
        .min()
        .unwrap_or(0);
    let mean = if samples.is_empty() {
        0.0
    } else {
        total as f64 / samples.len() as f64
    };
    samples.sort_by_key(|sample| std::cmp::Reverse(sample.multiplies));
    if top > 0 {
        samples.truncate(top);
    }
    Ok(CostReport {
        samples: oracle.len(),
        total,
        mean,
        max,
        min,
        per_layer,
        top: samples,
    })
}
