//! repo 报告构建。

use std::collections::BTreeMap;

use trit_learn::json::Json;

pub struct Stats<'a> {
    pub repo: &'a str,
    pub files: usize,
    pub languages: &'a BTreeMap<String, usize>,
    pub licenses: &'a BTreeMap<String, usize>,
    pub generated: usize,
    pub tests: usize,
    pub with_imports: usize,
    pub license_missing: usize,
    pub imports_unknown: usize,
}

pub fn build(s: &Stats<'_>) -> Json {
    let mut languages = Json::obj();
    for (k, v) in s.languages {
        languages = languages.put(k.clone(), *v as u64);
    }
    let mut licenses = Json::obj();
    for (k, v) in s.licenses {
        licenses = licenses.put(k.clone(), *v as u64);
    }
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-repo")
        .put("repo", s.repo.to_string())
        .put("files", s.files as u64)
        .put("languages", languages)
        .put("licenses", licenses)
        .put("generated", s.generated as u64)
        .put("tests", s.tests as u64)
        .put("with_imports", s.with_imports as u64)
        .put("license_missing", s.license_missing as u64)
        .put("imports_unknown", s.imports_unknown as u64)
        .put("tests_ratio", s.tests as f64 / s.files.max(1) as f64)
}
