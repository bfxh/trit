//! 内容哈希：BLAKE3（256 位）。
//!
//! 为什么不是 Git 的 SHA-1：SHA-1 的碰撞已经被实际构造（SHAttered），Git 现在只靠
//! 碰撞检测补丁活着；BLAKE3 在 256 位安全强度上同时更快（同机吞吐见 `examples/vcs_bench.rs`）。

use serde::{Deserialize, Serialize};

/// 32 字节内容地址。十六进制小写是它的外部形式。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hash([u8; 32]);

impl Hash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn short(self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short())
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_hex() {
        let hash = Hash::of(b"abc");
        assert_eq!(Hash::from_hex(&hash.to_hex()), Some(hash));
        assert_eq!(hash.to_hex().len(), 64);
        assert_eq!(hash.short().len(), 12);
    }

    #[test]
    fn rejects_bad_hex() {
        assert_eq!(Hash::from_hex(""), None);
        assert_eq!(Hash::from_hex("zz"), None);
        assert_eq!(Hash::from_hex(&"z".repeat(64)), None);
        assert_eq!(Hash::from_hex(&"a".repeat(63)), None);
    }

    #[test]
    fn one_byte_change_changes_hash() {
        assert_ne!(Hash::of(b"abc"), Hash::of(b"abd"));
        assert_ne!(Hash::of(b""), Hash::of(b"\0"));
    }

    /// 钉住 BLAKE3 的已知向量：换实现/换版本时这里先响。
    #[test]
    fn known_vector_empty() {
        assert_eq!(
            Hash::of(b"").to_hex(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }
}
