//! 格式 / 进制 / 位宽分布。

use super::QualityStats;

pub fn add(s: &mut QualityStats, text: &str) {
    for f in trit_data::format::detect(text) {
        *s.formats.entry(f.to_string()).or_insert(0) += 1;
    }
    *s.radix
        .entry(trit_data::radix::detect(text).as_str().to_string())
        .or_insert(0) += 1;
    for b in trit_data::bits::labels(text) {
        *s.bits.entry(b.to_string()).or_insert(0) += 1;
    }
}
