#![forbid(unsafe_code)]
//! Content-addressed chunks, versioned manifests, and hash-chain verification.
//!
//! Chunks are stored by SHA-256 under `<root>/chunks/<aa>/<rest>`. Manifests
//! are themselves chunks: a manifest lists named entries whose values are
//! chunk hashes, and points at its parent manifest. The parent chain is the
//! audit trail: every manifest is immutable and content-addressed, so
//! rewriting one breaks its hash and every later lookup.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash([u8; 32]);

impl Hash {
    /// Hashes bytes with SHA-256.
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// Raw digest.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).expect("nibble"));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("nibble"));
        }
        out
    }

    /// Parses 64 lowercase/uppercase hex characters.
    pub fn from_hex(text: &str) -> Result<Self, CasError> {
        if text.len() != 64 {
            return Err(CasError::Hex(format!(
                "hash must be 64 hex characters, got {}",
                text.len()
            )));
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in text.as_bytes().chunks(2).enumerate() {
            let high = hex_nibble(pair[0])
                .ok_or_else(|| CasError::Hex(format!("invalid hex at {index}")))?;
            let low = hex_nibble(pair[1])
                .ok_or_else(|| CasError::Hex(format!("invalid hex at {index}")))?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.to_hex())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.to_hex())
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Structured CAS errors.
#[derive(Debug)]
pub enum CasError {
    /// Filesystem error.
    Io(std::io::Error),
    /// Stored bytes do not match the requested hash.
    CorruptHash {
        /// Requested hash.
        expected: Hash,
        /// Hash of the bytes that were actually read.
        actual: Hash,
    },
    /// Manifest bytes are malformed.
    BadManifest {
        /// Human-readable reason.
        reason: &'static str,
    },
    /// Hash is absent from the store.
    NotFound {
        /// Missing hash.
        hash: Hash,
    },
    /// The parent pointers form a cycle.
    Cycle {
        /// Hash that repeated.
        hash: Hash,
    },
    /// Invalid hex input.
    Hex(String),
}

impl fmt::Display for CasError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CasError::Io(error) => write!(formatter, "cas io error: {error}"),
            CasError::CorruptHash { expected, actual } => {
                write!(formatter, "chunk {expected} hashes to {actual}")
            }
            CasError::BadManifest { reason } => write!(formatter, "bad manifest: {reason}"),
            CasError::NotFound { hash } => write!(formatter, "chunk {hash} not found"),
            CasError::Cycle { hash } => write!(formatter, "manifest chain cycles at {hash}"),
            CasError::Hex(reason) => write!(formatter, "invalid hex: {reason}"),
        }
    }
}

impl std::error::Error for CasError {}

impl From<std::io::Error> for CasError {
    fn from(error: std::io::Error) -> Self {
        CasError::Io(error)
    }
}

/// Content-addressed chunk store.
pub struct ChunkStore {
    root: PathBuf,
}

impl ChunkStore {
    /// Opens or creates a store rooted at `root`.
    pub fn open(root: &Path) -> Result<Self, CasError> {
        fs::create_dir_all(root.join("chunks"))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Store root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, hash: &Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.root.join("chunks").join(&hex[..2]).join(&hex[2..])
    }

    /// Stores bytes and returns their hash. Existing chunks are deduplicated.
    pub fn put(&self, bytes: &[u8]) -> Result<Hash, CasError> {
        let hash = Hash::of(bytes);
        let path = self.path_for(&hash);
        if path.is_file() {
            return Ok(hash);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(hash)
    }

    /// Reads a chunk and verifies its hash.
    pub fn get(&self, hash: &Hash) -> Result<Vec<u8>, CasError> {
        let path = self.path_for(hash);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CasError::NotFound { hash: *hash })
            }
            Err(error) => return Err(error.into()),
        };
        let actual = Hash::of(&bytes);
        if actual != *hash {
            return Err(CasError::CorruptHash {
                expected: *hash,
                actual,
            });
        }
        Ok(bytes)
    }

    /// Returns `true` when the chunk exists.
    pub fn has(&self, hash: &Hash) -> bool {
        self.path_for(hash).is_file()
    }
}

