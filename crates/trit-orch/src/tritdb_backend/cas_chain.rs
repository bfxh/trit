//! manifest 链的合并与增量：每层只记变化条目，删除用 `!del:<key>` 标记。
//!
//! 旧的全量 manifest 也能沿链合并：每层都是完整快照，后层覆盖前层。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use tritdb_cas::{ChunkStore, Hash, Manifest};
use tritdb_txn::TxnStore;

use crate::Error;

/// 删除标记的键前缀；WAL 键一律以 `tr/` 开头，不会撞上。
const TOMBSTONE: &str = "!del:";

/// 删除标记的占位值。
fn tombstone() -> Hash {
    Hash::of(b"trit-trace:deleted")
}

fn deleted(key: &str) -> String {
    format!("{TOMBSTONE}{key}")
}

/// WAL 当前的全部 `key -> chunk hash`。
pub(super) fn read_wal(
    store: &TxnStore,
    chunks: &ChunkStore,
) -> Result<BTreeMap<String, Hash>, Error> {
    let mut map = BTreeMap::new();
    for (key, value) in store.iter().map_err(|error| io(Path::new("wal"), error))? {
        let hash = chunks
            .put(&value)
            .map_err(|error| io(Path::new("cas"), error))?;
        map.insert(String::from_utf8_lossy(&key).into_owned(), hash);
    }
    Ok(map)
}

/// 相对 `head` 链只返回差异：变更/新增条目 + 删除标记。
pub(super) fn delta(
    chunks: &ChunkStore,
    head: &Hash,
    current: &BTreeMap<String, Hash>,
) -> Result<BTreeMap<String, Hash>, Error> {
    let base = resolve(chunks, head)?.0;
    let mut entries = BTreeMap::new();
    for (name, hash) in current {
        if base.get(name) != Some(hash) {
            entries.insert(name.clone(), *hash);
        }
    }
    for name in base.keys() {
        if !current.contains_key(name) {
            entries.insert(deleted(name), tombstone());
        }
    }
    Ok(entries)
}

/// 沿 parent 链合并到 `head`：子层覆盖父层，删除标记移除对应键。
pub(super) fn resolve(
    chunks: &ChunkStore,
    head: &Hash,
) -> Result<(BTreeMap<String, Hash>, Hash), Error> {
    let mut layers = Vec::new();
    let mut seen = HashSet::new();
    let mut current = *head;
    loop {
        if !seen.insert(current) {
            return Err(Error::TraceParse {
                path: "cas".into(),
                line: 0,
                message: "manifest 链出现环".into(),
            });
        }
        let manifest =
            Manifest::load(chunks, &current).map_err(|error| io(Path::new("cas"), error))?;
        let parent = manifest.parent;
        layers.push(manifest);
        match parent {
            Some(next) => current = next,
            None => break,
        }
    }
    let root = current;
    let mut merged = BTreeMap::new();
    for manifest in layers.iter().rev() {
        for (name, hash) in &manifest.entries {
            match name.strip_prefix(TOMBSTONE) {
                Some(key) => {
                    merged.remove(key);
                }
                None => {
                    merged.insert(name.clone(), *hash);
                }
            }
        }
    }
    Ok((merged, root))
}

pub(super) fn io(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tritdb_cas::{ChunkStore, Manifest};

    use super::{deleted, delta, resolve};

    #[test]
    fn delta_marks_deletions_and_resolve_prunes_them() {
        let mut root = std::env::temp_dir();
        root.push(format!("trit-cas-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let chunks = ChunkStore::open(&root).unwrap();

        let a = chunks.put(b"a").unwrap();
        let b = chunks.put(b"b").unwrap();
        let mut base = BTreeMap::new();
        base.insert("tr/1".to_string(), a);
        base.insert("tr/2".to_string(), b);
        let first = Manifest::new(None, base).store(&chunks).unwrap();

        // tr/1 被删、tr/2 不变、tr/3 新增。
        let c = chunks.put(b"c").unwrap();
        let mut current = BTreeMap::new();
        current.insert("tr/2".to_string(), b);
        current.insert("tr/3".to_string(), c);
        let entries = delta(&chunks, &first, &current).unwrap();
        assert!(entries.contains_key(&deleted("tr/1")));
        assert!(entries.contains_key("tr/3"));
        assert!(!entries.contains_key("tr/2"));

        let head = Manifest::new(Some(first), entries).store(&chunks).unwrap();
        let (merged, root_head) = resolve(&chunks, &head).unwrap();
        assert_eq!(root_head, first);
        assert!(!merged.contains_key("tr/1"));
        assert_eq!(merged.get("tr/2"), Some(&b));
        assert_eq!(merged.get("tr/3"), Some(&c));

        let _ = std::fs::remove_dir_all(&root);
    }
}
