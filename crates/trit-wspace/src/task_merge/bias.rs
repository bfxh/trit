//! f32 偏置的合并与写回（从 `task_merge.rs` 拆出，控制单文件行数）。

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::net::Net;
use crate::task::TaskVector;

/// 按差分写回逐行 f32 偏置：`None` 表示该行没有；层内任一行缺失 ⇒ 整层清空。
pub(crate) fn apply_bias_f32(net: &mut Net, records: &[(usize, usize, Option<f32>)]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut by_layer: BTreeMap<usize, BTreeMap<usize, Option<f32>>> = BTreeMap::new();
    for (layer, row, value) in records {
        by_layer.entry(*layer).or_default().insert(*row, *value);
    }
    for (layer, rows) in by_layer {
        let current = net
            .layer(layer)
            .ok_or(Error::OutOfBounds {
                layer,
                row: 0,
                col: 0,
            })?
            .clone();
        let mut values = Vec::with_capacity(current.rows());
        let mut complete = true;
        for row in 0..current.rows() {
            let value = match rows.get(&row) {
                Some(value) => *value,
                None => current.bias_f32().get(row).copied(),
            };
            match value {
                Some(value) => values.push(value),
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            values.clear();
        }
        *net.layer_mut(layer) = current.with_bias_f32(values)?;
    }
    Ok(())
}

/// 多任务 f32 偏置：权重大于 0 的任务全部有幅度才取加权平均，任一缺失 ⇒ 该行清空。
pub(super) fn merge_bias_f32(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
) -> Result<Vec<(usize, usize, Option<f32>)>> {
    let mut merged = Vec::new();
    for (layer, target) in base.layers().iter().enumerate() {
        for row in 0..target.rows() {
            let mut seen = false;
            let mut missing = false;
            let mut weighted = 0.0f64;
            let mut total = 0.0f64;
            for (task, weight) in tasks.iter().zip(weights) {
                if *weight == 0.0 {
                    continue;
                }
                seen = true;
                let state = task
                    .bias_f32
                    .iter()
                    .find(|(l, r, _)| *l == layer && *r == row)
                    .map(|(_, _, value)| *value)
                    .unwrap_or_else(|| target.bias_f32().get(row).copied());
                match state {
                    Some(value) => {
                        weighted += f64::from(value) * f64::from(*weight);
                        total += f64::from(*weight);
                    }
                    None => {
                        missing = true;
                        break;
                    }
                }
            }
            if !seen {
                continue;
            }
            let current = target.bias_f32().get(row).copied();
            let next = if missing {
                None
            } else {
                Some((weighted / total) as f32)
            };
            if current.map(f32::to_bits) != next.map(f32::to_bits) {
                merged.push((layer, row, next));
            }
        }
    }
    Ok(merged)
}
