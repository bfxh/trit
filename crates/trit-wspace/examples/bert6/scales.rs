//! 读取 f32 校准的 per-channel 激活 scale（`scales.json`）。

use std::fs;

use serde_json::Value;

use crate::activation::LayerScales;

/// 读取 f32 校准的 per-channel scale（`scales.json`）。
pub(crate) fn load_scales(data_dir: &str) -> Vec<LayerScales> {
    let path = format!("{data_dir}/scales.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value["layers"]
        .as_array()
        .map(|layers| {
            layers
                .iter()
                .map(|layer| LayerScales {
                    norm1: read_vec(&layer["norm1"]),
                    ffn: read_vec(&layer["ffn"]),
                    norm2: read_vec(&layer["norm2"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_vec(value: &Value) -> Vec<f32> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_f64().map(|x| x as f32))
                .collect()
        })
        .unwrap_or_default()
}
