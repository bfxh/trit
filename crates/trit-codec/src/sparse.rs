//! 三值稀疏编码（COO 格式）。
//!
//! 三值网络权重里 `Unknown(0)` 占多数时，稠密存整条 `Vec<Ternary>` 浪费空间。
//! COO（coordinate）格式只记录**非 Unknown** 的位置和值：
//!
//! - `(index, value)` 对，`value ∈ {False, True}`；
//! - 未列出的位置一律视为 `Unknown`；
//! - `index` 必须严格递增、落在 `[0, len)` 内、不重复。
//!
//! 压缩比 = `1 - 非零数 / 总长`；全 Unknown 时编码为空列表，解码后是全 Unknown。

use crate::CodecError;
use trit_core::value::Ternary;

/// 一条稀疏记录：`(下标, 值)`。
pub type Entry = (u32, Ternary);

/// 解码长度硬顶：`len` 来自外部（CLI `--len`），下标本身是 `u32`。
/// 超过这个值在分配**之前**拒绝——不是性能调优，是把「用 `usize::MAX` 撑爆内存」
/// 从量纲上排除。`Ternary` 是 `repr(u8)`（1 字节），2^28 ≈ 256 MiB。
pub const MAX_SPARSE_DECODE_LEN: usize = 1 << 28;

/// 编码：只保留非 Unknown 的位置。
pub fn encode(values: &[Ternary]) -> Vec<Entry> {
    values
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_known())
        .map(|(i, v)| (i as u32, *v))
        .collect()
}

/// 长度上限校验（纯函数，不分配）：超过硬顶报 [`CodecError::SparseLenTooLarge`]。
fn check_len(len: usize) -> Result<(), CodecError> {
    if len > MAX_SPARSE_DECODE_LEN {
        return Err(CodecError::SparseLenTooLarge {
            len,
            max: MAX_SPARSE_DECODE_LEN,
        });
    }
    Ok(())
}

/// 解码：按 `len` 长度还原完整序列，未列出位置填 `Unknown`。
///
/// 校验：长度先过硬顶（分配前拒绝外部任意长度），再 fallible reservation；
/// 下标必须严格递增、不越界、不重复；否则返回结构化错误而不是静默错。
pub fn decode(entries: &[Entry], len: usize) -> Result<Vec<Ternary>, CodecError> {
    check_len(len)?;
    let mut out: Vec<Ternary> = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| CodecError::SparseAllocFailed { len })?;
    out.resize(len, Ternary::Unknown);
    let mut prev: Option<u32> = None;
    for &(index, value) in entries {
        if index as usize >= len {
            return Err(CodecError::SparseIndexOutOfRange { index, len });
        }
        if let Some(p) = prev
            && index <= p
        {
            return Err(CodecError::SparseIndexNotIncreasing {
                prev: p,
                found: index,
            });
        }
        out[index as usize] = value;
        prev = Some(index);
    }
    Ok(out)
}

/// 压缩比：`1 - 非零数 / 总长`；总长为 0 时返回 0。
pub fn sparsity(entries: &[Entry], len: usize) -> f64 {
    if len == 0 {
        0.0
    } else {
        1.0 - entries.len() as f64 / len as f64
    }
}

#[cfg(test)]
mod tests;
