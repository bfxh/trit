//! Card 的 JSON 输出。

use super::Card; use super::counts::map_json;
use trit_learn::json::Json;

pub fn build(card: &Card) -> Json {
    Json::obj()
        .put("sources", map_json(&card.sources))
        .put("licenses", map_json(&card.licenses))
        .put("labels", map_json(&card.labels))
        .put("provenance_missing", card.provenance_missing as u64)
        .put("rewritten", card.rewritten as u64)
        .put("languages", map_json(&card.content_stats.languages))
        .put("styles", map_json(&card.content_stats.styles))
        .put("semantics", map_json(&card.content_stats.semantics))
        .put("quality", card.content_stats.quality.json())
        .put("coverage", card.coverage.json())
}
