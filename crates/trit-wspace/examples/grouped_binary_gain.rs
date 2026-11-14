//! 分组三值的**二进制落点**：WSP1G 文本 vs WSP2G 二进制的真实字节数。
//!
//! 文本 `wsp1g` 每个 trit 占一个 ASCII 字节、每个组号一段十进制文本——`docs/WSP-SUMMARY.md`
//! 里「42.4 MB → 56.9 MB（0.75×，比 f32 还大）」就是它的实测。二进制把 trit 压到 2 bit、
//! 组号压到 `ceil(log2 组数)` bit，**组号计入**，这才谈得上压缩。
//!
//! 口径：f32 只算权重（`rows×cols×4`，与 WSP-SUMMARY §6.2 一致）；两种三值格式都含
//! bias f32 与格式头，所以压缩率是**保守**的。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example grouped_binary_gain -- \
//!   model.safetensors 32,64,128,256
//! ```

use serde_json::json;
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_codec::to_wspg;
use trit_wspace::safetensors;
use trit_wspace::{from_wsp2_grouped, to_wsp2_grouped};

const LAYERS: usize = 6;
const LINEAR: [&str; 6] = [
    "attention.self.query.weight",
    "attention.self.key.weight",
    "attention.self.value.weight",
    "attention.output.dense.weight",
    "intermediate.dense.weight",
    "output.dense.weight",
];

/// 往返判等：逐 trit、逐组号、逐 scale 完全一致。
fn same(a: &GroupedLayer, b: &GroupedLayer) -> bool {
    if (a.rows(), a.cols(), a.group_count(), a.group_size())
        != (b.rows(), b.cols(), b.group_count(), b.group_size())
    {
        return false;
    }
    for row in 0..a.rows() {
        for col in 0..a.cols() {
            if a.trit(row, col) != b.trit(row, col) || a.group_of(row, col) != b.group_of(row, col)
            {
                return false;
            }
        }
    }
    a.scales() == b.scales()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model = args
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("用法: grouped_binary_gain <model.safetensors> [32,64,128,256]"));
    let groups: Vec<usize> = match args.get(1) {
        Some(text) => text
            .split(',')
            .filter_map(|word| word.trim().parse().ok())
            .collect(),
        None => vec![32, 64, 128, 256],
    };
    if groups.is_empty() {
        panic!("至少要一个组大小");
    }

    let mut tensors = Vec::new();
    let mut f32_bytes = 0usize;
    let mut elements = 0usize;
    for index in 0..LAYERS {
        for suffix in LINEAR {
            let name = format!("encoder.layer.{index}.{suffix}");
            let weight = safetensors::read_tensor(&model, &name)
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
            let bias_name = name.replace(".weight", ".bias");
            let bias = safetensors::read_vector(&model, &bias_name)
                .unwrap_or_else(|error| panic!("{bias_name}: {error:?}"));
            f32_bytes += weight.rows * weight.cols * 4;
            elements += weight.rows * weight.cols;
            tensors.push((name, weight.rows, weight.cols, weight.values, bias));
        }
    }

    let mut results = Vec::new();
    for &group in &groups {
        let mut layers = Vec::with_capacity(tensors.len());
        for (name, rows, cols, values, bias) in &tensors {
            let layer = GroupedLayer::quantize(*rows, *cols, values, bias, group)
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
            layers.push(layer);
        }
        let text_bytes: usize = layers.iter().map(|layer| to_wspg(layer).len()).sum();
        let binary = to_wsp2_grouped(&layers).expect("encode");
        let back = from_wsp2_grouped(&binary).expect("decode");
        let roundtrip =
            back.len() == layers.len() && layers.iter().zip(&back).all(|(a, b)| same(a, b));
        let group_count = layers.first().map(GroupedLayer::group_count).unwrap_or(0);
        results.push(json!({
            "group": group,
            "group_count_per_layer": group_count,
            "f32_bytes": f32_bytes,
            "wsp1g_text_bytes": text_bytes,
            "wsp2g_binary_bytes": binary.len(),
            "compression_wsp1g": f32_bytes as f64 / text_bytes.max(1) as f64,
            "compression_wsp2g": f32_bytes as f64 / binary.len().max(1) as f64,
            "bits_per_weight_wsp2g": 8.0 * binary.len() as f64 / elements as f64,
            "wsp2g_vs_wsp1g": text_bytes as f64 / binary.len().max(1) as f64,
            "roundtrip_bit_identical": roundtrip,
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "model": model,
            "tensors": tensors.len(),
            "elements": elements,
            "f32_weight_bytes": f32_bytes,
            "results": results,
        }))
        .expect("json")
    );
}