/// Immutable, versioned manifest. Its parent pointer is the audit link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Format version (currently `0`).
    pub version: u8,
    /// Parent manifest hash; `None` for the root.
    pub parent: Option<Hash>,
    /// Creation time in Unix milliseconds.
    pub created_ms: u64,
    /// Named entries, sorted for deterministic encoding.
    pub entries: BTreeMap<String, Hash>,
}

impl Manifest {
    /// Creates a manifest with the current timestamp.
    pub fn new(parent: Option<Hash>, entries: BTreeMap<String, Hash>) -> Self {
        let created_ms = now_ms();
        Self {
            version: 0,
            parent,
            created_ms,
            entries,
        }
    }

    /// Deterministic byte encoding.
    pub fn encode(&self) -> Result<Vec<u8>, CasError> {
        let mut out = Vec::new();
        out.extend_from_slice(b"TRMF");
        out.push(self.version);
        out.extend_from_slice(&[0u8; 3]);
        match self.parent {
            Some(parent) => out.extend_from_slice(parent.as_bytes()),
            None => out.extend_from_slice(&[0u8; 32]),
        }
        out.extend_from_slice(&self.created_ms.to_le_bytes());
        let count = u32::try_from(self.entries.len()).map_err(|_| CasError::BadManifest {
            reason: "too many entries",
        })?;
        out.extend_from_slice(&count.to_le_bytes());
        for (name, hash) in &self.entries {
            let len = u32::try_from(name.len()).map_err(|_| CasError::BadManifest {
                reason: "entry name too long",
            })?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(hash.as_bytes());
        }
        Ok(out)
    }

    /// Parses a manifest and rejects malformed input.
    pub fn decode(bytes: &[u8]) -> Result<Self, CasError> {
        if bytes.len() < 52 {
            return Err(CasError::BadManifest {
                reason: "truncated header",
            });
        }
        if &bytes[0..4] != b"TRMF" {
            return Err(CasError::BadManifest {
                reason: "bad magic",
            });
        }
        let version = bytes[4];
        if version != 0 {
            return Err(CasError::BadManifest {
                reason: "unsupported version",
            });
        }
        if bytes[5..8] != [0, 0, 0] {
            return Err(CasError::BadManifest {
                reason: "reserved bytes must be zero",
            });
        }
        let mut parent_bytes = [0u8; 32];
        parent_bytes.copy_from_slice(&bytes[8..40]);
        let parent = if parent_bytes == [0u8; 32] {
            None
        } else {
            Some(Hash(parent_bytes))
        };
        let mut created = [0u8; 8];
        created.copy_from_slice(&bytes[40..48]);
        let created_ms = u64::from_le_bytes(created);
        let mut count_bytes = [0u8; 4];
        count_bytes.copy_from_slice(&bytes[48..52]);
        let count = u32::from_le_bytes(count_bytes) as usize;

        let mut offset = 52usize;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let len_end = offset.checked_add(4).ok_or(CasError::BadManifest {
                reason: "entry length overflow",
            })?;
            let len_bytes = bytes.get(offset..len_end).ok_or(CasError::BadManifest {
                reason: "truncated entry length",
            })?;
            let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
                as usize;
            offset = len_end;
            let name_end = offset.checked_add(len).ok_or(CasError::BadManifest {
                reason: "entry name overflow",
            })?;
            let name_bytes = bytes.get(offset..name_end).ok_or(CasError::BadManifest {
                reason: "truncated entry name",
            })?;
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| CasError::BadManifest {
                    reason: "entry name is not utf-8",
                })?
                .to_string();
            offset = name_end;
            let hash_end = offset.checked_add(32).ok_or(CasError::BadManifest {
                reason: "entry hash overflow",
            })?;
            let hash_bytes = bytes.get(offset..hash_end).ok_or(CasError::BadManifest {
                reason: "truncated entry hash",
            })?;
            let mut hash = [0u8; 32];
            hash.copy_from_slice(hash_bytes);
            offset = hash_end;
            entries.insert(name, Hash(hash));
        }
        if offset != bytes.len() {
            return Err(CasError::BadManifest {
                reason: "trailing bytes",
            });
        }
        Ok(Self {
            version,
            parent,
            created_ms,
            entries,
        })
    }

    /// Stores the encoded manifest and returns its hash.
    pub fn store(&self, store: &ChunkStore) -> Result<Hash, CasError> {
        store.put(&self.encode()?)
    }

    /// Loads and decodes a manifest.
    pub fn load(store: &ChunkStore, hash: &Hash) -> Result<Self, CasError> {
        Self::decode(&store.get(hash)?)
    }
}

