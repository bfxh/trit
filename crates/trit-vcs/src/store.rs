//! 内容寻址对象库：**打包存储 + zlib 压缩 + 读时校验**。
//!
//! 与 Git 的差别（都是"更强"或"至少不弱"）：
//! 1. **块级去重**：文件按内容定义切块（[`crate::chunk`]），插入一段只新增边界附近那几个块；
//! 2. **打包**：块按 `[u32 长度][zlib 负载]` 追加进单个 pack，避免每块一个小文件的簇开销
//!    （第一版按文件存，60 个文件 × 20 轮的样本比 git 大 5 倍，主要是这个开销）；
//! 3. **读时校验**：每次读出来都重算 BLAKE3 与索引里的哈希比对，被改一个字节立刻现形。
//!
//! 单写者假设：写入期间持 `pack.lock`（`create_new` 语义），第二个进程直接报错而不是写坏包。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::chunk::Chunking;
use crate::delta::{
    PackEntry, PackPlan, apply_delta, common_prefix, common_suffix, compress, decompress,
};
use crate::hash::Hash;

/// 一个文件 = 有序块列表 + 原始长度。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileMap {
    pub size: u64,
    pub chunks: Vec<Hash>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub objects: u64,
    pub bytes_stored: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PutStats {
    pub new_chunks: u64,
    pub reused_chunks: u64,
    pub bytes_written: u64,
}

impl PutStats {
    pub fn total_chunks(&self) -> u64 {
        self.new_chunks + self.reused_chunks
    }

    /// 复用比例（按块数）。0.0 = 一点没省。
    pub fn reuse_ratio(&self) -> f64 {
        if self.total_chunks() == 0 {
            return 0.0;
        }
        self.reused_chunks as f64 / self.total_chunks() as f64
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("对象缺失: {0}")]
    Missing(String),
    #[error("对象内容与名字对不上（被改过或损坏）: 期望 {expected}，实际 {actual}")]
    Corrupt { expected: String, actual: String },
    #[error("文件映射解析失败: {0}")]
    BadMap(String),
    #[error("另一个进程正在写这个对象库（{0} 存在）")]
    Locked(String),
    #[error("索引行坏了：{0}")]
    BadIndex(String),
}

