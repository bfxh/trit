//! 权重加载：safetensors → `Bert`（从 `model.rs` 拆出，守住行数预算）。
use crate::activation::NormKind;
use crate::linear::{F32Linear, Linear};
use crate::model::{Bert, BertConfig, BertLayer};
use trit_wspace::safetensors;
mod h;
mod wrap;

pub(crate) fn load(file: &str, config: BertConfig) -> Bert {
    let (layers, hidden) = (config.layers, config.hidden);
    let (heads, eps) = (config.heads, config.eps);
    let activation = config.activation;
    let tensor = |name: &str| -> F32Linear {
        let weight =
            safetensors::read_tensor(file, name).unwrap_or_else(|e| panic!("读 {name} 失败：{e}"));
        let bias_name = name.replace(".weight", ".bias");
        let bias = safetensors::read_vector(file, &bias_name)
            .unwrap_or_else(|e| panic!("读 {bias_name} 失败：{e}"));
        F32Linear {
            rows: weight.rows,
            cols: weight.cols,
            weight: weight.values,
            bias,
        }
    };
    let norm = |prefix: &str| -> NormKind {
        let weight =
            safetensors::read_vector(file, &format!("{prefix}.weight")).expect("LN weight");
        let bias = safetensors::read_vector(file, &format!("{prefix}.bias")).expect("LN bias");
        NormKind::new(activation, weight, bias, eps)
    };
    let build = |name: &str, short: &str, layer: usize| -> Linear {
        wrap::build(tensor(name), layer, short, &config)
    };
    let mut built = Vec::with_capacity(layers);
    for index in 0..layers {
        let prefix = format!("encoder.layer.{index}");
        built.push(BertLayer {
            query: build(
                &format!("{prefix}.attention.self.query.weight"),
                &format!("q{index}"),
                index,
            ),
            key: build(
                &format!("{prefix}.attention.self.key.weight"),
                &format!("k{index}"),
                index,
            ),
            value: build(
                &format!("{prefix}.attention.self.value.weight"),
                &format!("v{index}"),
                index,
            ),
            output: build(
                &format!("{prefix}.attention.output.dense.weight"),
                &format!("o{index}"),
                index,
            ),
            norm1: norm(&format!("{prefix}.attention.output.LayerNorm")),
            intermediate: build(
                &format!("{prefix}.intermediate.dense.weight"),
                &format!("i{index}"),
                index,
            ),
            ffn_output: build(
                &format!("{prefix}.output.dense.weight"),
                &format!("f{index}"),
                index,
            ),
            norm2: norm(&format!("{prefix}.output.LayerNorm")),
            activation,
        });
    }
    Bert {
        hidden,
        heads,
        head_dim: hidden / heads,
        layers: built,
        scales: None,
    }
}