/// Result of a garbage-collection pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GcReport {
    /// Chunks reachable from the given manifest roots.
    pub reachable: usize,
    /// Chunk files removed.
    pub removed: usize,
    /// Bytes removed.
    pub bytes_removed: u64,
}

/// Result of copying a manifest chain to another store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplicaInfo {
    /// Copied head manifest.
    pub head: Hash,
    /// Manifests copied.
    pub manifests: usize,
    /// Non-manifest chunks copied.
    pub chunks: usize,
    /// Bytes copied.
    pub bytes: u64,
}

impl ChunkStore {
    /// Removes chunks that are not reachable from `roots`.
    ///
    /// `roots` must be manifest hashes. Each root is walked back through its
    /// parent chain; every manifest and every entry chunk is kept.
    pub fn gc(&self, roots: &[Hash]) -> Result<GcReport, CasError> {
        let mut reachable = HashSet::new();
        for root in roots {
            self.collect_reachable(root, &mut reachable)?;
        }

        let chunks_dir = self.root.join("chunks");
        let mut removed = 0usize;
        let mut bytes_removed = 0u64;
        if !chunks_dir.is_dir() {
            return Ok(GcReport {
                reachable: reachable.len(),
                removed,
                bytes_removed,
            });
        }
        for prefix_entry in fs::read_dir(&chunks_dir)? {
            let prefix_entry = prefix_entry?;
            if !prefix_entry.file_type()?.is_dir() {
                continue;
            }
            let prefix = prefix_entry.file_name().to_string_lossy().to_string();
            let mut empty = true;
            for file in fs::read_dir(prefix_entry.path())? {
                let file = file?;
                if !file.file_type()?.is_file() {
                    continue;
                }
                empty = false;
                let rest = file.file_name().to_string_lossy().to_string();
                let Ok(hash) = Hash::from_hex(&format!("{prefix}{rest}")) else {
                    continue;
                };
                if reachable.contains(&hash) {
                    continue;
                }
                let len = file.metadata()?.len();
                fs::remove_file(file.path())?;
                removed += 1;
                bytes_removed += len;
            }
            if empty {
                let _ = fs::remove_dir(prefix_entry.path());
            }
        }
        Ok(GcReport {
            reachable: reachable.len(),
            removed,
            bytes_removed,
        })
    }

    fn collect_reachable(&self, head: &Hash, out: &mut HashSet<Hash>) -> Result<(), CasError> {
        let mut current = Some(*head);
        while let Some(hash) = current {
            if !out.insert(hash) {
                break;
            }
            let manifest = Manifest::load(self, &hash)?;
            for entry in manifest.entries.values() {
                out.insert(*entry);
            }
            current = manifest.parent;
        }
        Ok(())
    }
}

/// Copies a manifest chain and its entry chunks into another store.
///
/// The target is verified with [`verify_chain`] after the copy, so a partial
/// or tampered replica fails immediately. A read-only replica is simply a
/// target directory that the caller mounts/uses read-only afterwards.
pub fn replicate(
    source: &ChunkStore,
    target: &ChunkStore,
    head: &Hash,
) -> Result<ReplicaInfo, CasError> {
    let mut manifests = 0usize;
    let mut chunks = 0usize;
    let mut bytes = 0u64;
    let mut seen = HashSet::new();
    let mut current = Some(*head);

    while let Some(hash) = current {
        if !seen.insert(hash) {
            break;
        }
        let data = source.get(&hash)?;
        bytes += data.len() as u64;
        target.put(&data)?;
        manifests += 1;
        let manifest = Manifest::decode(&data)?;
        for entry in manifest.entries.values() {
            if !seen.insert(*entry) {
                continue;
            }
            let data = source.get(entry)?;
            bytes += data.len() as u64;
            target.put(&data)?;
            chunks += 1;
        }
        current = manifest.parent;
    }

    verify_chain(target, head)?;
    Ok(ReplicaInfo {
        head: *head,
        manifests,
        chunks,
        bytes,
    })
}

