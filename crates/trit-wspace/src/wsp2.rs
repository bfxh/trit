//! WSP2：WSP1 的 2-bit 压缩容器（二进制）。
//! 真实模型的大张量用 WSP1 文本会膨胀到数十 MB；WSP2 把 trit 压到 2 bit
//! (`-1/0/+1 → 0/1/2`，3 非法)、bias 同样打包、scale 保持 f32，末尾带 FNV-1a。
//! 它是缓存/传输格式，解包后回到 WSP1。
//!
//! 布局（小端）：`magic "WSP2" | version u8 | nlayers u16`；每层
//! `rows/cols u32 | flags u8 | scales | [bias_f32] | trits 2bit | bias 2bit`；末尾 `checksum u64`。

use crate::error::{Error, Result};
use crate::net::{Layer, Net};

pub(super) const MAGIC: &[u8; 4] = b"WSP2";
pub(super) const VERSION: u8 = 2;
const MAX_LAYERS: usize = 4096;

/// 从 WSP2 二进制解码网络；magic/版本/校验和/2-bit 非法值都会报错。
pub fn from_wsp2(bytes: &[u8]) -> Result<Net> {
    if bytes.len() < 4 + 1 + 2 + 8 {
        return Err(Error::Wire("WSP2 太短".into()));
    }
    if &bytes[..4] != MAGIC {
        return Err(Error::Wire("WSP2 magic 不符".into()));
    }
    let version = bytes[4];
    if version != 1 && version != VERSION {
        return Err(Error::Wire(format!("WSP2 版本 {version} 不受支持")));
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
        // 分配护栏：每行至少要 4 字节 scale（biasf/2-bit trits 由 take 兜底）。
        // 攻击者可用 rows=u32::MAX 的 15 字节文件让 with_capacity 试图分配 16GB。
        if rows.saturating_mul(4) > bytes.len() - pos {
            return Err(Error::Wire(format!(
                "WSP2 行数 {rows} 与剩余 {} 字节不符",
                bytes.len() - pos
            )));
        }
        let flags = if version == 2 {
            take(bytes, &mut pos, 1)?[0]
        } else {
            0
        };
        if flags & !1 != 0 {
            return Err(Error::Wire(format!("WSP2 flags {flags:#04x} 含未知位")));
        }
        let elements = rows
            .checked_mul(cols)
            .ok_or_else(|| Error::Wire("WSP2 元素数溢出".into()))?;
        let mut scales = Vec::with_capacity(rows);
        for _ in 0..rows {
            scales.push(read_f32(bytes, &mut pos)?);
        }
        let mut bias_f32 = Vec::new();
        if flags & 1 != 0 {
            for _ in 0..rows {
                bias_f32.push(read_f32(bytes, &mut pos)?);
            }
        }
        let trit_slice = take(bytes, &mut pos, elements.div_ceil(4))?;
        let trits = unpack_trits(trit_slice, elements)?;
        let bias_slice = take(bytes, &mut pos, rows.div_ceil(4))?;
        let bias = unpack_trits(bias_slice, rows)?;
        let layer = Layer::with_scales(rows, cols, &trits, &bias, scales)?;
        let layer = if bias_f32.is_empty() {
            layer
        } else {
            layer.with_bias_f32(bias_f32)?
        };
        layers.push(layer);
    }
    if pos != payload_end {
        return Err(Error::Wire(format!(
            "WSP2 尾部多出 {} 字节",
            payload_end.saturating_sub(pos)
        )));
    }
    Net::new(layers)
}

pub(crate) mod bytes;
mod write;

use bytes::*;
pub use write::to_wsp2;

mod index;
mod index_io;
pub use index::{BlockIndex, DEFAULT_BLOCK_ROWS, LayerIndex, RowBlock};
