//! 零活跃 trit 水印：改**不影响行为**的 trit 位，嵌一段 bit 串。
//!
//! 位置筛选是精确的：对每个候选位置，分别把它改成 `+1` 与 `-1`，要求 oracle 上的
//! loss **完全不变**（三值没有容差）。只有两个方向都不改变行为的位置才进候选。
//! 嵌入后再次验证整串水印的 loss 与 base 相同——这是「改了但没改坏」的离散证据。
//!
//! 提取只读位置的 trit 符号（`+` = 1，`-` = 0），不需要密钥；**位置表本身就是钥匙**，
//! 换一份位置表就换了一个水印通道。

use serde_json::{Value, json};

use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 一次嵌入的读数。
#[derive(Clone, Debug, PartialEq)]
pub struct WatermarkReport {
    /// 写入的位置（层/行/列）。
    pub positions: Vec<(usize, usize, usize)>,
    /// 写入的 bit（`true` = `+1`，`false` = `-1`）。
    pub bits: Vec<bool>,
    /// base 上的 loss。
    pub base_loss: usize,
    /// 嵌入后的 loss（必须等于 `base_loss`）。
    pub embedded_loss: usize,
    /// 扫描到的候选位置总数。
    pub candidates: usize,
    /// 位置选择用的种子。
    pub seed: u64,
}

impl WatermarkReport {
    /// 水印是否验证通过（嵌入前后 loss 相同）。
    #[must_use]
    pub fn verified(&self) -> bool {
        self.base_loss == self.embedded_loss
    }

    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.watermark.v1",
            "bits": self.bits.len(),
            "positions": self.positions.iter().map(|(layer, row, col)| json!([layer, row, col])).collect::<Vec<_>>(),
            "base_loss": self.base_loss,
            "embedded_loss": self.embedded_loss,
            "verified": self.verified(),
            "candidates": self.candidates,
            "seed": self.seed,
        })
    }
}

fn set_trit(net: &Net, layer: usize, row: usize, col: usize, to: Trit) -> Result<Net> {
    let mut edited = net.clone();
    Patch::new(vec![Edit::SetTrit {
        layer,
        row,
        col,
        to,
    }])
    .apply(&mut edited)?;
    Ok(edited)
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// 找出「两个方向都不改变行为」的零 trit 位置。
///
/// `max_scan` 限制尝试的零位置数（大网络用）；`max` 限制返回的候选数。
pub fn find_positions(
    net: &Net,
    oracle: &Oracle,
    base_loss: usize,
    max: usize,
    max_scan: usize,
) -> Result<Vec<(usize, usize, usize)>> {
    let mut positions = Vec::new();
    let mut scanned = 0usize;
    'outer: for (layer_index, layer) in net.layers().iter().enumerate() {
        for row in 0..layer.rows() {
            for col in 0..layer.cols() {
                if layer.trit(row, col) != Some(Trit::Zero) {
                    continue;
                }
                if scanned >= max_scan {
                    break 'outer;
                }
                scanned += 1;
                let plus = set_trit(net, layer_index, row, col, Trit::Pos)?;
                if oracle.loss(&plus)? != base_loss {
                    continue;
                }
                let minus = set_trit(net, layer_index, row, col, Trit::Neg)?;
                if oracle.loss(&minus)? != base_loss {
                    continue;
                }
                positions.push((layer_index, row, col));
                if positions.len() >= max {
                    break 'outer;
                }
            }
        }
    }
    Ok(positions)
}

/// 把 `message` 嵌进工件；候选不足直接报错（不静默截断）。
pub fn embed(
    net: &Net,
    oracle: &Oracle,
    message: &[bool],
    seed: u64,
    max_scan: usize,
) -> Result<(Net, WatermarkReport)> {
    let base_loss = oracle.loss(net)?;
    if message.is_empty() {
        return Ok((
            net.clone(),
            WatermarkReport {
                positions: Vec::new(),
                bits: Vec::new(),
                base_loss,
                embedded_loss: base_loss,
                candidates: 0,
                seed,
            },
        ));
    }
    let candidates = find_positions(net, oracle, base_loss, message.len(), max_scan)?;
    if candidates.len() < message.len() {
        return Err(Error::Wire(format!(
            "水印容量不足：需要 {} 位，只有 {} 个零活跃位置",
            message.len(),
            candidates.len()
        )));
    }
    let mut state = seed;
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    for index in (1..order.len()).rev() {
        let pick = (splitmix64(&mut state) % (index as u64 + 1)) as usize;
        order.swap(index, pick);
    }
    let chosen: Vec<(usize, usize, usize)> = order
        .into_iter()
        .take(message.len())
        .map(|index| candidates[index])
        .collect();

    let mut edited = net.clone();
    let mut edits = Vec::with_capacity(message.len());
    for (index, (layer, row, col)) in chosen.iter().enumerate() {
        edits.push(Edit::SetTrit {
            layer: *layer,
            row: *row,
            col: *col,
            to: if message[index] { Trit::Pos } else { Trit::Neg },
        });
    }
    Patch::new(edits).apply(&mut edited)?;
    let embedded_loss = oracle.loss(&edited)?;
    Ok((
        edited,
        WatermarkReport {
            positions: chosen,
            bits: message.to_vec(),
            base_loss,
            embedded_loss,
            candidates: candidates.len(),
            seed,
        },
    ))
}

/// 从给定位置读回 bit（`+` = `true`，其余 = `false`）。
#[must_use]
pub fn extract(net: &Net, positions: &[(usize, usize, usize)]) -> Vec<bool> {
    positions
        .iter()
        .filter_map(|(layer, row, col)| {
            net.layers()
                .get(*layer)
                .and_then(|layer| layer.trit(*row, *col))
                .map(|trit| trit == Trit::Pos)
        })
        .collect()
}
