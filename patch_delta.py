"""给 trit-vcs store 加 delta 支持（一次性补丁；跑完即删）。"""
import io

P = r"E:\KF\trit-claims\crates\trit-vcs\src\store.rs"
s = io.open(P, encoding="utf-8").read()


def rep(old, new):
    global s
    assert old in s, "锚点丢了：" + old[:80]
    s = s.replace(old, new, 1)


# 1) PackEntry 加 delta 字段
rep(
    """/// 索引里的一条：块在 pack 里的位置与原始长度。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PackEntry {
    offset: u64,
    len: u64,
}""",
    """/// 索引里的一条：块在 pack 里的位置与形态。
///
/// `kind=0` 整块（zlib）；`kind=1` 相对另一块的增量（前后缀相同、中间是新内容）。
/// `base_off/base_len` 指向 base 记录自身而不是它的哈希：省 64 字节/条，索引不膨胀。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PackEntry {
    offset: u64,
    len: u64,
    kind: u8,
    base_off: u64,
    base_len: u64,
    prefix: u32,
    suffix: u32,
}

impl PackEntry {
    fn full(offset: u64, len: u64) -> Self {
        Self { offset, len, kind: 0, base_off: 0, base_len: 0, prefix: 0, suffix: 0 }
    }

    fn is_delta(&self) -> bool {
        self.kind == 1
    }
}

/// 一条记录写进 pack 之前的形态（整块或增量）。
struct PackPlan {
    payload: Vec<u8>,
    kind: u8,
    base_off: u64,
    base_len: u64,
    prefix: u32,
    suffix: u32,
}

impl PackPlan {
    fn full(payload: Vec<u8>) -> Self {
        let len = payload.len() as u64;
        Self { payload, kind: 0, base_off: 0, base_len: len, prefix: 0, suffix: 0 }
    }
}

fn compress(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

fn decompress(payload: &[u8]) -> Result<Vec<u8>, StoreError> {
    let mut decoder = ZlibDecoder::new(payload);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
}

fn apply_delta(base: &[u8], body: &[u8], prefix: u32, suffix: u32) -> Result<Vec<u8>, StoreError> {
    let prefix = prefix as usize;
    let suffix = suffix as usize;
    if prefix > base.len() || suffix > base.len() - prefix {
        return Err(StoreError::BadMap("delta 的 prefix/suffix 越界".to_string()));
    }
    let mut out = Vec::with_capacity(prefix + body.len() + suffix);
    out.extend_from_slice(&base[..prefix]);
    out.extend_from_slice(body);
    out.extend_from_slice(&base[base.len() - suffix..]);
    Ok(out)
}""",
)

# 2) put_chunk -> put_chunk_with_base
rep(
    """    /// 写块：压缩后追加进 pack，再追加一行索引。同名对象已存在就直接复用。
    pub fn put_chunk(&self, bytes: &[u8]) -> Result<(Hash, bool), StoreError> {
        let hash = Hash::of(bytes);
        if self.index.borrow().contains_key(&hash) {
            return Ok((hash, false));
        }
        let lock = self.lock()?;
        let result = (|| -> Result<(), StoreError> {
            let (offset, compressed_len) = self.append_chunk(bytes)?;
            self.append_index(&hash, offset, compressed_len)
        })();
        let _ = fs::remove_file(&lock);
        result?;
        Ok((hash, true))
    }""",
    """    /// 写块：压缩后追加进 pack，再追加一行索引。同名对象已存在就直接复用。
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
    }""",
)

# 3) append_chunk -> append_payload；append_index 接 PackEntry（签名按 cargo fmt 的多行形态）
rep(
    """    fn append_chunk(&self, bytes: &[u8]) -> Result<(u64, u64), StoreError> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes)?;
        let compressed = encoder.finish()?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.pack_path())?;
        let offset = file.metadata()?.len();
        file.write_all(&(compressed.len() as u32).to_le_bytes())?;
        file.write_all(&compressed)?;
        file.sync_data()?;
        Ok((offset, compressed.len() as u64))
    }

    fn append_index(
        &self,
        hash: &Hash,
        offset: u64,
        compressed_len: u64,
    ) -> Result<(), StoreError> {
        let entry = PackEntry {
            offset,
            len: compressed_len,
        };
        let line = serde_json::to_string(&(hash.to_hex(), entry))
            .map_err(|e| StoreError::BadIndex(e.to_string()))?;""",
    """    fn append_payload(&self, payload: &[u8]) -> Result<(u64, u64), StoreError> {
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
            .map_err(|e| StoreError::BadIndex(e.to_string()))?;""",
)

# 4) get_chunk 支持 delta
rep(
    """    /// 读块并**校验**：内容与名字对不上就报 Corrupt，而不是把坏数据交出去。
    pub fn get_chunk(&self, hash: &Hash) -> Result<Vec<u8>, StoreError> {
        let entry = self
            .index
            .borrow()
            .get(hash)
            .copied()
            .ok_or_else(|| StoreError::Missing(hash.to_hex()))?;
        let mut file = fs::File::open(self.pack_path())?;
        file.seek(SeekFrom::Start(entry.offset + 4))?;
        let mut compressed = vec![0u8; entry.len as usize];
        file.read_exact(&mut compressed)?;
        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut bytes = Vec::new();
        decoder.read_to_end(&mut bytes)?;
        let actual = Hash::of(&bytes);
        if actual != *hash {
            return Err(StoreError::Corrupt {
                expected: hash.to_hex(),
                actual: actual.to_hex(),
            });
        }
        Ok(bytes)
    }""",
    """    /// 读块并**校验**：内容与名字对不上就报 Corrupt，而不是把坏数据交出去。
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
    }""",
)

# 5) put_file 支持 prev（上一版文件映射）
rep(
    """    /// 把一个文件按内容切块入库，返回文件映射对象的 id 与本次写入明细。
    pub fn put_file(&self, bytes: &[u8]) -> Result<(Hash, PutStats), StoreError> {
        let mut stats = PutStats::default();
        let mut chunks = Vec::new();
        for (start, end) in self.chunking.split(bytes) {
            let (hash, is_new) = self.put_chunk(&bytes[start..end])?;""",
    """    /// 把一个文件按内容切块入库，返回文件映射对象的 id 与本次写入明细。
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
            let (hash, is_new) = self.put_chunk_with_base(&bytes[start..end], base)?;""",
)
rep(
    """            chunks.push(hash);
        }
        let map = FileMap {""",
    """            chunks.push(hash);
        }
        let map = FileMap {""",
)

io.open(P, "w", encoding="utf-8", newline="\n").write(s)
print("store.rs patched:", len(s.splitlines()), "lines")
