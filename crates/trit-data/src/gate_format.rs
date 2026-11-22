//! gate 的格式筛选。

use crate::gate::Config;

pub fn apply(cfg: &Config, text: &str, soft: &mut Vec<&'static str>) {
    let formats = crate::format::detect(text);
    if formats.iter().any(|f| cfg.formats.denied(f)) {
        soft.push("format_filtered");
    }
}
