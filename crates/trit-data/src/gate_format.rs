//! gate 的格式筛选。

use crate::gate::Config;

pub fn apply(cfg: &Config, text: &str, soft: &mut Vec<&'static str>) {
    let formats = crate::format::detect(text);
    if crate::format_filter::denied(&cfg.formats, &formats) {
        soft.push("format_filtered");
    }
}
