//! `TRIT_BUDGET` 的实际字节账：按 blocks 的 rows/cols/bits 与 group 复算。

use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct BudgetAccount {
    pub(super) group: usize,
    pub(super) actual_bytes: u64,
    pub(super) weight_bytes: u64,
    pub(super) metadata_bytes: u64,
    pub(super) f32_bytes: u64,
    pub(super) counts: BTreeMap<String, usize>,
}

/// 公式与 `scripts/wsp_budget.py` 一致：
/// `ceil(rows*cols*bits/8) + rows*(ceil(cols/group)*4 + 4)`。
/// 整层 f32 回退的层按 `rows*cols*4` 计，不再计 scale/bias。
pub(super) fn budget_account(layers: usize, f32_layers: &[usize]) -> Option<BudgetAccount> {
    let path = std::env::var("TRIT_BUDGET").ok()?;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
    let value: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("解析 {path} 失败：{e}"));
    let group = value["group"].as_u64().unwrap_or(32) as usize;
    assert!(group > 0, "budget group 必须大于 0");
    let blocks = value["blocks"]
        .as_array()
        .unwrap_or_else(|| panic!("{path} 缺少 blocks"));
    let mut counts = BTreeMap::new();
    let mut actual_bytes = 0_u64;
    let mut weight_bytes = 0_u64;
    let mut metadata_bytes = 0_u64;
    let mut f32_bytes = 0_u64;
    let mut seen = 0_usize;
    for block in blocks {
        let layer = block["layer"].as_u64().expect("budget layer") as usize;
        if layer >= layers {
            continue;
        }
        let rows = block["rows"].as_u64().expect("budget rows") as usize;
        let cols = block["cols"].as_u64().expect("budget cols") as usize;
        let bits = block["bits"].as_u64().expect("budget bits") as u32;
        assert!((2..=5).contains(&bits), "budget bits 要在 2..=5");
        assert!(rows > 0 && cols > 0, "budget rows/cols 必须大于 0：{layer}");
        seen += 1;
        let f32_block = (rows as u64) * (cols as u64) * 4;
        if f32_layers.contains(&layer) {
            actual_bytes += f32_block;
            f32_bytes += f32_block;
            continue;
        }
        let weight = ((rows as u64) * (cols as u64) * u64::from(bits)).div_ceil(8);
        let metadata = (rows as u64) * (((cols as u64).div_ceil(group as u64)) * 4 + 4);
        actual_bytes += weight + metadata;
        weight_bytes += weight;
        metadata_bytes += metadata;
        *counts.entry(bits.to_string()).or_insert(0) += 1;
    }
    assert!(
        seen == layers * 6,
        "{path} 的 blocks 只覆盖 {seen} 个线性矩阵，期望 {}（layers={layers}）",
        layers * 6
    );
    Some(BudgetAccount {
        group,
        actual_bytes,
        weight_bytes,
        metadata_bytes,
        f32_bytes,
        counts,
    })
}
