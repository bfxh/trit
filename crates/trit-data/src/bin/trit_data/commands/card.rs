//! 数据卡统计：来源 / 许可证 / 标签 / 重写标记。
use self::card_labels::ContentStats;
use self::counts::map_json;
use std::collections::BTreeMap;
use trit_data::schema::Trajectory;
use trit_learn::json::Json;
#[path = "card_labels.rs"]
mod card_labels;
#[path = "counts.rs"]
mod counts;
#[derive(Default)]
pub struct Card {
    pub sources: BTreeMap<String, usize>,
    pub licenses: BTreeMap<String, usize>,
    pub labels: BTreeMap<String, usize>,
    pub provenance_missing: usize,
    pub rewritten: usize,
    pub content_stats: ContentStats,
}

impl Card {
    pub fn add(&mut self, t: &Trajectory) {
        let src = t.source.clone().unwrap_or_else(|| "missing".to_string());
        let lic = t.license.clone().unwrap_or_else(|| "missing".to_string());
        *self.sources.entry(src).or_insert(0) += 1;
        *self.licenses.entry(lic).or_insert(0) += 1;
        for l in &t.labels {
            *self.labels.entry(l.clone()).or_insert(0) += 1;
        }
        if t.source.is_none() || t.license.is_none() {
            self.provenance_missing += 1;
        }
        if t.harness
            .as_deref()
            .is_some_and(|h| h.to_lowercase().contains("rewrite"))
        {
            self.rewritten += 1;
        }
        self.content_stats.add(t);
    }

    pub fn json(&self) -> Json {
        Json::obj()
            .put("sources", map_json(&self.sources))
            .put("licenses", map_json(&self.licenses))
            .put("labels", map_json(&self.labels))
            .put("provenance_missing", self.provenance_missing as u64)
            .put("rewritten", self.rewritten as u64)
            .put("languages", map_json(&self.content_stats.languages))
            .put("styles", map_json(&self.content_stats.styles))
            .put("semantics", map_json(&self.content_stats.semantics))
    }
}
