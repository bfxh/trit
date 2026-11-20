//! I2_S：2 bit/trit + per-tensor f32 scale（bitnet.cpp 互操作格式）。
//!
//! 规格来源：`microsoft/BitNet` 的 `utils/convert-hf-to-gguf-bitnet.py` 的 `quantize_to_i2_s`。
//! 映射 `-1 → 0`、`0 → 1`、`+1 → 2`；每 128 值一块，块内 `reshape(4, 32)`，
//! 第 `j` 组第 `k` 个值放在第 `k` 字节的 `(3-j)` 个 2-bit 槽；尾部 32 字节，
//! 前 4 字节是 f32 小端。

use trit_core::Ternary;

/// 每块元素数。
pub const BLOCK_VALUES: usize = 128;
/// 每块字节数。
pub const BLOCK_BYTES: usize = 32;
/// 尾部的字节数（前 4 字节 f32，其余补 0）。
pub const TAIL_BYTES: usize = 32;

/// I2_S 编解码错误。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum I2sError {
    /// 元素数不是 128 的倍数。
    #[error("I2_S 元素数 {0} 不是 128 的倍数")]
    BlockAlign(usize),
    /// 缓冲区长度与声明元素数不符。
    #[error("I2_S 长度 {packed} 与 {count} 个元素不符（应为 {expected}）")]
    Length {
        packed: usize,
        count: usize,
        expected: usize,
    },
    /// scale 不是正有限值。
    #[error("I2_S scale 必须是正有限值（收到 {0}）")]
    Scale(f32),
}

/// `-1/0/+1` → 2-bit 编码 `0/1/2`。
fn encode_trit(t: Ternary) -> u8 {
    match t {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

/// 2-bit 编码 → 三值（`3` 不会出现在合法输入里，按 `+1` 处理）。
fn decode_code(code: u8) -> Ternary {
    match code {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

/// 只打包 2-bit 块；元素数必须是 128 的倍数。
pub fn pack(trits: &[Ternary]) -> Result<Vec<u8>, I2sError> {
    if !trits.len().is_multiple_of(BLOCK_VALUES) {
        return Err(I2sError::BlockAlign(trits.len()));
    }
    let mut out = vec![0u8; trits.len() / 4];
    for (bi, block) in trits.chunks_exact(BLOCK_VALUES).enumerate() {
        let base = bi * BLOCK_BYTES;
        for k in 0..32 {
            let c0 = encode_trit(block[k]);
            let c1 = encode_trit(block[32 + k]);
            let c2 = encode_trit(block[64 + k]);
            let c3 = encode_trit(block[96 + k]);
            out[base + k] = (c0 << 6) | (c1 << 4) | (c2 << 2) | c3;
        }
    }
    Ok(out)
}

/// 解包 `count` 个元素；`packed` 必须正好 `count / 4` 字节。
pub fn unpack(packed: &[u8], count: usize) -> Result<Vec<Ternary>, I2sError> {
    if !count.is_multiple_of(BLOCK_VALUES) {
        return Err(I2sError::BlockAlign(count));
    }
    let expected = count / 4;
    if packed.len() != expected {
        return Err(I2sError::Length {
            packed: packed.len(),
            count,
            expected,
        });
    }
    let mut out = Vec::with_capacity(count);
    for block in packed.chunks_exact(BLOCK_BYTES) {
        let mut vals = [Ternary::Unknown; BLOCK_VALUES];
        for k in 0..32 {
            let b = block[k];
            vals[k] = decode_code((b >> 6) & 3);
            vals[32 + k] = decode_code((b >> 4) & 3);
            vals[64 + k] = decode_code((b >> 2) & 3);
            vals[96 + k] = decode_code(b & 3);
        }
        out.extend_from_slice(&vals);
    }
    Ok(out)
}

/// scale：第一个非零 `|w|`（阈值 `1e-6`）；全零取 `1e-5`（与 bitnet.cpp 的转换脚本一致）。
pub fn quantize_scale(values: &[f32]) -> f32 {
    values
        .iter()
        .map(|v| v.abs())
        .find(|a| *a > 1e-6)
        .unwrap_or(1e-5)
}

/// 从 f32 量化并打包，返回完整 buffer 与 scale。
pub fn encode_f32(values: &[f32], override_scale: Option<f32>) -> Result<(Vec<u8>, f32), I2sError> {
    let scale = override_scale.unwrap_or_else(|| quantize_scale(values));
    if !(scale.is_finite() && scale > 0.0) {
        return Err(I2sError::Scale(scale));
    }
    let trits: Vec<Ternary> = values
        .iter()
        .map(|v| match (v / scale).round().clamp(-1.0, 1.0) {
            x if x > 0.5 => Ternary::True,
            x if x < -0.5 => Ternary::False,
            _ => Ternary::Unknown,
        })
        .collect();
    let out = encode_ternary(&trits, scale)?;
    Ok((out, scale))
}

/// 已是三值：打包 + 32 字节尾部，返回完整 buffer。
pub fn encode_ternary(trits: &[Ternary], scale: f32) -> Result<Vec<u8>, I2sError> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(I2sError::Scale(scale));
    }
    let mut out = pack(trits)?;
    out.extend_from_slice(&scale.to_le_bytes());
    out.resize(out.len() + TAIL_BYTES - 4, 0);
    Ok(out)
}

/// 从完整 buffer 解出 `(trits, scale)`。
pub fn decode(bytes: &[u8], count: usize) -> Result<(Vec<Ternary>, f32), I2sError> {
    if !count.is_multiple_of(BLOCK_VALUES) {
        return Err(I2sError::BlockAlign(count));
    }
    let expected = count / 4 + TAIL_BYTES;
    if bytes.len() != expected {
        return Err(I2sError::Length {
            packed: bytes.len(),
            count,
            expected,
        });
    }
    let packed = &bytes[..count / 4];
    let scale = f32::from_le_bytes(bytes[count / 4..count / 4 + 4].try_into().unwrap());
    if !(scale.is_finite() && scale > 0.0) {
        return Err(I2sError::Scale(scale));
    }
    Ok((unpack(packed, count)?, scale))
}
