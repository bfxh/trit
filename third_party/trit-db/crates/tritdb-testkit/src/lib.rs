#![forbid(unsafe_code)]
//! Shared test helpers for trit-db. This crate is not part of the product
//! surface: it is `publish = false` and only used from integration tests.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A temporary directory removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a unique directory. `prefix` only makes failure messages
    /// easier to read; uniqueness comes from pid + a process-local counter.
    pub fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "tritdb-{prefix}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// Directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Joins a relative path to the directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Small deterministic PRNG (xorshift64*).
pub struct Rng(u64);

impl Rng {
    /// Creates a PRNG. Zero is replaced with a fixed non-zero seed.
    pub const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    /// Raw 64-bit output.
    pub fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.0 = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// 32-bit output.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform-ish value in `0..bound`. `bound` must be non-zero.
    pub fn below(&mut self, bound: u32) -> u32 {
        assert!(bound > 0, "bound must be non-zero");
        self.next_u32() % bound
    }

    /// A trit value in `0..=2`.
    pub fn trit(&mut self) -> u8 {
        (self.below(3)) as u8
    }

    /// A boolean with the given percentage chance.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
}

/// Lowercase hex encoding.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").expect("write hex");
    }
    out
}

/// Lowercase hex decoding; rejects odd length and non-hex characters.
pub fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    if text.len() & 1 != 0 {
        return Err(format!("odd hex length: {}", text.len()));
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for index in (0..bytes.len()).step_by(2) {
        let hi = hex_nibble(bytes[index])?;
        let lo = hex_nibble(bytes[index + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        other => Err(format!("non-hex byte: {other:#04x}")),
    }
}

/// Reads a UTF-8 file, trimming a trailing newline.
pub fn read_text(path: &Path) -> String {
    let mut text = fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("read {}: {error}", path.display());
    });
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}
