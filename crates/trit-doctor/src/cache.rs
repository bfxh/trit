use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const AGENT_DIR: &str = ".agent";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedFace {
    pub status: String,
    pub evidence: String,
    pub detail: String,
}

#[must_use]
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    serde_json::to_string(value)
        .ok()
        .and_then(|text| fs::write(path, text).ok())
        .is_some()
}
