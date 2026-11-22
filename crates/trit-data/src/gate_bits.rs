//! gate 的位宽 / 多值筛选。

use crate::gate::Config;

pub fn apply(cfg: &Config, text: &str, soft: &mut Vec<&'static str>) {
    let bits = crate::bits::labels(text);
    if crate::format_filter::denied(&cfg.bits, &bits) {
        soft.push("bits_filtered");
    }
}
