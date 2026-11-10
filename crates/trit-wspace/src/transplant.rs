//! 跨层单元移植：把一个隐藏单元连同它的入边、偏置、scale 和**出边**整体搬到另一个网络。
//!
//! 一个隐藏单元在第 `l` 层是第 `r` 行；它到下一层的连接是第 `l+1` 层的第 `r` 列。
//! 只搬行会留下半截单元（入边换了、出边还是旧的），所以这里两类坐标一起改。
//! 输出层没有下游列，只搬行（权重 + 偏置 + scale）。
//!
//! 产物是普通 [`Patch`]（逐点编辑），可以再走事务、审计日志、三方重放。

use std::collections::BTreeSet;

use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;

/// 把 `source` 第 `layer` 层 `rows` 里的单元移植到 `dest`（两者必须同架构）。
///
/// 只生成与 `dest` 现状不同的编辑；`rows` 为空时返回空补丁。重复行号判错，
/// 避免“搬了两遍”这种静默语义。
pub fn transplant_units(source: &Net, dest: &Net, layer: usize, rows: &[usize]) -> Result<Patch> {
    if source.arch() != dest.arch() {
        return Err(Error::ArchMismatch);
    }
    let src = source.layer(layer).ok_or(Error::OutOfBounds {
        layer,
        row: 0,
        col: 0,
    })?;
    let dst = dest.layer(layer).ok_or(Error::OutOfBounds {
        layer,
        row: 0,
        col: 0,
    })?;
    let mut seen = BTreeSet::new();
    for &row in rows {
        if row >= src.rows() {
            return Err(Error::OutOfBounds { layer, row, col: 0 });
        }
        if !seen.insert(row) {
            return Err(Error::DuplicateRow { layer, row });
        }
    }
    let outgoing = if layer + 1 < dest.depth() {
        let next_src = source.layer(layer + 1).ok_or(Error::OutOfBounds {
            layer: layer + 1,
            row: 0,
            col: 0,
        })?;
        let next_dst = dest.layer(layer + 1).ok_or(Error::OutOfBounds {
            layer: layer + 1,
            row: 0,
            col: 0,
        })?;
        Some((next_src, next_dst))
    } else {
        None
    };

    let mut edits = Vec::new();
    for &row in rows {
        for col in 0..src.cols() {
            let to = src.trit(row, col).expect("row/col in range");
            if dst.trit(row, col) != Some(to) {
                edits.push(Edit::SetTrit {
                    layer,
                    row,
                    col,
                    to,
                });
            }
        }
        let bias = src.bias_trit(row).expect("row in range");
        if dst.bias_trit(row) != Some(bias) {
            edits.push(Edit::SetBias {
                layer,
                row,
                to: bias,
            });
        }
        let scale = src.scale(row).expect("row in range");
        if dst.scale(row).map(f32::to_bits) != Some(scale.to_bits()) {
            edits.push(Edit::SetScale {
                layer,
                row,
                to: scale,
            });
        }
        if let Some((next_src, next_dst)) = outgoing {
            for dest_row in 0..next_src.rows() {
                let to = next_src.trit(dest_row, row).expect("same arch");
                if next_dst.trit(dest_row, row) != Some(to) {
                    edits.push(Edit::SetTrit {
                        layer: layer + 1,
                        row: dest_row,
                        col: row,
                        to,
                    });
                }
            }
        }
    }
    Ok(Patch::new(edits))
}
