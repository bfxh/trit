//! 实验权重格式的字节账：默认只算线性权重；有 `TRIT_BUDGET` 时按实际
//! per-block 位宽、per-group scale 和 per-row bias 报总字节。

use super::account::budget_account;
use serde_json::{Value, json};

/// MiniLM 每层线性参数：q/k/v/o 四个 `h×h` + FFN 两个 `h×4h`/`4h×h` = `12h²`。
pub(crate) fn report(hidden: usize) -> Value {
    let bits = std::env::var("TRIT_WEIGHT_BITS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(2);
    let mut f32_layers = f32_layers();
    f32_layers.sort_unstable();
    f32_layers.dedup();
    let layers = layers();
    let per_layer = 12 * hidden * hidden;
    let f32_params = f32_layers.iter().filter(|&&layer| layer < layers).count() * per_layer;
    let quant_params = layers * per_layer - f32_params;
    let quant_bytes = (quant_params * bits).div_ceil(8);
    let f32_bytes = f32_params * 4;
    let dense_bytes = layers * per_layer * 4;

    if let Some(account) = budget_account(layers, &f32_layers) {
        return json!({
            "bits": bits,
            "mixed": true,
            "mixed_bits": account.counts,
            "f32_layers": f32_layers,
            "quant_params": quant_params,
            "f32_params": f32_params,
            "budget_group": account.group,
            "budget_actual_bytes": account.actual_bytes,
            "budget_weight_bytes": account.weight_bytes,
            "budget_metadata_bytes": account.metadata_bytes,
            "budget_f32_bytes": account.f32_bytes,
            "weight_bytes": account.actual_bytes,
            "dense_f32_bytes": dense_bytes,
            "ratio_vs_f32": account.actual_bytes as f64 / dense_bytes as f64,
            "accounting": "actual_bytes_with_metadata",
        });
    }

    json!({
        "bits": bits,
        "f32_layers": f32_layers,
        "quant_params": quant_params,
        "f32_params": f32_params,
        "weight_bytes": quant_bytes + f32_bytes,
        "dense_f32_bytes": dense_bytes,
        "ratio_vs_f32": (quant_bytes + f32_bytes) as f64 / dense_bytes as f64,
    })
}

fn f32_layers() -> Vec<usize> {
    std::env::var("TRIT_F32_LAYERS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter(|item| !item.trim().is_empty())
                .map(|item| item.trim().parse().expect("层号"))
                .collect()
        })
        .unwrap_or_default()
}

fn layers() -> usize {
    std::env::var("TRIT_LAYERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(6)
}
