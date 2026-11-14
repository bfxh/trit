//! 秩一编辑能不能三值化？——用真实 MiniLM 权重量"编辑残留"。
//!
//! ## 为什么要问这个
//!
//! 秩一编辑（ROME 式）把一条事实写成 `W' = W + δ xᵀ/(xᵀx)`，保证 `W' x = target`
//! **精确成立**。但 `δ xᵀ` 是**稠密**的——整行数值都变。
//!
//! 而三值化只保留**符号**（× 组 scale）。`tests/rank_one.rs` 已经测出：秩一更新
//! 虽然让整行数值都变，**符号翻转却很稀疏**（只有接近零的权重会变号）。这意味着
//! 三值化很可能把编辑效果丢掉大半。
//!
//! 顺序也无法回避：只能「先 f32 编辑 → 再量化」，反过来在三值空间里做秩一编辑不可能
//! （trit 只有 −1/0/+1 三档，写不出连续增量）。
//!
//! ## 本示例量什么
//!
//! 对 MiniLM 的真实权重做一次秩一编辑，然后：
//!
//! 1. `edited_f32`：编辑后的误差（应为 0）；
//! 2. `ternary group=G`：三值化之后的误差与余弦——**编辑残留**；
//! 3. `unquantized→quantized`：**没编辑**直接三值化的误差（对照，看哪些误差是三值化本身的）。
//!
//! 用法：`rank_one_ternary <model> [层号] [group...]`

use serde_json::json;
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::rank_one::{apply, edit_cosine, edit_error, rank_one_update};
use trit_wspace::safetensors;

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: rank_one_ternary <model.safetensors> [层号] [group...]");
    let layer: usize = args.next().map(|v| v.parse().expect("layer")).unwrap_or(0);
    let groups: Vec<usize> = {
        let rest: Vec<usize> = args.filter_map(|v| v.parse().ok()).collect();
        if rest.is_empty() {
            vec![256, 1024, 4096]
        } else {
            rest
        }
    };

    let name = format!("encoder.layer.{layer}.attention.self.query.weight");
    let bias_name = name.replace(".weight", ".bias");
    let weight = safetensors::read_tensor(&model, &name).expect("weight");
    let bias = safetensors::read_vector(&model, &bias_name).expect("bias");
    let embeddings = safetensors::read_tensor(&model, EMBEDDINGS).expect("embeddings");
    let (rows, cols) = (weight.rows, weight.cols);
    let dim = cols.min(embeddings.cols);

    // 编辑输入：取一个真实词向量（前 dim 维）
    let x: Vec<f32> = embeddings.values[..dim].to_vec();
    // 编辑目标：人为构造（与当前输出明显不同的方向）
    let target: Vec<f32> = (0..rows)
        .map(|index| ((index as f32) * 0.013).sin() * 0.8)
        .collect();

    let original = weight.values.clone();
    let before = edit_error(&original, rows, cols, &x, &target);

    // 秩一编辑（f32）
    let mut edited = original.clone();
    rank_one_update(&mut edited, rows, cols, &x, &target).expect("rank-one update");
    let after_edit = edit_error(&edited, rows, cols, &x, &target);
    let cosine_edit = edit_cosine(&edited, rows, cols, &x, &target);

    // 原始输出（用于衡量三值化把输出方向带偏了多少）
    let original_output = apply(&original, rows, cols, &x);

    // 三值化：编辑后的层 / 未编辑的层（对照）
    let mut rows_json = Vec::new();
    for group in &groups {
        let edited_grouped =
            GroupedLayer::quantize(rows, cols, &edited, &bias, *group).expect("quantize edited");
        let plain_grouped =
            GroupedLayer::quantize(rows, cols, &original, &bias, *group).expect("quantize plain");
        let edited_ternary = edited_grouped.to_f32();
        let plain_ternary = plain_grouped.to_f32();
        rows_json.push(json!({
            "group": group,
            "bits_per_weight": edited_grouped.bits_per_weight(),
            // 编辑后三值化：编辑残留
            "edited_then_ternary": {
                "error": edit_error(&edited_ternary, rows, cols, &x, &target),
                "cosine": edit_cosine(&edited_ternary, rows, cols, &x, &target),
            },
            // 对照：没编辑直接三值化（衡量三值化本身对"原始输出"的影响）
            "plain_ternary": {
                "error": edit_error(&plain_ternary, rows, cols, &x, &target),
                "cosine": edit_cosine(&plain_ternary, rows, cols, &x, &target),
                "output_drift": 1.0
                    - edit_cosine(&plain_ternary, rows, cols, &x, &original_output),
            },
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "layer": layer,
            "rows": rows,
            "cols": cols,
            "edit": {
                "error_before_edit": before,
                "error_after_edit": after_edit,
                "cosine_after_edit": cosine_edit,
            },
            "ternary": rows_json,
            "note": "顺序只能是「先 f32 编辑 → 再量化」；三值空间里写不出连续增量",
        }))
        .expect("json")
    );
}
