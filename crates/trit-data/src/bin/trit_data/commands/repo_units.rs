//! RepoUnit → JSONL 行。

use trit_data::repo::Unit;
use trit_learn::json::Json;

pub fn lines(units: &[Unit]) -> Vec<String> {
    units
        .iter()
        .map(|u| {
            Json::obj()
                .put("repo", u.repo.clone())
                .put("path", u.path.clone())
                .put("lang", u.lang.clone())
                .put("bytes", u.bytes)
                .put("generated", u.generated)
                .put(
                    "license",
                    u.license.clone().unwrap_or_else(|| "missing".into()),
                )
                .put(
                    "imports",
                    Json::arr(u.imports.iter().map(Json::str).collect()),
                )
                .put("imports_known", u.imports_known)
                .put("is_test", u.is_test)
                .dump()
        })
        .collect()
}
