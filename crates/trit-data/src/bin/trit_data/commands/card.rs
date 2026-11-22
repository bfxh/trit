//! 数据卡统计：结构定义与入口。
use self::card_coverage::Coverage;
use self::card_labels::ContentStats;
use std::collections::BTreeMap;
use trit_data::schema::Trajectory;
use trit_learn::json::Json;
#[path = "card_add.rs"]
mod card_add;
#[path = "card_coverage.rs"]
mod card_coverage;
#[path = "card_json.rs"]
mod card_json;
#[path = "card_labels.rs"]
mod card_labels;
#[path = "card_quality.rs"]
mod card_quality;
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
    pub coverage: Coverage,
}
impl Card {
    pub fn add(&mut self, t: &Trajectory) {
        card_add::add(self, t)
    }
    pub fn json(&self) -> Json {
        card_json::build(self)
    }
}