/// Copies only the named entries of a manifest chain.
///
/// An empty `names` copies every entry. Missing entries on the target are
/// expected: `verify_chain` only needs the manifest chain itself.
pub fn replicate_subset(
    source: &ChunkStore,
    target: &ChunkStore,
    head: &Hash,
    names: &[String],
) -> Result<ReplicaInfo, CasError> {
    let mut manifests = 0usize;
    let mut chunks = 0usize;
    let mut bytes = 0u64;
    let mut seen = HashSet::new();
    let mut current = Some(*head);

    while let Some(hash) = current {
        if !seen.insert(hash) {
            break;
        }
        let data = source.get(&hash)?;
        bytes += data.len() as u64;
        target.put(&data)?;
        manifests += 1;
        let manifest = Manifest::decode(&data)?;
        for (name, entry) in &manifest.entries {
            if !names.is_empty() && !names.iter().any(|wanted| wanted == name) {
                continue;
            }
            if !seen.insert(*entry) {
                continue;
            }
            let data = source.get(entry)?;
            bytes += data.len() as u64;
            target.put(&data)?;
            chunks += 1;
        }
        current = manifest.parent;
    }

    verify_chain(target, head)?;
    Ok(ReplicaInfo {
        head: *head,
        manifests,
        chunks,
        bytes,
    })
}

/// External anchor for a manifest chain head.
///
/// The anchor is stored outside the CAS directory; it records the head hash and
/// an optional HMAC-SHA256 over the anchor bytes. HMAC is symmetric: it proves
/// "someone who knows the key signed this head", not public-key authorship.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// Anchored manifest head.
    pub head: Hash,
    /// Creation time in Unix milliseconds.
    pub created_ms: u64,
    /// Optional HMAC-SHA256 tag.
    pub hmac: Option<[u8; 32]>,
}

impl Anchor {
    /// Creates an unsigned anchor.
    pub fn new(head: Hash) -> Self {
        Self {
            head,
            created_ms: now_ms(),
            hmac: None,
        }
    }

    fn encode_unsigned(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(48);
        out.extend_from_slice(b"TRAN");
        out.push(0);
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(self.head.as_bytes());
        out.extend_from_slice(&self.created_ms.to_le_bytes());
        out
    }

    /// Encodes the anchor (with HMAC when present).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(80);
        out.extend_from_slice(b"TRAN");
        out.push(u8::from(self.hmac.is_some()));
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(self.head.as_bytes());
        out.extend_from_slice(&self.created_ms.to_le_bytes());
        if let Some(tag) = &self.hmac {
            out.extend_from_slice(tag);
        }
        out
    }

    /// Parses an anchor.
    pub fn decode(bytes: &[u8]) -> Result<Self, CasError> {
        if bytes.len() < 48 || &bytes[0..4] != b"TRAN" {
            return Err(CasError::BadManifest {
                reason: "bad anchor header",
            });
        }
        if bytes[5..8] != [0, 0, 0] {
            return Err(CasError::BadManifest {
                reason: "anchor reserved bytes must be zero",
            });
        }
        let mut head = [0u8; 32];
        head.copy_from_slice(&bytes[8..40]);
        let mut created = [0u8; 8];
        created.copy_from_slice(&bytes[40..48]);
        let created_ms = u64::from_le_bytes(created);
        let hmac = if bytes[4] & 1 == 1 {
            if bytes.len() != 80 {
                return Err(CasError::BadManifest {
                    reason: "anchor hmac length",
                });
            }
            let mut tag = [0u8; 32];
            tag.copy_from_slice(&bytes[48..80]);
            Some(tag)
        } else {
            if bytes.len() != 48 {
                return Err(CasError::BadManifest {
                    reason: "anchor trailing bytes",
                });
            }
            None
        };
        Ok(Self {
            head: Hash(head),
            created_ms,
            hmac,
        })
    }

    /// Signs the anchor with HMAC-SHA256.
    pub fn sign(&mut self, key: &[u8]) {
        self.hmac = Some(hmac_sha256(key, &self.encode_unsigned()));
    }

    /// Verifies the HMAC tag.
    pub fn verify(&self, key: &[u8]) -> bool {
        match self.hmac {
            Some(tag) => constant_time_eq(&hmac_sha256(key, &self.encode_unsigned()), &tag),
            None => false,
        }
    }
}

