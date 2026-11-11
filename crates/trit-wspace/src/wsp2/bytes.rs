//! WSP2 的字节读写辅助（2-bit 打包、FNV-1a、小端读取）。

use crate::error::{Error, Result};

/// 把 trit 序列按 2 bit 打包（`-1/0/+1 → 0/1/2`）。
pub(super) fn pack_trits(values: &[i8]) -> Vec<u8> {
    let mut out = vec![0u8; values.len().div_ceil(4)];
    for (index, &value) in values.iter().enumerate() {
        let code = match value {
            -1 => 0u8,
            0 => 1,
            1 => 2,
            _ => 3,
        };
        out[index / 4] |= code << ((index % 4) * 2);
    }
    out
}

/// 解包 `count` 个 trit；2-bit 值 3 判非法。
pub(super) fn unpack_trits(bytes: &[u8], count: usize) -> Result<Vec<i8>> {
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let code = (bytes[index / 4] >> ((index % 4) * 2)) & 0b11;
        out.push(match code {
            0 => -1,
            1 => 0,
            2 => 1,
            _ => return Err(Error::InvalidTrit(3)),
        });
    }
    Ok(out)
}

/// FNV-1a 校验和。
pub(super) fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 从 `pos` 取 `len` 字节并前进；越界报截断。
pub(super) fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| Error::Wire("WSP2 偏移溢出".into()))?;
    if end > bytes.len() {
        return Err(Error::Wire(format!(
            "WSP2 截断：需要到 {end}，只有 {}",
            bytes.len()
        )));
    }
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

/// 读小端 u16。
pub(super) fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16> {
    let slice = take(bytes, pos, 2)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

/// 读小端 u32。
pub(super) fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let slice = take(bytes, pos, 4)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// 读小端 f32。
pub(super) fn read_f32(bytes: &[u8], pos: &mut usize) -> Result<f32> {
    let slice = take(bytes, pos, 4)?;
    Ok(f32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
