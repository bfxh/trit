//! gate 的内容 / 风格 / 语义筛选。

use crate::gate::Config;

pub fn apply(cfg: &Config, text: &str, soft: &mut Vec<&'static str>) {
    if cfg.code_filter || cfg.natural_filter {
        let content = crate::content::analyze(text);
        if cfg.code_filter && content.code_ratio >= cfg.code_threshold {
            soft.push("code_filtered");
        }
        if cfg.natural_filter && content.natural_ratio >= cfg.code_threshold {
            soft.push("natural_filtered");
        }
    }
    let style = crate::style::detect(text);
    if cfg.style.denied(style.as_str()) {
        soft.push("style_filtered");
    }
    let semantics = crate::semantics::detect(text);
    if cfg.semantics.denied(semantics.as_str()) {
        soft.push("semantics_filtered");
    }
}
