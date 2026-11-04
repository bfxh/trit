import io

P = r"E:\KF\trit-claims\crates\trit-vcs\src\store.rs"
s = io.open(P, encoding="utf-8").read()
anchor = "    pub fn file_map(&self, id: &Hash) -> Result<FileMap, StoreError> {"
add = '''    /// 只算"这个内容入库后会是哪个 id"，**不写盘**：`status` 靠它和工作树比对。
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

'''
assert anchor in s
s = s.replace(anchor, add + anchor, 1)
io.open(P, "w", encoding="utf-8", newline="\n").write(s)

L = r"E:\KF\trit-claims\crates\trit-vcs\src\lib.rs"
t = io.open(L, encoding="utf-8").read()
if "pub mod worktree;" not in t:
    t = t.replace("pub mod store;", "pub mod store;\npub mod worktree;", 1)
    t = t.replace("pub use store::{FileMap, PutStats, Stats, Store, StoreError};",
                  "pub use store::{FileMap, PutStats, Stats, Store, StoreError};\n"
                  "pub use worktree::{Change, Kind as ChangeKind, WorktreeError};", 1)
io.open(L, "w", encoding="utf-8", newline="\n").write(t)
print("store+lib wired")
