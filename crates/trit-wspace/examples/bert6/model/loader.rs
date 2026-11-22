//! 权重加载：safetensors → `Bert`（从 `model.rs` 拆出，守住行数预算）。
use crate::activation::NormKind;
use crate::linear::{F32Linear, Linear, QuantLinear};
use crate::model::{Bert, BertConfig, BertLayer};
use trit_wspace::safetensors;

pub(crate) fn load(file: &str, config: BertConfig) -> Bert {
    let BertConfig {
        layers,
        hidden,
        intermediate,
        heads,
        eps,
        quantize,
        activation,
        sidecar_rank,
        gptq_dir,
    } = config;
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
    let load_h = |short: &str| -> Option<Vec<f32>> {
        let dir = gptq_dir.as_ref()?;
        let path = format!("{dir}/{short}.f32");
        trit_wspace::activations::read_matrix(&path)
            .ok()
            .map(|(_, _, values)| values)
    };
    let wrap = |name: &str, short: &str| {
        let linear = tensor(name);
        if quantize {
            Linear::Quant(QuantLinear::from(
                &linear,
                sidecar_rank,
                load_h(short).as_deref(),
            ))
        } else {
            Linear::F32(linear)
        }
    };
    let mut built = Vec::with_capacity(layers);
    for index in 0..layers {
        let prefix = format!("encoder.layer.{index}");
        built.push(BertLayer {
            query: wrap(
                &format!("{prefix}.attention.self.query.weight"),
                &format!("q{index}"),
            ),
            key: wrap(
                &format!("{prefix}.attention.self.key.weight"),
                &format!("k{index}"),
            ),
            value: wrap(
                &format!("{prefix}.attention.self.value.weight"),
                &format!("v{index}"),
            ),
            output: wrap(
                &format!("{prefix}.attention.output.dense.weight"),
                &format!("o{index}"),
            ),
            norm1: norm(&format!("{prefix}.attention.output.LayerNorm")),
            intermediate: wrap(
                &format!("{prefix}.intermediate.dense.weight"),
                &format!("i{index}"),
            ),
            ffn_output: wrap(
                &format!("{prefix}.output.dense.weight"),
                &format!("f{index}"),
            ),
            norm2: norm(&format!("{prefix}.output.LayerNorm")),
            activation,
        });
    }
    let _ = intermediate;
    Bert {
        hidden,
        heads,
        head_dim: hidden / heads,
        layers: built,
        scales: None,
    }
}
