//! 梯度知情排序（从 `learned.rs` 拆出）：STE 反向传播 + `-grad * delta` 打分。

use crate::analyze::edit_key;
use crate::edit::Edit;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

use super::ScoredEdit;
use super::features::FeatureTable;

#[allow(clippy::needless_range_loop)]
/// 梯度知情排序：用 STE 反向传播算 `∂L/∂w`，按 `-grad * (to - current)` 打分。
///
/// 前向与 `Net::readout` 一致：隐藏层 `sign(acc)`，输出层保留 `scale * acc`；
/// 损失是输出层 raw 与目标 trit（-1/0/+1）的平方误差。隐藏层 `sign` 的导数用
/// 直通估计器（STE，恒为 1）。
pub fn gradient_scores(net: &Net, oracle: &Oracle) -> Result<Vec<ScoredEdit>> {
    let table = FeatureTable::new(net, oracle)?;
    let samples = oracle.len();
    if samples == 0 {
        return Ok(Vec::new());
    }
    let depth = net.depth();
    let mut grads: Vec<Vec<Vec<f64>>> = net
        .layers()
        .iter()
        .map(|layer| vec![vec![0.0; layer.cols() + 1]; layer.rows()])
        .collect();
    for sample in 0..samples {
        let mut deltas: Vec<Vec<f64>> = net
            .layers()
            .iter()
            .map(|layer| vec![0.0; layer.rows()])
            .collect();
        let last = depth - 1;
        for row in 0..net.layers()[last].rows() {
            let acc = f64::from(table.layer_pre[last][sample][row]);
            let scale = f64::from(net.layers()[last].scale(row).expect("scale"));
            let raw = scale * acc;
            let target = f64::from(oracle.outputs()[sample][row].to_i8());
            deltas[last][row] = (raw - target) * scale;
        }
        for layer in (0..last).rev() {
            let next = layer + 1;
            for row in 0..net.layers()[layer].rows() {
                let mut sum = 0.0;
                for next_row in 0..net.layers()[next].rows() {
                    let weight = f64::from(
                        net.layers()[next]
                            .trit(next_row, row)
                            .expect("trit")
                            .to_i8(),
                    );
                    sum += deltas[next][next_row] * weight;
                }
                deltas[layer][row] = sum;
            }
        }
        for layer in 0..depth {
            let inputs = &table.layer_inputs[layer][sample];
            for row in 0..net.layers()[layer].rows() {
                let delta = deltas[layer][row];
                for col in 0..net.layers()[layer].cols() {
                    let value = f64::from(inputs[col].to_i8());
                    grads[layer][row][col] += delta * value;
                }
                grads[layer][row][net.layers()[layer].cols()] += delta;
            }
        }
    }
    let mut scored = Vec::new();
    for layer in 0..depth {
        let target = &net.layers()[layer];
        for row in 0..target.rows() {
            for col in 0..target.cols() {
                let current = target.trit(row, col).expect("trit");
                let gradient = (grads[layer][row][col] / samples as f64) as f32;
                for to in Trit::ALL {
                    if to == current {
                        continue;
                    }
                    let delta = f32::from(to.to_i8() - current.to_i8());
                    scored.push(ScoredEdit {
                        edit: Edit::SetTrit {
                            layer,
                            row,
                            col,
                            to,
                        },
                        score: -gradient * delta,
                    });
                }
            }
            let current = target.bias_trit(row).expect("bias");
            let gradient = (grads[layer][row][target.cols()] / samples as f64) as f32;
            for to in Trit::ALL {
                if to == current {
                    continue;
                }
                let delta = f32::from(to.to_i8() - current.to_i8());
                scored.push(ScoredEdit {
                    edit: Edit::SetBias { layer, row, to },
                    score: -gradient * delta,
                });
            }
        }
    }
    scored.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| edit_key(&left.edit).cmp(&edit_key(&right.edit)))
    });
    Ok(scored)
}
