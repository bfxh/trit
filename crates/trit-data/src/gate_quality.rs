//! gate 的代码质量 / 教育性筛选。

use crate::gate::Config;

pub fn apply(cfg: &Config, text: &str, soft: &mut Vec<&'static str>) {
    if cfg.code_quality
        && crate::content::analyze(text).code_ratio >= cfg.code_threshold
        && !crate::code_quality::check(text).ok
    {
        soft.push("code_quality_filtered");
    }
    if cfg.edu_min > 0.0 && crate::edu::score(text) < cfg.edu_min {
        soft.push("edu_filtered");
    }
}
