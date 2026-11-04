//! `status` 的 stat 缓存：文件没变就不读内容。
//!
//! 判等用 `(size, mtime_ns)`——和 git 的 index stat 缓存同一思路，但记录的是**内容 id**，
//! 命中时直接复用，不必再分块/压缩/哈希。缓存坏掉就整份丢弃（自愈），不影响判定正确性。
//!
//! 已知边界：同一台机器上"同大小 + 同 mtime 纳秒"的改写会被漏掉；NTFS/ext4 的纳秒精度下
//! 连续写不会撞，粗粒度文件系统（FAT）需要用内容模式兜底——这里不静默假装没有。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::hash::Hash;

const MAGIC: &[u8; 4] = b"TVSC";
const SCHEMA: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("不是 status 缓存（magic 不对）")]
    BadMagic,
    #[error("不支持的 status 缓存 schema：{0}")]
    UnsupportedSchema(u32),
    #[error("status 缓存被截断")]
    Truncated,
    #[error("status 缓存里的路径不是 UTF-8")]
    BadPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedFile {
    pub size: u64,
    pub mtime_ns: u64,
    pub id: Hash,
}

impl CachedFile {
    /// 元数据没变 ⇒ 复用上次算出的内容 id。
    pub fn matches(&self, size: u64, mtime_ns: u64) -> bool {
        self.size == size && self.mtime_ns == mtime_ns
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusCache {
    pub files: BTreeMap<String, CachedFile>,
    pub dirs: BTreeMap<String, u64>,
}

impl StatusCache {
    /// 读不出来就返回空缓存（坏文件不阻塞 `status`，只是这次要全量读）。
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| Self::decode(&bytes).ok())
            .unwrap_or_default()
    }

    /// 先写临时文件再改名：半截缓存不会覆盖好缓存。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, self.encode())?;
        fs::rename(&tmp, path)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&SCHEMA.to_le_bytes());
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        for (path, file) in &self.files {
            push_str(&mut out, path);
            out.extend_from_slice(&file.size.to_le_bytes());
            out.extend_from_slice(&file.mtime_ns.to_le_bytes());
            out.extend_from_slice(file.id.as_bytes());
        }
        out.extend_from_slice(&(self.dirs.len() as u32).to_le_bytes());
        for (path, mtime_ns) in &self.dirs {
            push_str(&mut out, path);
            out.extend_from_slice(&mtime_ns.to_le_bytes());
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CacheError> {
        let mut cur = Cursor {
            bytes,
            at: 0,
            end: bytes.len(),
        };
        if cur.take(4)? != MAGIC {
            return Err(CacheError::BadMagic);
        }
        let schema = cur.u32()?;
        if schema != SCHEMA {
            return Err(CacheError::UnsupportedSchema(schema));
        }
        let file_count = cur.u32()? as usize;
        let mut files = BTreeMap::new();
        for _ in 0..file_count {
            let path = cur.string()?;
            let size = cur.u64()?;
            let mtime_ns = cur.u64()?;
            let mut id = [0u8; 32];
            id.copy_from_slice(cur.take(32)?);
            files.insert(
                path,
                CachedFile {
                    size,
                    mtime_ns,
                    id: Hash::from_bytes(id),
                },
            );
        }
        let dir_count = cur.u32()? as usize;
        let mut dirs = BTreeMap::new();
        for _ in 0..dir_count {
            let path = cur.string()?;
            dirs.insert(path, cur.u64()?);
        }
        Ok(Self { files, dirs })
    }
}

fn push_str(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], CacheError> {
        let next = self.at.checked_add(len).ok_or(CacheError::Truncated)?;
        if next > self.end {
            return Err(CacheError::Truncated);
        }
        let slice = &self.bytes[self.at..next];
        self.at = next;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, CacheError> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(buf))
    }

    fn u64(&mut self) -> Result<u64, CacheError> {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(buf))
    }

    fn string(&mut self) -> Result<String, CacheError> {
        let mut len = [0u8; 2];
        len.copy_from_slice(self.take(2)?);
        let len = u16::from_le_bytes(len) as usize;
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| CacheError::BadPath)
    }
}

/// 缓存文件放在仓库元数据目录里，和 `HEAD`/`index.json` 同级。
pub fn cache_path(repo_root: &Path) -> PathBuf {
    repo_root.join(crate::repo::DIR).join("status-cache.bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StatusCache {
        let mut cache = StatusCache::default();
        cache.files.insert(
            "src/a.rs".to_string(),
            CachedFile {
                size: 12,
                mtime_ns: 123,
                id: Hash::of(b"a"),
            },
        );
        cache.dirs.insert("src".to_string(), 99);
        cache
    }

    #[test]
    fn cache_roundtrips() {
        let cache = sample();
        assert_eq!(StatusCache::decode(&cache.encode()).unwrap(), cache);
    }

    #[test]
    fn bad_cache_is_dropped_instead_of_failing() {
        assert!(matches!(
            StatusCache::decode(b"NOPE"),
            Err(CacheError::BadMagic) | Err(CacheError::Truncated)
        ));
        let mut bytes = sample().encode();
        bytes.truncate(bytes.len() - 3);
        assert!(matches!(
            StatusCache::decode(&bytes),
            Err(CacheError::Truncated)
        ));
        let mut wrong_schema = sample().encode();
        wrong_schema[4..8].copy_from_slice(&(SCHEMA + 1).to_le_bytes());
        assert!(matches!(
            StatusCache::decode(&wrong_schema),
            Err(CacheError::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn stat_equality_is_size_and_mtime_exactly() {
        let entry = CachedFile {
            size: 10,
            mtime_ns: 500,
            id: Hash::of(b"x"),
        };
        assert!(entry.matches(10, 500));
        assert!(!entry.matches(11, 500), "大小变了必须重算");
        assert!(!entry.matches(10, 501), "mtime 变了必须重算");
    }
}