/// Compares two byte strings without short-circuiting on the first mismatch.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    for (a, b) in left.iter().zip(right) {
        diff |= usize::from(a ^ b);
    }
    diff == 0
}

/// HMAC-SHA256 (FIPS 198-1) over the in-crate SHA-256.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend(block.iter().map(|byte| byte ^ 0x36));
    inner.extend_from_slice(message);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(64 + 32);
    outer.extend(block.iter().map(|byte| byte ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

/// Result of walking a manifest hash chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainInfo {
    /// Head manifest.
    pub head: Hash,
    /// Root manifest (no parent).
    pub root: Hash,
    /// Number of manifests in the chain.
    pub length: u64,
}

/// Walks `head` back to the root, verifying every manifest hash.
///
/// A rewritten manifest fails the content-address check in
/// [`ChunkStore::get`]; a parent that does not exist returns `NotFound`; a
/// cycle returns `Cycle`.
pub fn verify_chain(store: &ChunkStore, head: &Hash) -> Result<ChainInfo, CasError> {
    let mut current = *head;
    let mut visited = HashSet::new();
    let mut length = 0u64;
    loop {
        if !visited.insert(current) {
            return Err(CasError::Cycle { hash: current });
        }
        let manifest = Manifest::load(store, &current)?;
        length += 1;
        match manifest.parent {
            Some(parent) => current = parent,
            None => {
                return Ok(ChainInfo {
                    head: *head,
                    root: current,
                    length,
                })
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// SHA-256 (FIPS 180-4). Kept in-crate so the database stays dependency-free.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut message = bytes.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for (index, word) in chunk.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut out = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            Hash::of(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Hash::of(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            Hash::of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").to_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn chunk_store_deduplicates_and_detects_tampering() {
        let dir = tempfile_dir("cas-chunks");
        let store = ChunkStore::open(&dir).expect("open store");
        let first = store.put(b"hello").expect("put");
        let second = store.put(b"hello").expect("put");
        assert_eq!(first, second);
        assert_eq!(store.get(&first).expect("get"), b"hello");
        assert!(store.has(&first));

        let path = dir
            .join("chunks")
            .join(&first.to_hex()[..2])
            .join(&first.to_hex()[2..]);
        fs::write(&path, b"tampered").expect("tamper");
        let error = store.get(&first).expect_err("tamper must fail");
        assert!(matches!(error, CasError::CorruptHash { .. }));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_roundtrips_and_verifies_chain() {
        let dir = tempfile_dir("cas-chain");
        let store = ChunkStore::open(&dir).expect("open store");
        let blob = store.put(b"payload").expect("put payload");

        let mut entries = BTreeMap::new();
        entries.insert("blob".to_string(), blob);
        let root = Manifest::new(None, entries.clone())
            .store(&store)
            .expect("store root");

        let child = Manifest::new(Some(root), entries)
            .store(&store)
            .expect("store child");
        let info = verify_chain(&store, &child).expect("verify chain");
        assert_eq!(info.length, 2);
        assert_eq!(info.root, root);

        let loaded = Manifest::load(&store, &child).expect("load");
        assert_eq!(loaded.parent, Some(root));

        let encoded = loaded.encode().expect("encode");
        assert_eq!(Manifest::decode(&encoded).expect("decode"), loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gc_keeps_reachable_chunks_and_removes_orphans() {
        let dir = tempfile_dir("cas-gc");
        let store = ChunkStore::open(&dir).expect("open");
        let blob = store.put(b"payload").expect("put blob");
        let orphan = store.put(b"orphan").expect("put orphan");

        let mut entries = BTreeMap::new();
        entries.insert("blob".to_string(), blob);
        let root = Manifest::new(None, entries)
            .store(&store)
            .expect("store root");

        let report = store.gc(&[root]).expect("gc");
        assert_eq!(report.removed, 1);
        assert!(store.has(&root));
        assert!(store.has(&blob));
        assert!(!store.has(&orphan));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replicate_copies_the_chain_to_a_read_only_target() {
        let source_dir = tempfile_dir("cas-source");
        let target_dir = tempfile_dir("cas-target");
        let source = ChunkStore::open(&source_dir).expect("open source");
        let target = ChunkStore::open(&target_dir).expect("open target");

        let blob = source.put(b"payload").expect("put blob");
        let mut entries = BTreeMap::new();
        entries.insert("blob".to_string(), blob);
        let root = Manifest::new(None, entries.clone())
            .store(&source)
            .expect("store root");
        let head = Manifest::new(Some(root), entries)
            .store(&source)
            .expect("store head");

        let info = replicate(&source, &target, &head).expect("replicate");
        assert_eq!(info.manifests, 2);
        assert_eq!(info.chunks, 1);
        verify_chain(&target, &head).expect("target verifies");

        // Source GC must not affect the replica.
        source.gc(&[head]).expect("gc source");
        assert!(target.has(&blob));
        verify_chain(&target, &head).expect("replica still verifies");

        let _ = fs::remove_dir_all(&source_dir);
        let _ = fs::remove_dir_all(&target_dir);
    }

    #[test]
    fn subset_replication_copies_only_selected_entries() {
        let source_dir = tempfile_dir("cas-subset-source");
        let target_dir = tempfile_dir("cas-subset-target");
        let source = ChunkStore::open(&source_dir).expect("open source");
        let target = ChunkStore::open(&target_dir).expect("open target");

        let first = source.put(b"first").expect("put first");
        let second = source.put(b"second").expect("put second");
        let mut entries = BTreeMap::new();
        entries.insert("first".to_string(), first);
        entries.insert("second".to_string(), second);
        let head = Manifest::new(None, entries)
            .store(&source)
            .expect("store manifest");

        let info = replicate_subset(&source, &target, &head, &["first".to_string()])
            .expect("subset replicate");
        assert_eq!(info.chunks, 1);
        assert!(target.has(&first));
        assert!(!target.has(&second));
        verify_chain(&target, &head).expect("manifest chain copied");

        let _ = fs::remove_dir_all(&source_dir);
        let _ = fs::remove_dir_all(&target_dir);
    }

    #[test]
    fn constant_time_eq_matches_byte_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn anchor_signs_and_detects_tampering() {
        let head = Hash::of(b"head");
        let mut anchor = Anchor::new(head);
        anchor.sign(b"secret-key");
        let bytes = anchor.encode();
        let decoded = Anchor::decode(&bytes).expect("decode");
        assert!(decoded.verify(b"secret-key"));
        assert!(!decoded.verify(b"wrong-key"));

        let mut tampered = bytes.clone();
        tampered[8] ^= 1;
        let tampered = Anchor::decode(&tampered).expect("decode tampered");
        assert!(!tampered.verify(b"secret-key"));
    }

    #[test]
    fn hmac_matches_rfc4231_vector_2() {
        let tag = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            tag.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn chain_detects_a_missing_parent() {
        let dir = tempfile_dir("cas-missing");
        let store = ChunkStore::open(&dir).expect("open store");
        let missing = Hash::of(b"missing");
        let child = Manifest::new(Some(missing), BTreeMap::new())
            .store(&store)
            .expect("store child");
        let error = verify_chain(&store, &child).expect_err("missing parent");
        assert!(matches!(error, CasError::NotFound { .. }));
        let _ = fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tritdb-{prefix}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
