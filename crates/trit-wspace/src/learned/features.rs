//! 特征表：静态/行为特征（从 `learned.rs` 拆出，控制单文件行数）。

use crate::edit::Edit;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

use super::FEATURE_NAMES;

pub(super) struct FeatureTable<'a> {
    pub(super) net: &'a Net,
    pub(super) layer_inputs: Vec<Vec<Vec<Trit>>>,
    pub(super) layer_pre: Vec<Vec<Vec<i32>>>,
    pub(super) global_scale: f32,
}

impl<'a> FeatureTable<'a> {
    pub(super) fn new(net: &'a Net, oracle: &'a Oracle) -> Result<Self> {
        oracle.check_arch(net)?;
        let depth = net.depth();
        let mut layer_inputs = vec![Vec::with_capacity(oracle.len()); depth];
        let mut layer_pre = vec![Vec::with_capacity(oracle.len()); depth];
        for input in oracle.inputs() {
            let mut current = input.clone();
            for (layer_index, layer) in net.layers().iter().enumerate() {
                layer_inputs[layer_index].push(current.clone());
                let mut pre = Vec::with_capacity(layer.rows());
                let mut next = Vec::with_capacity(layer.rows());
                for row in 0..layer.rows() {
                    let mut acc = i32::from(layer.bias_trit(row).expect("row").to_i8());
                    for (col, value) in current.iter().enumerate().take(layer.cols()) {
                        acc += i32::from(layer.trit(row, col).expect("trit").to_i8())
                            * i32::from(value.to_i8());
                    }
                    pre.push(acc);
                    next.push(sign(acc));
                }
                layer_pre[layer_index].push(pre);
                current = next;
            }
        }
        let scales = net
            .layers()
            .iter()
            .flat_map(|layer| layer.scales().iter().copied())
            .collect::<Vec<_>>();
        let mean = scales.iter().sum::<f32>() / scales.len().max(1) as f32;
        let global_scale = if mean.is_finite() && mean > 0.0 {
            mean
        } else {
            1.0
        };
        Ok(Self {
            net,
            layer_inputs,
            layer_pre,
            global_scale,
        })
    }

    pub(super) fn extract(&self, edit: &Edit) -> Vec<f32> {
        match *edit {
            Edit::SetTrit {
                layer,
                row,
                col,
                to,
            } => self.extract_point(layer, row, Some(col), to),
            Edit::SetBias { layer, row, to } => self.extract_point(layer, row, None, to),
            _ => vec![0.0; FEATURE_NAMES.len()],
        }
    }

    fn extract_point(&self, layer: usize, row: usize, col: Option<usize>, to: Trit) -> Vec<f32> {
        let target = &self.net.layers()[layer];
        let rows = target.rows();
        let cols = target.cols();
        let depth = self.net.depth();
        let current = match col {
            Some(col) => target.trit(row, col).expect("trit"),
            None => target.bias_trit(row).expect("bias"),
        };
        let row_trits = target.row(row).expect("row");
        let nonzero = row_trits.iter().filter(|trit| **trit != Trit::Zero).count();
        let abs_sum = row_trits
            .iter()
            .map(|trit| f32::from(trit.to_i8().abs()))
            .sum::<f32>();
        let scale = target.scale(row).expect("scale");
        let bias = target.bias_trit(row).expect("bias");
        let (activation_mean, activation_active, col_mean, col_nonzero) = match col {
            Some(col) => self.column_features(layer, row, col),
            None => {
                let (mean, active) = self.row_activation(layer, row);
                (mean, active, 0.0, 0.0)
            }
        };
        let col_frac = col.map_or(0.0, |col| col as f32 / cols.saturating_sub(1).max(1) as f32);
        vec![
            layer as f32 / depth.saturating_sub(1).max(1) as f32,
            row as f32 / rows.saturating_sub(1).max(1) as f32,
            col_frac,
            f32::from(to.to_i8()),
            f32::from(current.to_i8()),
            if col.is_none() { 1.0 } else { 0.0 },
            nonzero as f32 / cols.max(1) as f32,
            abs_sum / cols.max(1) as f32,
            scale / self.global_scale,
            f32::from(bias.to_i8()),
            activation_mean,
            activation_active,
            col_mean,
            col_nonzero,
            if to == bias { 1.0 } else { 0.0 },
            if layer + 1 == depth { 1.0 } else { 0.0 },
        ]
    }

    fn row_activation(&self, layer: usize, row: usize) -> (f32, f32) {
        let samples = self.layer_pre[layer].len();
        if samples == 0 {
            return (0.0, 0.0);
        }
        let cols = self.net.layers()[layer].cols().max(1);
        let mut abs_sum = 0.0f64;
        let mut active = 0usize;
        for pre in &self.layer_pre[layer] {
            let value = pre[row];
            abs_sum += f64::from(value.abs()) / cols as f64;
            if value != 0 {
                active += 1;
            }
        }
        (
            (abs_sum / samples as f64) as f32,
            active as f32 / samples as f32,
        )
    }

    fn column_features(&self, layer: usize, row: usize, col: usize) -> (f32, f32, f32, f32) {
        let (activation_mean, activation_active) = self.row_activation(layer, row);
        let samples = self.layer_inputs[layer].len();
        if samples == 0 {
            return (activation_mean, activation_active, 0.0, 0.0);
        }
        let mut sum = 0.0f64;
        let mut nonzero = 0usize;
        for input in &self.layer_inputs[layer] {
            let value = input[col].to_i8();
            sum += f64::from(value);
            if value != 0 {
                nonzero += 1;
            }
        }
        (
            activation_mean,
            activation_active,
            (sum / samples as f64) as f32,
            nonzero as f32 / samples as f32,
        )
    }
}

fn sign(value: i32) -> Trit {
    match value {
        value if value > 0 => Trit::Pos,
        value if value < 0 => Trit::Neg,
        _ => Trit::Zero,
    }
}
