//! 检索上下文输出：BM25 命中 + 依赖闭包，带内容级指纹。

use std::path::Path;

use trit_data::repo;
use trit_data::repo::Unit;
use trit_learn::json::{Json, write_json};

const BUDGET: usize = 8000;
const MAX_FILES: usize = 8;

pub fn json(root: &Path, units: &[Unit], query: &str) -> Json {
    let b = repo::context::bundle(root, units, query, BUDGET, MAX_FILES);
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-context")
        .put("query", b.query)
        .put("budget_tokens", b.budget as u64)
        .put("total_tokens", b.total_tokens as u64)
        .put("bundle_sha256", b.sha256)
        .put("files", Json::arr(b.hits.iter().map(file).collect()))
}

fn file(h: &repo::context::Hit) -> Json {
    Json::obj()
        .put("path", h.path.clone())
        .put("reason", h.reason.to_string())
        .put("score", h.score)
        .put("tokens", h.tokens as u64)
}

pub fn print_and_write(
    root: &Path,
    units: &[Unit],
    query: &str,
    out: Option<&Path>,
) -> Result<(), String> {
    println!("{}", json(root, units, query).dump());
    if let Some(dir) = out { write(dir, root, units, query) } else { Ok(()) }
}

pub fn write(dir: &Path, root: &Path, units: &[Unit], query: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}: {e}", dir.display()))?;
    write_json(&dir.join("repo.context.json"), &json(root, units, query))
}
