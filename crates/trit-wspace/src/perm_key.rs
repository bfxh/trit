//! 置换密钥指纹与对齐攻击自测。
//!
//! 隐藏单元置换不改行为（交换第 `l` 层两行 + 第 `l+1` 层对应两列），所以它可以当**密钥**：
//! 同一份权重按密钥排列打乱后，行为指纹完全不变，只有知道排列的人能按原位读出。
//!
//! 本模块同时量「攻击者不知道排列，只用 `align` 贪心对齐能恢复多少」——这是置换水印的
//! 真实边界：如果贪心对齐能完全恢复，置换密钥就**不是**安全边界，读数要如实写出来。

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;

/// 一份置换密钥：每层一个排列（最后一层不参与置换）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermKey {
    /// `layers[l][new_row] = old_row`。
    pub layers: Vec<Vec<usize>>,
}

impl PermKey {
    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.perm-key.v1",
            "layers": self.layers,
        })
    }
}

/// 攻击自测结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackReport {
    /// 网络层数。
    pub layers: usize,
    /// 三值总数（含偏置以外的权重 trit）。
    pub trits: usize,
    /// `align` 对齐后与 base 逐 trit 相同的数量。
    pub aligned_matches: usize,
    /// 是否完全恢复（贪心对齐成功）。
    pub attack_success: bool,
    /// 置换后行为是否与 base 相同（置换不应改变行为）。
    pub behavior_identical: bool,
}

impl AttackReport {
    /// 匹配率（0..=1）。
    #[must_use]
    pub fn match_rate(&self) -> f64 {
        if self.trits == 0 {
            1.0
        } else {
            self.aligned_matches as f64 / self.trits as f64
        }
    }

    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.perm-attack.v1",
            "layers": self.layers,
            "trits": self.trits,
            "aligned_matches": self.aligned_matches,
            "match_rate": self.match_rate(),
            "attack_success": self.attack_success,
            "behavior_identical": self.behavior_identical,
        })
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// 恒等密钥（不置换）。
#[must_use]
pub fn identity(net: &Net) -> PermKey {
    let layers = net
        .layers()
        .iter()
        .take(net.layers().len().saturating_sub(1))
        .map(|layer| (0..layer.rows()).collect())
        .collect();
    PermKey { layers }
}

/// 由种子生成置换密钥。
#[must_use]
pub fn from_seed(net: &Net, seed: u64) -> PermKey {
    let mut state = seed;
    let mut layers = Vec::new();
    for layer in net
        .layers()
        .iter()
        .take(net.layers().len().saturating_sub(1))
    {
        let rows = layer.rows();
        let mut perm: Vec<usize> = (0..rows).collect();
        for slot in (1..rows).rev() {
            let pick = (splitmix64(&mut state) % (slot as u64 + 1)) as usize;
            perm.swap(slot, pick);
        }
        layers.push(perm);
    }
    PermKey { layers }
}

/// 按密钥置换网络（行为不变）。
pub fn apply(net: &Net, key: &PermKey) -> Result<Net> {
    let hidden = net.layers().len().saturating_sub(1);
    if key.layers.len() != hidden {
        return Err(Error::Wire(format!(
            "置换密钥有 {} 层，网络隐藏层是 {hidden}",
            key.layers.len()
        )));
    }
    let mut current = net.clone();
    for (layer, perm) in key.layers.iter().enumerate() {
        current = current.permute_rows(layer, perm)?;
    }
    Ok(current)
}

/// 用贪心对齐攻击 `permuted`，报告能恢复多少权重。
pub fn attack(base: &Net, permuted: &Net, oracle: &Oracle) -> Result<AttackReport> {
    if base.arch() != permuted.arch() {
        return Err(Error::ArchMismatch);
    }
    let behavior_identical = oracle.outputs_of(base)? == oracle.outputs_of(permuted)?;
    let aligned = crate::merge::align(base, permuted)?;
    let mut trits = 0usize;
    let mut aligned_matches = 0usize;
    for (left, right) in base.layers().iter().zip(aligned.layers()) {
        for row in 0..left.rows() {
            for col in 0..left.cols() {
                trits += 1;
                if left.trit(row, col) == right.trit(row, col) {
                    aligned_matches += 1;
                }
            }
        }
    }
    Ok(AttackReport {
        layers: base.layers().len(),
        trits,
        aligned_matches,
        attack_success: aligned_matches == trits,
        behavior_identical,
    })
}
