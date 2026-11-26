//! 任务头评测用的句向量落盘：`TRIT_EMBED_DUMP` 目录 + `TRIT_EMBED_TAG` 文件名。
//!
//! 格式：`WSPEMB1\0` + rows:u32 + cols:u32 + row-major f32。
//! 向量就是 STS 的 `sentence_vectors`：mask mean pooling + L2 normalize。

use std::env;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::modes::Request;
use crate::sts::{Pair, metrics, sentence_vectors};

pub(crate) fn vectors_with_dump(
    hidden: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    width: usize,
    tag: &str,
) -> Vec<Vec<f32>> {
    let vectors = sentence_vectors(hidden, mask, batch, seq, width);
    dump(&vectors, tag);
    vectors
}

pub(crate) fn sts_with_dump(
    hidden: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    width: usize,
    pairs: &[Pair],
    tag: &str,
) -> Value {
    let vectors = vectors_with_dump(hidden, mask, batch, seq, width, tag);
    metrics(&vectors, pairs)
}

pub(crate) fn mode_sts(request: &Request, hidden: &[f32], mode_key: &str) -> Value {
    let tag = env::var("TRIT_EMBED_TAG").unwrap_or_else(|_| mode_key.to_string());
    sts_with_dump(
        hidden,
        request.mask,
        request.batch,
        request.seq,
        request.hidden,
        request.pairs,
        &tag,
    )
}

pub(crate) fn pure_sts(
    hidden: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    width: usize,
    pairs: &[Pair],
) -> Value {
    sts_with_dump(hidden, mask, batch, seq, width, pairs, "pure-f32")
}

pub(crate) fn dump(vectors: &[Vec<f32>], tag: &str) {
    let Some(dir) = env::var_os("TRIT_EMBED_DUMP") else {
        return;
    };
    let Some(first) = vectors.first() else {
        return;
    };
    let rows = vectors.len();
    let cols = first.len();
    let mut bytes = Vec::with_capacity(16 + rows * cols * 4);
    bytes.extend_from_slice(b"WSPEMB1\0");
    bytes.extend_from_slice(&(rows as u32).to_le_bytes());
    bytes.extend_from_slice(&(cols as u32).to_le_bytes());
    for vector in vectors {
        assert_eq!(vector.len(), cols, "embedding width");
        for value in vector {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    let safe: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = Path::new(&dir).join(format!("{safe}.f32"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create embed dump dir");
    }
    fs::write(&path, bytes).expect("write embed dump");
}
