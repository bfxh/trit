//! 块级传输：按树闭包只传缺的对象，支持目录级部分拉取与断点续传。
//!
//! 包格式（`TVBD`）：头 + 一串对象 `{32 字节 id, u32 长度, 原始内容}`。
//! 对象是内容寻址的原始字节（树、文件映射、数据块一视同仁）；收端**逐块重算 BLAKE3**，
//! 名字对不上就拒写。已存在的块直接跳过——这同时是"断点续传"：中断后重跑只会补缺的。
//!
//! v0 是本地/离线的：`export` 从源库读、`import` 写目标库；网络、压缩协商、并发下载都不在这里。

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};

use crate::hash::Hash;
use crate::store::{Store, StoreError};

pub const MAGIC: &[u8; 4] = b"TVBD";
pub const SCHEMA: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("对象库: {0}")]
    Store(#[from] StoreError),
    #[error("不是 trit-vcs 包（magic 不对）")]
    BadMagic,
    #[error("不支持的包 schema：{0}")]
    UnsupportedSchema(u32),
    #[error("包被截断")]
    Truncated,
    #[error("包里的对象内容与 id 对不上：期望 {expected}，实际 {actual}")]
    Corrupt { expected: String, actual: String },
    #[error("包声明了树 {0}，但包里没有它")]
    MissingTree(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportStats {
    /// 选中的文件数（前缀过滤后）。
    pub files: usize,
    /// 闭包里一共多少个对象（树 + 文件映射 + 数据块，去重后）。
    pub objects: usize,
    /// 实际写进包的对象数（扣掉目标库已有的）。
    pub sent_objects: usize,
    /// 闭包里对象的原始字节总数。
    pub total_bytes: u64,
    /// 实际写进包的原始字节数。
    pub sent_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStats {
    pub received: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub tree: Option<Hash>,
}

fn push_object(order: &mut Vec<Hash>, seen: &mut HashSet<Hash>, hash: Hash) {
    if seen.insert(hash) {
        order.push(hash);
    }
}

/// 选中的树条目：`prefix` 为 `None` 时全量；否则按路径段匹配（`gen` 覆盖 `gen/...`）。
pub fn select_entries(
    store: &Store,
    tree: Hash,
    prefix: Option<&str>,
) -> Result<BTreeMap<String, Hash>, BundleError> {
    let entries = store.tree_entries(&tree)?;
    let Some(prefix) = prefix else {
        return Ok(entries);
    };
    let prefix = prefix.trim_end_matches('/');
    let scoped = format!("{prefix}/");
    Ok(entries
        .into_iter()
        .filter(|(path, _)| path == prefix || path.starts_with(&scoped))
        .collect())
}

/// 把 `tree`（可限定到某个目录）的对象写成包；`dest` 已有的对象不写（块级增量）。
pub fn export<W: Write>(
    store: &Store,
    tree: Hash,
    prefix: Option<&str>,
    dest: Option<&Store>,
    out: &mut W,
) -> Result<ExportStats, BundleError> {
    let entries = select_entries(store, tree, prefix)?;
    // 部分拉取发的是**过滤后的树对象**；全量时它的 id 与原树相同（同一份 JSON 编码）。
    let manifest = serde_json::to_vec(&entries)
        .map_err(|e| BundleError::Store(StoreError::BadMap(e.to_string())))?;
    let manifest_id = Hash::of(&manifest);
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    push_object(&mut order, &mut seen, manifest_id);
    for file_id in entries.values() {
        push_object(&mut order, &mut seen, *file_id);
        for chunk in &store.file_map(file_id)?.chunks {
            push_object(&mut order, &mut seen, *chunk);
        }
    }

    let mut stats = ExportStats {
        files: entries.len(),
        objects: order.len(),
        ..Default::default()
    };
    out.write_all(MAGIC)?;
    out.write_all(&SCHEMA.to_le_bytes())?;
    out.write_all(manifest_id.as_bytes())?;
    out.write_all(&(entries.len() as u32).to_le_bytes())?;
    out.write_all(&(order.len() as u32).to_le_bytes())?;

    for hash in &order {
        let payload = if *hash == manifest_id {
            manifest.clone()
        } else {
            store.get_chunk(hash)?
        };
        stats.total_bytes += payload.len() as u64;
        if dest.is_some_and(|dest| dest.has_chunk(hash)) {
            continue;
        }
        stats.sent_objects += 1;
        stats.sent_bytes += payload.len() as u64;
        out.write_all(hash.as_bytes())?;
        out.write_all(&(payload.len() as u32).to_le_bytes())?;
        out.write_all(&payload)?;
    }
    Ok(stats)
}

/// 从包里收对象；已有块跳过，新块逐个校验后入库。
pub fn import<R: Read>(store: &Store, input: &mut R) -> Result<ImportStats, BundleError> {
    let mut magic = [0u8; 4];
    read_exact(input, &mut magic)?;
    if &magic != MAGIC {
        return Err(BundleError::BadMagic);
    }
    let schema = read_u32(input)?;
    if schema != SCHEMA {
        return Err(BundleError::UnsupportedSchema(schema));
    }
    let mut tree_bytes = [0u8; 32];
    read_exact(input, &mut tree_bytes)?;
    let tree = Hash::from_bytes(tree_bytes);
    let _files = read_u32(input)?;
    let blob_count = read_u32(input)? as usize;

    let mut stats = ImportStats {
        tree: Some(tree),
        ..Default::default()
    };
    for _ in 0..blob_count {
        let mut id_bytes = [0u8; 32];
        read_exact(input, &mut id_bytes)?;
        let id = Hash::from_bytes(id_bytes);
        let len = read_u32(input)? as usize;
        let mut payload = vec![0u8; len];
        read_exact(input, &mut payload)?;
        let actual = Hash::of(&payload);
        if actual != id {
            return Err(BundleError::Corrupt {
                expected: id.to_hex(),
                actual: actual.to_hex(),
            });
        }
        if store.has_chunk(&id) {
            stats.skipped += 1;
            continue;
        }
        store.put_chunk(&payload)?;
        stats.received += 1;
        stats.bytes += payload.len() as u64;
    }
    if !store.has_chunk(&tree) {
        return Err(BundleError::MissingTree(tree.to_hex()));
    }
    Ok(stats)
}

fn read_exact<R: Read>(input: &mut R, buf: &mut [u8]) -> Result<(), BundleError> {
    input.read_exact(buf).map_err(|err| {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            BundleError::Truncated
        } else {
            BundleError::Io(err)
        }
    })
}

fn read_u32<R: Read>(input: &mut R) -> Result<u32, BundleError> {
    let mut buf = [0u8; 4];
    read_exact(input, &mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::gate::ClaimSpec;
    use crate::repo::{CommitRequest, Repo};

    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trit-vcs-bundle-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn corpus(dirs: usize, per_dir: usize) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        for d in 0..dirs {
            for f in 0..per_dir {
                let mut body = String::new();
                for line in 0..8 {
                    body.push_str(&format!(
                        "dir {d} file {f} line {line} value {}\n",
                        d * 97 + f * 13 + line
                    ));
                }
                out.insert(format!("gen/d{d:03}/f{f:03}.txt"), body.into_bytes());
            }
        }
        out
    }

    fn seed(root: &std::path::Path, files: &BTreeMap<String, Vec<u8>>) -> Repo {
        let mut repo = Repo::init(root).unwrap();
        repo.stage(files, 1_000).unwrap();
        repo.commit(CommitRequest {
            message: "seed".to_string(),
            session: "qoder:bundle".to_string(),
            claims: vec![ClaimSpec {
                id: "c1".to_string(),
                owner: "qoder:bundle".to_string(),
                paths: vec!["gen".to_string()],
                acquired_at: 1,
                expires_at: u64::MAX / 2,
            }],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
        repo
    }

    fn head_tree(repo: &Repo) -> Hash {
        let head = repo.head().unwrap().unwrap();
        repo.commit_at(&head).unwrap().tree
    }

    #[test]
    fn partial_pull_sends_well_under_a_tenth() {
        let root = temp("partial");
        let files = corpus(20, 10);
        let src = seed(&root.join("src"), &files);
        let tree = head_tree(&src);

        let mut full = Vec::new();
        let full_stats = export(src.store(), tree, None, None, &mut full).unwrap();
        let mut partial = Vec::new();
        let partial_stats =
            export(src.store(), tree, Some("gen/d000"), None, &mut partial).unwrap();

        assert_eq!(partial_stats.files, 10);
        assert!(
            partial_stats.sent_bytes * 10 < full_stats.sent_bytes,
            "单目录应小于全量 1/10：partial={} full={}",
            partial_stats.sent_bytes,
            full_stats.sent_bytes
        );
        assert!(partial_stats.sent_objects < full_stats.sent_objects);

        let dst = Repo::init(root.join("dst")).unwrap();
        let stats = import(dst.store(), &mut partial.as_slice()).unwrap();
        assert_eq!(stats.received, partial_stats.sent_objects);
        for (path, id) in select_entries(src.store(), tree, Some("gen/d000")).unwrap() {
            assert_eq!(
                dst.store().get_file(&id).unwrap(),
                *files.get(&path).unwrap(),
                "{path} 拉取后应逐字节一致"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn export_skips_objects_the_destination_already_has() {
        let root = temp("incremental");
        let files = corpus(4, 4);
        let src = seed(&root.join("src"), &files);
        let tree = head_tree(&src);
        let dst = Repo::init(root.join("dst")).unwrap();

        let mut first = Vec::new();
        let stats = export(src.store(), tree, None, None, &mut first).unwrap();
        import(dst.store(), &mut first.as_slice()).unwrap();

        let mut second = Vec::new();
        let again = export(src.store(), tree, None, Some(dst.store()), &mut second).unwrap();
        assert_eq!(again.sent_bytes, 0, "目标已有全部对象就不该再传内容");
        assert_eq!(again.sent_objects, 0);
        assert_eq!(again.total_bytes, stats.total_bytes);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn truncated_bundle_resumes_and_ends_byte_identical() {
        let root = temp("resume");
        let files = corpus(5, 8);
        let src = seed(&root.join("src"), &files);
        let tree = head_tree(&src);
        let dst = Repo::init(root.join("dst")).unwrap();

        let mut bundle = Vec::new();
        let full = export(src.store(), tree, None, None, &mut bundle).unwrap();
        let half = bundle.len() / 2;
        assert!(matches!(
            import(dst.store(), &mut &bundle[..half]).unwrap_err(),
            BundleError::Truncated
        ));

        let stats = import(dst.store(), &mut bundle.as_slice()).unwrap();
        assert!(
            stats.received > 0 && stats.skipped > 0,
            "断点续传应只补缺块：{stats:?}"
        );
        assert_eq!(stats.received + stats.skipped, full.objects);

        for (path, bytes) in &files {
            let id = src.store().file_id_bytes(bytes).unwrap();
            assert_eq!(
                dst.store().get_file(&id).unwrap(),
                *bytes,
                "{path} 必须逐位一致"
            );
        }

        let mut third = Vec::new();
        let _ = export(src.store(), tree, None, None, &mut third).unwrap();
        let repeat = import(dst.store(), &mut third.as_slice()).unwrap();
        assert_eq!(repeat.received, 0, "重复导入不产生第二次写入");
        assert_eq!(repeat.skipped, full.objects);
        assert_eq!(repeat.bytes, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tampered_payload_is_rejected_before_storing() {
        let root = temp("tamper");
        let files = corpus(1, 1);
        let src = seed(&root.join("src"), &files);
        let tree = head_tree(&src);
        let dst = Repo::init(root.join("dst")).unwrap();

        let mut bundle = Vec::new();
        export(src.store(), tree, None, None, &mut bundle).unwrap();
        let last = bundle.len() - 1;
        bundle[last] ^= 0xFF;
        assert!(matches!(
            import(dst.store(), &mut bundle.as_slice()).unwrap_err(),
            BundleError::Corrupt { .. }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
