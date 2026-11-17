//! 按环境变量对权重做三值化。

use crate::quant::Quant;
use crate::quantize::quantize_matrix;
use crate::weights::Weights;

pub fn run(w: &mut Weights) {
    let quant = Quant::from_env_var("WSP_QUANT");
    let quant_row =
        std::env::var("WSP_QUANT_ROW").map_or(quant, |_| Quant::from_env_var("WSP_QUANT_ROW"));
    let target = std::env::var("WSP_QUANT_TARGET").unwrap_or_else(|_| "both".to_string());
    if target == "a" || target == "both" {
        quantize_matrix(&mut w.a, w.c, w.c, quant);
    }
    if target == "row" || target == "both" {
        quantize_matrix(&mut w.row_w, w.h, w.r, quant_row);
    }
    println!(
        "量化 A={quant:?} row={quant_row:?}｜目标 {target}｜A {}×{}｜二阶表 {}×{}",
        w.c, w.c, w.h, w.r
    );
}