/// 对象库：`objects/pack-000001.pack` + `objects/index.jsonl`。
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    chunking: Chunking,
    index: RefCell<HashMap<Hash, PackEntry>>,
    pack_len: RefCell<u64>,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        let store = Self {
            root,
            chunking: Chunking::default_4k(),
            index: RefCell::new(HashMap::new()),
            pack_len: RefCell::new(0),
        };
        store.load()?;
        Ok(store)
    }

    fn objects(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn pack_path(&self) -> PathBuf {
        self.objects().join("pack-000001.pack")
    }

    fn index_path(&self) -> PathBuf {
        self.objects().join("index.jsonl")
    }

    fn load(&self) -> Result<(), StoreError> {
        self.index.borrow_mut().clear();
        if self.index_path().is_file() {
            let text = fs::read_to_string(self.index_path())?;
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let row: (String, PackEntry) =
                    serde_json::from_str(line).map_err(|e| StoreError::BadIndex(e.to_string()))?;
                let hash = Hash::from_hex(&row.0)
                    .ok_or_else(|| StoreError::BadIndex(format!("不是哈希：{}", row.0)))?;
                self.index.borrow_mut().insert(hash, row.1);
            }
        }
        *self.pack_len.borrow_mut() = fs::metadata(self.pack_path()).map(|m| m.len()).unwrap_or(0);
        Ok(())
    }

    pub fn with_chunking(mut self, chunking: Chunking) -> Self {
        self.chunking = chunking;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn chunking(&self) -> Chunking {
        self.chunking
    }

    pub fn has(&self, hash: &Hash) -> bool {
        self.index.borrow().contains_key(hash)
    }

    fn lock(&self) -> Result<PathBuf, StoreError> {
        let path = self.objects().join("pack.lock");
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                Ok(path)
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(StoreError::Locked(path.display().to_string()))
            }
            Err(err) => Err(StoreError::Io(err)),
        }
    }

    /// 写块：压缩后追加进 pack，再追加一行索引。同名对象已存在就直接复用。
    pub fn put_chunk(&self, bytes: &[u8]) -> Result<(Hash, bool), StoreError> {
        self.put_chunk_with_base(bytes, None)
    }

    /// 写块，`base` 给一个"上一版同位置块"的提示：更小就存增量，否则存整块。
    pub fn put_chunk_with_base(
        &self,
        bytes: &[u8],
        base: Option<Hash>,
    ) -> Result<(Hash, bool), StoreError> {
        let hash = Hash::of(bytes);
        if self.index.borrow().contains_key(&hash) {
            return Ok((hash, false));
        }
        let lock = self.lock()?;
        let result = (|| -> Result<(), StoreError> {
            let full = compress(bytes)?;
            let mut plan = PackPlan::full(full);
            if let Some(base_hash) = base
                && let Some(delta) = self.try_delta(bytes, base_hash, plan.payload.len() as u64)?
            {
                plan = delta;
            }
            let (offset, len) = self.append_payload(&plan.payload)?;
            let entry = PackEntry {
                offset,
                len,
                kind: plan.kind,
                base_off: plan.base_off,
                base_len: plan.base_len,
                prefix: plan.prefix,
                suffix: plan.suffix,
            };
            self.append_index(&hash, entry)
        })();
        let _ = fs::remove_file(&lock);
        result?;
        Ok((hash, true))
    }

    /// 只有"确实更小"才用增量；base 读不出来或被改过就退回整块。
    fn try_delta(
        &self,
        bytes: &[u8],
        base_hash: Hash,
        full_len: u64,
    ) -> Result<Option<PackPlan>, StoreError> {
        let base_entry = match self.index.borrow().get(&base_hash).copied() {
            Some(entry) => entry,
            None => return Ok(None),
        };
        if base_entry.is_delta() {
            // 单层增量：不让链变长（读侧还有 4 层保险）
            return Ok(None);
        }
        let base = match self.get_chunk(&base_hash) {
            Ok(base) => base,
            Err(_) => return Ok(None),
        };
        let prefix = common_prefix(&base, bytes);
        let suffix = common_suffix(&base[prefix..], &bytes[prefix..]);
        if prefix + suffix >= bytes.len() {
            // 整块都是公共前后缀（理论上哈希会相同，早就在 dedup 分支返回）
            return Ok(None);
        }
        let middle = &bytes[prefix..bytes.len() - suffix];
        let delta = compress(middle)?;
        if (delta.len() as u64) >= full_len / 2 {
            return Ok(None);
        }
        Ok(Some(PackPlan {
            payload: delta,
            kind: 1,
            base_off: base_entry.offset,
            base_len: base_entry.len,
            prefix: prefix as u32,
            suffix: suffix as u32,
        }))
    }

    fn append_payload(&self, payload: &[u8]) -> Result<(u64, u64), StoreError> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.pack_path())?;
        let offset = file.metadata()?.len();
        file.write_all(&(payload.len() as u32).to_le_bytes())?;
        file.write_all(payload)?;
        file.sync_data()?;
        Ok((offset, payload.len() as u64))
    }

    fn append_index(&self, hash: &Hash, entry: PackEntry) -> Result<(), StoreError> {
        let line = serde_json::to_string(&(hash.to_hex(), entry))
            .map_err(|e| StoreError::BadIndex(e.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.index_path())?;
        writeln!(file, "{line}")?;
        file.sync_data()?;
        self.index.borrow_mut().insert(*hash, entry);
        Ok(())
    }

    /// 块在不在本地（只查索引，不读盘、不校验内容）。
    pub fn has_chunk(&self, hash: &Hash) -> bool {
        self.index.borrow().contains_key(hash)
    }

    /// 读块并**校验**：内容与名字对不上就报 Corrupt，而不是把坏数据交出去。
    pub fn get_chunk(&self, hash: &Hash) -> Result<Vec<u8>, StoreError> {
        let bytes = self.read_at(hash, 0)?;
        let actual = Hash::of(&bytes);
        if actual != *hash {
            return Err(StoreError::Corrupt {
                expected: hash.to_hex(),
                actual: actual.to_hex(),
            });
        }
        Ok(bytes)
    }

    fn read_at(&self, hash: &Hash, depth: u32) -> Result<Vec<u8>, StoreError> {
        let entry = self
            .index
            .borrow()
            .get(hash)
            .copied()
            .ok_or_else(|| StoreError::Missing(hash.to_hex()))?;
        let body = self.read_payload(entry.offset, entry.len)?;
        if !entry.is_delta() {
            return Ok(body);
        }
        let base = self.read_by_offset(entry.base_off, entry.base_len, depth + 1)?;
        apply_delta(&base, &body, entry.prefix, entry.suffix)
    }

    fn read_payload(&self, offset: u64, len: u64) -> Result<Vec<u8>, StoreError> {
        let mut file = fs::File::open(self.pack_path())?;
        file.seek(SeekFrom::Start(offset + 4))?;
        let mut payload = vec![0u8; len as usize];
        file.read_exact(&mut payload)?;
        decompress(&payload)
    }

    /// 按 pack 偏移读一条记录（delta 的 base 用偏移引用，索引因此更小）。
    fn read_by_offset(&self, offset: u64, len: u64, depth: u32) -> Result<Vec<u8>, StoreError> {
        if depth > 4 {
            return Err(StoreError::BadMap("delta 链过深".to_string()));
        }
        let body = self.read_payload(offset, len)?;
        let entry = self
            .index
            .borrow()
            .values()
            .find(|candidate| candidate.offset == offset)
            .copied();
        match entry {
            Some(entry) if entry.is_delta() => {
                let base = self.read_by_offset(entry.base_off, entry.base_len, depth + 1)?;
                apply_delta(&base, &body, entry.prefix, entry.suffix)
            }
            _ => Ok(body),
        }
    }

    /// 把一个文件按内容切块入库，返回文件映射对象的 id 与本次写入明细。
    pub fn put_file(&self, bytes: &[u8]) -> Result<(Hash, PutStats), StoreError> {
        self.put_file_with_prev(bytes, None)
    }

    /// 同 `put_file`，但给上一版的文件映射 id：同位置的新块会试着对上一版块存增量。
    pub fn put_file_with_prev(
        &self,
        bytes: &[u8],
        prev: Option<Hash>,
    ) -> Result<(Hash, PutStats), StoreError> {
        let prev_chunks = prev
            .and_then(|id| self.file_map(&id).ok())
            .map(|map| map.chunks)
            .unwrap_or_default();
        let mut stats = PutStats::default();
        let mut chunks = Vec::new();
        for (index, (start, end)) in self.chunking.split(bytes).into_iter().enumerate() {
            let base = prev_chunks.get(index).copied();
            let (hash, is_new) = self.put_chunk_with_base(&bytes[start..end], base)?;
            if is_new {
                stats.new_chunks += 1;
                stats.bytes_written += (end - start) as u64;
            } else {
                stats.reused_chunks += 1;
            }
            chunks.push(hash);
        }
        let map = FileMap {
            size: bytes.len() as u64,
            chunks,
        };
        let encoded = serde_json::to_vec(&map).map_err(|e| StoreError::BadMap(e.to_string()))?;
        let (id, _) = self.put_chunk(&encoded)?;
        Ok((id, stats))
    }

    /// 只算"这个内容入库后会是哪个 id"，**不写盘**：`status` 靠它和工作树比对。
    pub fn file_id_bytes(&self, bytes: &[u8]) -> Result<Hash, StoreError> {
        let mut chunks = Vec::new();
        for (start, end) in self.chunking.split(bytes) {
            chunks.push(Hash::of(&bytes[start..end]));
        }
        let map = FileMap {
            size: bytes.len() as u64,
            chunks,
        };
        let encoded = serde_json::to_vec(&map).map_err(|e| StoreError::BadMap(e.to_string()))?;
        Ok(Hash::of(&encoded))
    }

    pub fn file_map(&self, id: &Hash) -> Result<FileMap, StoreError> {
        let bytes = self.get_chunk(id)?;
        serde_json::from_slice(&bytes).map_err(|e| StoreError::BadMap(e.to_string()))
    }

    pub fn get_file(&self, id: &Hash) -> Result<Vec<u8>, StoreError> {
        let map = self.file_map(id)?;
        let mut out = Vec::with_capacity(map.size as usize);
        for chunk in &map.chunks {
            out.extend_from_slice(&self.get_chunk(chunk)?);
        }
        Ok(out)
    }

    pub fn stats(&self) -> Result<Stats, StoreError> {
        let pack = fs::metadata(self.pack_path()).map(|m| m.len()).unwrap_or(0);
        let index = fs::metadata(self.index_path())
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(Stats {
            objects: self.index.borrow().len() as u64,
            bytes_stored: pack + index,
        })
    }

    /// 全库体检：每条索引都能读出来、内容哈希都对得上、pack 没被截断。
    pub fn fsck(&self) -> Result<usize, Vec<String>> {
        let mut checked = 0usize;
        let mut bad = Vec::new();
        let pack_size = fs::metadata(self.pack_path()).map(|m| m.len()).unwrap_or(0);
        let snapshot: Vec<(Hash, PackEntry)> =
            self.index.borrow().iter().map(|(h, e)| (*h, *e)).collect();
        for (hash, entry) in &snapshot {
            if entry.offset + entry.len + 4 > pack_size {
                bad.push(format!(
                    "{} 越界（pack 只有 {} 字节）",
                    hash.short(),
                    pack_size
                ));
                continue;
            }
            match self.get_chunk(hash) {
                Ok(_) => checked += 1,
                Err(err) => bad.push(format!("{}: {err}", hash.short())),
            }
        }
        if bad.is_empty() {
            Ok(checked)
        } else {
            Err(bad)
        }
    }

    /// 目录快照：路径 → 文件映射 id。
    pub fn put_tree(
        &self,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(Hash, PutStats), StoreError> {
        let mut stats = PutStats::default();
        let mut entries = BTreeMap::new();
        for (path, bytes) in files {
            let (id, detail) = self.put_file(bytes)?;
            stats.new_chunks += detail.new_chunks;
            stats.reused_chunks += detail.reused_chunks;
            stats.bytes_written += detail.bytes_written;
            entries.insert(path.clone(), id);
        }
        let encoded =
            serde_json::to_vec(&entries).map_err(|e| StoreError::BadMap(e.to_string()))?;
        let (id, _) = self.put_chunk(&encoded)?;
        Ok((id, stats))
    }

    pub fn tree_entries(&self, id: &Hash) -> Result<BTreeMap<String, Hash>, StoreError> {
        let bytes = self.get_chunk(id)?;
        serde_json::from_slice(&bytes).map_err(|e| StoreError::BadMap(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trit-vcs-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// 像真文本的数据：带随机扰动，避免周期性输入把 gear 哈希也弄成周期（切点会不触发）。
    fn realistic(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed | 1;
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.push(if i % 61 == 0 {
                b'\n'
            } else {
                b'a' + (state % 26) as u8
            });
        }
        out
    }

    #[test]
    fn roundtrip_and_stats() {
        let root = temp("roundtrip");
        let store = Store::open(&root).unwrap();
        let bytes = realistic(50_000, 7);
        let (id, stats) = store.put_file(&bytes).unwrap();
        assert_eq!(store.get_file(&id).unwrap(), bytes);
        assert!(
            stats.new_chunks > 1,
            "50 KiB 应切成多块，实际 {}",
            stats.new_chunks
        );
        assert_eq!(stats.reused_chunks, 0);
        assert!(store.stats().unwrap().bytes_stored > 0);
        assert!(store.fsck().is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reopening_keeps_the_index() {
        let root = temp("reopen");
        let bytes = realistic(20_000, 3);
        let (id, _) = {
            let store = Store::open(&root).unwrap();
            store.put_file(&bytes).unwrap()
        };
        let store = Store::open(&root).unwrap();
        assert_eq!(store.get_file(&id).unwrap(), bytes);
        let (_, stats) = store.put_file(&bytes).unwrap();
        assert_eq!(stats.new_chunks, 0, "重开后同样内容仍是复用");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn identical_content_writes_nothing_new() {
        let root = temp("dedup");
        let store = Store::open(&root).unwrap();
        let bytes = realistic(30_000, 11);
        let _ = store.put_file(&bytes).unwrap();
        let before = store.stats().unwrap();
        let (_, stats) = store.put_file(&bytes).unwrap();
        assert_eq!(stats.new_chunks, 0, "同样内容不该写任何新块");
        assert_eq!(stats.bytes_written, 0);
        assert_eq!(store.stats().unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn inserting_middle_reuses_most_chunks() {
        let root = temp("insert");
        let store = Store::open(&root).unwrap();
        let base = realistic(200_000, 13);
        let _ = store.put_file(&base).unwrap();
        let mut inserted = base.clone();
        inserted.splice(90_000..90_000, vec![b'x'; 64]);
        let (_, stats) = store.put_file(&inserted).unwrap();
        assert!(
            stats.reuse_ratio() > 0.5,
            "中间插 64 字节后应复用大半块，实际复用率 {:.2}",
            stats.reuse_ratio()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn compression_actually_shrinks_text() {
        let root = temp("compress");
        let store = Store::open(&root).unwrap();
        let bytes = realistic(200_000, 17);
        let _ = store.put_file(&bytes).unwrap();
        let stored = store.stats().unwrap().bytes_stored;
        assert!(
            stored < bytes.len() as u64,
            "文本应被压缩：原 {} 字节，库 {} 字节",
            bytes.len(),
            stored
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tampered_pack_is_detected_on_read_and_fsck() {
        let root = temp("tamper");
        let store = Store::open(&root).unwrap();
        let (id, _) = store.put_file(b"hello world").unwrap();
        let map = store.file_map(&id).unwrap();
        let entry = store.index.borrow().get(&map.chunks[0]).copied().unwrap();
        let pack = store.pack_path();
        let mut bytes = fs::read(&pack).unwrap();
        let at = entry.offset as usize + 4;
        bytes[at] ^= 0xFF;
        fs::write(&pack, &bytes).unwrap();
        assert!(store.get_file(&id).is_err(), "改一位就必须读失败");
        assert!(store.fsck().is_err(), "fsck 必须报出来");
        let _ = fs::remove_dir_all(&root);
    }

    /// 索引被改到"另一个合法块"上：zlib 能解、但内容哈希对不上 ⇒ 必须 Corrupt。
    /// 这条专钉"读时不校验哈希"——只翻一个字节的话 zlib 自己就会报错，考验不出哈希校验。
    #[test]
    fn index_pointing_at_another_chunk_is_corrupt() {
        let root = temp("misindex");
        let store = Store::open(&root).unwrap();
        let (a, _) = store.put_file(&realistic(5_000, 23)).unwrap();
        let _ = store.put_file(&realistic(6_000, 29)).unwrap();
        let a_chunk = store.file_map(&a).unwrap().chunks[0];
        let other = {
            let index = store.index.borrow();
            index
                .iter()
                .find(|(hash, _)| **hash != a_chunk)
                .map(|(hash, entry)| (*hash, *entry))
                .unwrap()
        };
        // 把 a_chunk 的索引条目改指向另一个块的字节
        let path = store.index_path();
        let mut lines: Vec<String> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        for line in lines.iter_mut() {
            if let Ok((hex, _)) = serde_json::from_str::<(String, PackEntry)>(line)
                && hex == a_chunk.to_hex()
            {
                let patched = PackEntry {
                    offset: other.1.offset,
                    len: other.1.len,
                    kind: 0,
                    base_off: 0,
                    base_len: 0,
                    prefix: 0,
                    suffix: 0,
                };
                *line = serde_json::to_string(&(hex, patched)).unwrap();
            }
        }
        fs::write(
            &path,
            lines.join(
                "
",
            ) + "
",
        )
        .unwrap();
        let reopened = Store::open(&root).unwrap();
        match reopened.get_chunk(&a_chunk) {
            Err(StoreError::Corrupt { .. }) => {}
            other_err => panic!("期望 Corrupt（索引指错块），得到 {other_err:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn truncated_pack_is_reported() {
        let root = temp("truncate");
        let store = Store::open(&root).unwrap();
        let (_, _) = store.put_file(&realistic(10_000, 19)).unwrap();
        let pack = store.pack_path();
        let bytes = fs::read(&pack).unwrap();
        fs::write(&pack, &bytes[..bytes.len() / 2]).unwrap();
        let bad = store.fsck().unwrap_err();
        assert!(
            bad.iter().any(|line| line.contains("越界")),
            "截断必须点名越界，而不是只报解压失败：{bad:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// 局部修改 ⇒ 至少有一个块存成增量，且读回来逐字节一致。
    #[test]
    fn delta_is_used_for_a_local_edit() {
        let root = temp("delta");
        let store = Store::open(&root).unwrap();
        let base = realistic(60_000, 31);
        let (first, _) = store.put_file(&base).unwrap();
        let mut edited = base.clone();
        edited.splice(
            30_000..30_000,
            b"-- inserted line --
"
            .to_vec(),
        );
        let (second, _) = store.put_file_with_prev(&edited, Some(first)).unwrap();
        assert_eq!(store.get_file(&second).unwrap(), edited);
        let deltas = store
            .index
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.is_delta())
            .count();
        assert!(deltas > 0, "局部修改应至少有一个增量块");
        let _ = fs::remove_dir_all(&root);
    }

    /// 与上一版完全无关的内容不该硬套增量（套了反而更大）。
    #[test]
    fn delta_is_not_used_for_unrelated_content() {
        let root = temp("delta-unrelated");
        let store = Store::open(&root).unwrap();
        let (first, _) = store.put_file(&realistic(30_000, 37)).unwrap();
        let (second, _) = store
            .put_file_with_prev(&realistic(30_000, 41), Some(first))
            .unwrap();
        assert!(store.get_file(&second).is_ok());
        let deltas = store
            .index
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.is_delta())
            .count();
        assert_eq!(deltas, 0, "无关内容不该用增量");
        let _ = fs::remove_dir_all(&root);
    }

    /// 增量块的 base 被改坏时，读它必须报错（不能悄悄返回错内容）。
    #[test]
    fn corrupt_base_makes_delta_unreadable() {
        let root = temp("delta-corrupt-base");
        let store = Store::open(&root).unwrap();
        let base = realistic(40_000, 43);
        let (first, _) = store.put_file(&base).unwrap();
        let mut edited = base.clone();
        edited.splice(20_000..20_000, b"x".to_vec());
        let (second, _) = store.put_file_with_prev(&edited, Some(first)).unwrap();
        // 把 base 的那个块改坏
        let base_chunk = store.file_map(&first).unwrap().chunks[0];
        let entry = store.index.borrow().get(&base_chunk).copied().unwrap();
        let pack = store.pack_path();
        let mut bytes = fs::read(&pack).unwrap();
        bytes[entry.offset as usize + 4] ^= 0xFF;
        fs::write(&pack, bytes).unwrap();
        assert!(
            store.get_file(&second).is_err(),
            "base 坏了，增量块也不许读出内容"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tree_snapshot_roundtrip() {
        let root = temp("tree");
        let store = Store::open(&root).unwrap();
        let mut files = BTreeMap::new();
        files.insert("a.txt".to_string(), b"alpha".to_vec());
        files.insert("dir/b.txt".to_string(), b"beta".to_vec());
        let (id, _) = store.put_tree(&files).unwrap();
        let entries = store.tree_entries(&id).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            store.get_file(entries.get("dir/b.txt").unwrap()).unwrap(),
            b"beta"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
