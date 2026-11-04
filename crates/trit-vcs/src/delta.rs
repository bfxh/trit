//! 索引记录与增量（delta）原语：整块 / 相对上一版的增量。
//!
//! 单独成文件是为了守住"单文件 800 行"的模块预算；判定与编码都在这里，store 只做 IO 与编排。

use std::io::Write;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::{Deserialize, Serialize};
use std::io::Read;

use crate::store::StoreError;

/// 索引里的一条：块在 pack 里的位置与形态。
///
/// `kind=0` 整块（zlib）；`kind=1` 相对另一块的增量（前后缀相同、中间是新内容）。
/// `base_off/base_len` 指向 base 记录自身而不是它的哈希：省 64 字节/条，索引不膨胀。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PackEntry {
    pub(crate) offset: u64,
    pub(crate) len: u64,
    pub(crate) kind: u8,
    pub(crate) base_off: u64,
    pub(crate) base_len: u64,
    pub(crate) prefix: u32,
    pub(crate) suffix: u32,
}

impl PackEntry {
    pub(crate) fn is_delta(&self) -> bool {
        self.kind == 1
    }
}

/// 一条记录写进 pack 之前的形态（整块或增量）。
pub(crate) struct PackPlan {
    pub(crate) payload: Vec<u8>,
    pub(crate) kind: u8,
    pub(crate) base_off: u64,
    pub(crate) base_len: u64,
    pub(crate) prefix: u32,
    pub(crate) suffix: u32,
}

impl PackPlan {
    pub(crate) fn full(payload: Vec<u8>) -> Self {
        let len = payload.len() as u64;
        Self {
            payload,
            kind: 0,
            base_off: 0,
            base_len: len,
            prefix: 0,
            suffix: 0,
        }
    }
}

pub(crate) fn compress(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

pub(crate) fn decompress(payload: &[u8]) -> Result<Vec<u8>, StoreError> {
    let mut decoder = ZlibDecoder::new(payload);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

pub(crate) fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

pub(crate) fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

pub(crate) fn apply_delta(
    base: &[u8],
    body: &[u8],
    prefix: u32,
    suffix: u32,
) -> Result<Vec<u8>, StoreError> {
    let prefix = prefix as usize;
    let suffix = suffix as usize;
    if prefix > base.len() || suffix > base.len() - prefix {
        return Err(StoreError::BadMap(
            "delta 的 prefix/suffix 越界".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(prefix + body.len() + suffix);
    out.extend_from_slice(&base[..prefix]);
    out.extend_from_slice(body);
    out.extend_from_slice(&base[base.len() - suffix..]);
    Ok(out)
}
