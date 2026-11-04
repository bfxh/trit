//! 技术标签索引（techmap）的只读适配器：按路径/测试/标签反查，并把测试名翻成可跑的命令。
//!
//! 索引文件由 `repo-tools/scripts/techmap.py` 维护；这里只读、不改，保证两边口径一致。
//! 目标很具体：**拿到一个提交或一个文件，能直接说出它属于哪些技术、该跑哪些测试**。

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TechmapError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("techmap 不是合法 JSON：{0}")]
    Json(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct Doc {
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Techmap {
    entries: Vec<Entry>,
}

impl Techmap {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TechmapError> {
        let text = std::fs::read_to_string(path)?;
        let doc: Doc =
            serde_json::from_str(&text).map_err(|e| TechmapError::Json(e.to_string()))?;
        Ok(Self {
            entries: doc.entries,
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 子串反查：路径、测试名、标签、标题、id 任一命中。
    pub fn where_entries(&self, needle: &str) -> Vec<&Entry> {
        let needle = needle.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                entry.id.to_lowercase().contains(&needle)
                    || entry.title.to_lowercase().contains(&needle)
                    || entry
                        .paths
                        .iter()
                        .chain(entry.tests.iter())
                        .chain(entry.tags.iter())
                        .any(|field| field.to_lowercase().contains(&needle))
            })
            .collect()
    }

    /// 提交 tags → 条目（任一标签命中）。
    pub fn for_tags(&self, tags: &[String]) -> Vec<&Entry> {
        let wanted: Vec<String> = tags.iter().map(|tag| tag.to_lowercase()).collect();
        self.entries
            .iter()
            .filter(|entry| {
                entry
                    .tags
                    .iter()
                    .any(|tag| wanted.iter().any(|want| want == &tag.to_lowercase()))
            })
            .collect()
    }
}

/// `crates/<crate>/...::<test>` → `cargo test -p <crate> <test>`。
pub fn test_command(spec: &str) -> Option<String> {
    let (path, name) = spec.split_once("::")?;
    let crate_name = path.strip_prefix("crates/")?.split('/').next()?;
    if crate_name.is_empty() || name.is_empty() {
        return None;
    }
    Some(format!("cargo test -p {crate_name} {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Techmap {
        let text = r#"{
          "entries": [
            {"id":"a","title":"alpha kernel","tags":["gpu","wf32"],
             "paths":["crates/trit-gpu/src/a.rs"],
             "tests":["crates/trit-gpu/tests/a.rs::kernel_matches"]},
            {"id":"b","title":"merge","tags":["vcs","merge"],
             "paths":["crates/trit-vcs/src/merge.rs"],"tests":[]}
          ]
        }"#;
        let doc: Doc = serde_json::from_str(text).unwrap();
        Techmap {
            entries: doc.entries,
        }
    }

    #[test]
    fn where_matches_paths_tests_and_tags() {
        let map = fixture();
        assert_eq!(map.where_entries("trit-gpu/src/a.rs")[0].id, "a");
        assert_eq!(map.where_entries("kernel_matches")[0].id, "a");
        assert_eq!(map.where_entries("merge")[0].id, "b");
        assert!(map.where_entries("nothing-here").is_empty());
    }

    #[test]
    fn tags_map_commits_to_entries() {
        let map = fixture();
        let hits = map.for_tags(&["vcs".to_string(), "gpu".to_string()]);
        assert_eq!(hits.len(), 2);
        assert!(map.for_tags(&["nope".to_string()]).is_empty());
    }

    #[test]
    fn test_specs_turn_into_runnable_commands() {
        assert_eq!(
            test_command("crates/trit-gpu/tests/a.rs::kernel_matches").unwrap(),
            "cargo test -p trit-gpu kernel_matches"
        );
        assert!(test_command("scripts/foo.py::x").is_none());
        assert!(test_command("no-separator").is_none());
    }

    #[test]
    fn bad_or_missing_files_are_loud() {
        let dir = std::env::temp_dir().join(format!("trit-vcs-techmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"not json").unwrap();
        assert!(matches!(Techmap::load(&bad), Err(TechmapError::Json(_))));
        assert!(matches!(
            Techmap::load(dir.join("missing.json")),
            Err(TechmapError::Io(_))
        ));
        let empty = dir.join("empty.json");
        std::fs::write(&empty, br#"{"entries":[]}"#).unwrap();
        assert!(Techmap::load(&empty).unwrap().entries().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
