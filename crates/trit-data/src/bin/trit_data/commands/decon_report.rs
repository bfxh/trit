//! decontaminate 报告构建。

use std::path::Path;

use trit_learn::json::Json;

pub struct Stats<'a> {
    pub jsonl: &'a Path,
    pub bench: &'a Path,
    pub texts: usize,
    pub ngram: usize,
    pub threshold: f64,
    pub records: usize,
    pub contaminated: usize,
    pub language_filtered: usize,
    pub filtered: usize,
}

pub fn build(s: &Stats<'_>) -> Json {
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-decontaminate")
        .put("created_unix", crate::commands::created_unix())
        .put(
            "input",
            Json::obj().put("path", s.jsonl.display().to_string()),
        )
        .put(
            "bench",
            Json::obj()
                .put("path", s.bench.display().to_string())
                .put("texts", s.texts as u64),
        )
        .put("ngram", s.ngram as u64)
        .put("threshold", s.threshold)
        .put("records", s.records as u64)
        .put("contaminated", s.contaminated as u64)
        .put("language_filtered", s.language_filtered as u64)
        .put("filtered", s.filtered as u64)
}
