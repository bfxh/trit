//! 内容统计：语言 / 风格 / 语义分布。
use super::card_quality::QualityStats;
use std::collections::BTreeMap;
use trit_data::schema::Trajectory;
use trit_data::script::{primary_language, trajectory_text};
use trit_data::{semantics, style};
#[derive(Default)]
pub struct ContentStats {
    pub languages: BTreeMap<String, usize>,
    pub styles: BTreeMap<String, usize>,
    pub semantics: BTreeMap<String, usize>,
    pub quality: QualityStats,
}

impl ContentStats {
    pub fn add(&mut self, t: &Trajectory) {
        let text = trajectory_text(t);
        *self
            .languages
            .entry(primary_language(&text).to_string())
            .or_insert(0) += 1;
        *self
            .styles
            .entry(style::detect(&text).as_str().to_string())
            .or_insert(0) += 1;
        *self
            .semantics
            .entry(semantics::detect(&text).as_str().to_string())
            .or_insert(0) += 1;
        self.quality.add(&text);
    }
}
