//! WSP2：WSP1 的 2-bit 压缩容器（二进制）。
//!
//! WSP1 是“可读、可 diff、可校验”的规范文本；真实模型的大张量用文本会膨胀到数十 MB。
//! WSP2 把每个 trit 压到 2 bit（`-1/0/+1 → 0/1/2`，3 保留为非法），bias 同样打包，
//! scale 保持 f32，末尾带 FNV-1a 校验和。它是**缓存/传输格式**，不是规范形式：
//! 解包后仍然回到 WSP1 的可读工件。
//!
//! 布局（小端）：
//! ```text
//! magic "WSP2" | version u8=1 | nlayers u16
//! per layer: rows u32 | cols u32 | scales rows×f32 | trits 2bit | bias 2bit
//! checksum u64（对前面所有字节做 FNV-1a）
//! ```

use crate::error::{Error, Result};
use crate::net::{Layer, Net};

const MAGIC: &[u8; 4] = b"WSP2";
const VERSION: u8 = 1;
const MAX_LAYERS: usize = 4096;

/// 把网络编码成 WSP2 二进制。
pub fn to_wsp2(net: &Net) -> Result<Vec<u8>> {
    let nlayers =
        u16::try_from(net.layers().len()).map_err(|_| Error::Wire("层数超过 u16".into()))?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&nlayers.to_le_bytes());
    for layer in net.layers() {
        if !layer.bias_f32().is_empty() {
            return Err(Error::Wire(
                "WSP2 暂不支持 f32 bias（biasf 行）；请保留 WSP1 或先去掉 biasf".into(),
            ));
        }
        let rows = u32::try_from(layer.rows()).map_err(|_| Error::Wire("行数超过 u32".into()))?;
        let cols = u32::try_from(layer.cols()).map_err(|_| Error::Wire("列数超过 u32".into()))?;
        out.extend_from_slice(&rows.to_le_bytes());
        out.extend_from_slice(&cols.to_le_bytes());
        for scale in layer.scales() {
            out.extend_from_slice(&scale.to_le_bytes());
        }
        let trits: Vec<i8> = layer.trits().iter().map(|trit| trit.to_i8()).collect();
        out.extend_from_slice(&pack_trits(&trits));
        let bias: Vec<i8> = layer.bias().iter().map(|trit| trit.to_i8()).collect();
        out.extend_from_slice(&pack_trits(&bias));
    }
    let checksum = fnv(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

fn pack_trits(values: &[i8]) -> Vec<u8> {
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

fn unpack_trits(bytes: &[u8], count: usize) -> Result<Vec<i8>> {
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

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 从 WSP2 二进制解码网络；magic/版本/校验和/2-bit 非法值都会报错。
pub fn from_wsp2(bytes: &[u8]) -> Result<Net> {
    if bytes.len() < 4 + 1 + 2 + 8 {
        return Err(Error::Wire("WSP2 太短".into()));
    }
    if &bytes[..4] != MAGIC {
        return Err(Error::Wire("WSP2 magic 不符".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Wire(format!("WSP2 版本 {} 不受支持", bytes[4])));
    }
    let payload_end = bytes.len() - 8;
    let expected = u64::from_le_bytes(bytes[payload_end..].try_into().expect("8 字节校验和"));
    let got = fnv(&bytes[..payload_end]);
    if got != expected {
        return Err(Error::Checksum { expected, got });
    }
    let mut pos = 5;
    let nlayers = usize::from(read_u16(bytes, &mut pos)?);
    if nlayers == 0 || nlayers > MAX_LAYERS {
        return Err(Error::Wire(format!("WSP2 层数 {nlayers} 越界")));
    }
    let mut layers = Vec::with_capacity(nlayers);
    for _ in 0..nlayers {
        let rows = usize::try_from(read_u32(bytes, &mut pos)?)
            .map_err(|_| Error::Wire("WSP2 行数超出 usize".into()))?;
        let cols = usize::try_from(read_u32(bytes, &mut pos)?)
            .map_err(|_| Error::Wire("WSP2 列数超出 usize".into()))?;
        if rows == 0 || cols == 0 {
            return Err(Error::EmptyLayer { rows, cols });
        }
        let elements = rows
            .checked_mul(cols)
            .ok_or_else(|| Error::Wire("WSP2 元素数溢出".into()))?;
        let mut scales = Vec::with_capacity(rows);
        for _ in 0..rows {
            scales.push(read_f32(bytes, &mut pos)?);
        }
        let trit_slice = take(bytes, &mut pos, elements.div_ceil(4))?;
        let trits = unpack_trits(trit_slice, elements)?;
        let bias_slice = take(bytes, &mut pos, rows.div_ceil(4))?;
        let bias = unpack_trits(bias_slice, rows)?;
        layers.push(Layer::with_scales(rows, cols, &trits, &bias, scales)?);
    }
    if pos != payload_end {
        return Err(Error::Wire(format!(
            "WSP2 尾部多出 {} 字节",
            payload_end.saturating_sub(pos)
        )));
    }
    Net::new(layers)
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8]> {
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

fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16> {
    let slice = take(bytes, pos, 2)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let slice = take(bytes, pos, 4)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_f32(bytes: &[u8], pos: &mut usize) -> Result<f32> {
    let slice = take(bytes, pos, 4)?;
    Ok(f32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
