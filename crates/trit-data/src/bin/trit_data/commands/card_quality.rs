//! 代码质量 / 教育性 / 格式 / 进制分布。

use std::collections::BTreeMap;

use super::counts::map_json;
use trit_learn::json::Json;

#[derive(Default)]
pub struct QualityStats {
    pub code_quality: BTreeMap<String, usize>,
    pub edu: BTreeMap<String, usize>,
    pub formats: BTreeMap<String, usize>,
    pub radix: BTreeMap<String, usize>,
}

impl QualityStats {
    pub fn add(&mut self, text: &str) {
        let cq = trit_data::code_quality::check(text);
        let key = if cq.ok { "ok" } else { "bad" };
        *self.code_quality.entry(key.to_string()).or_insert(0) += 1;
        let e = trit_data::edu::score(text);
        let lo = e.floor() as u32;
        let bucket = format!("{lo}-{}", (lo + 1).min(5));
        *self.edu.entry(bucket).or_insert(0) += 1;
        for f in trit_data::format::detect(text) {
            *self.formats.entry(f.to_string()).or_insert(0) += 1;
        }
        *self
            .radix
            .entry(trit_data::radix::detect(text).as_str().to_string())
            .or_insert(0) += 1;
    }

    pub fn json(&self) -> Json {
        Json::obj()
            .put("code_quality", map_json(&self.code_quality))
            .put("edu", map_json(&self.edu))
            .put("formats", map_json(&self.formats))
            .put("radix", map_json(&self.radix))
    }
}
