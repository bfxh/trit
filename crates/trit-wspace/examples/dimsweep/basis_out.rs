//! 把降维基导出成 WSP 工件（`dimsweep --out-basis`）。
//!
//! 导出后可以直接喂给 `trit-wspace project --basis`，于是「实验导出基 → 命令行压模型 →
//!  certify 验证」连成一条链。基就是一层三值权重，不需要新格式。

use std::fs;

use trit_wspace::{Layer, Net, to_wsp};

use crate::basis::trit_row_i8;

/// `comps` 是 `k × dim` 的 f64 基向量；逐行 absmean 三值化后写成单层 WSP。
pub fn export(comps: &[Vec<f64>], dim: usize, path: &str) -> Result<(), String> {
    let mut trits = Vec::with_capacity(comps.len() * dim);
    for row in comps {
        let values: Vec<f32> = row.iter().map(|value| *value as f32).collect();
        trits.extend(trit_row_i8(&values));
    }
    let layer = Layer::new(comps.len(), dim, &trits, &vec![0; comps.len()])
        .map_err(|error| format!("basis layer: {error}"))?;
    let net = Net::new(vec![layer]).map_err(|error| format!("basis net: {error}"))?;
    fs::write(path, to_wsp(&net)).map_err(|error| format!("{path}: {error}"))
}
