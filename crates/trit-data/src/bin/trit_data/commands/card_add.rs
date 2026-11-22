//! `Card` 的累加：来源 / 许可证 / 标签 / 重写标记。

use super::Card;
use trit_data::schema::Trajectory;

pub fn add(card: &mut Card, t: &Trajectory) {
    let src = t.source.clone().unwrap_or_else(|| "missing".to_string());
    let lic = t.license.clone().unwrap_or_else(|| "missing".to_string());
    *card.sources.entry(src).or_insert(0) += 1;
    *card.licenses.entry(lic).or_insert(0) += 1;
    for l in &t.labels {
        *card.labels.entry(l.clone()).or_insert(0) += 1;
    }
    if t.source.is_none() || t.license.is_none() {
        card.provenance_missing += 1;
    }
    if t.harness
        .as_deref()
        .is_some_and(|h| h.to_lowercase().contains("rewrite"))
    {
        card.rewritten += 1;
    }
    card.content_stats.add(t);
    card.coverage.add(t);
}
