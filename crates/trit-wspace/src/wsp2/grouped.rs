//! WSP2G：分组三值层的二进制容器（WSP2 v3 的**分组段**）。
//!
//! ## 为什么要有这一层
//!
//! 文本 `wsp1g` 每个元素要写一个十进制组号（1–2 字节），实测 42.4 MB → **56.9 MB
//! （0.75×）**——比 f32 还大。二进制里同样的信息是**定长位宽**：`bits = ceil(log2 组数)`，
//! G=256 时约 12 bit/元素，合计 `2 + 12 + 32/G` bit/权重。**组号必须计入**，漏掉它会
//! 把 2.37× 写成 15×（本项目踩过，故有测试守着）。
//!
//! ## 布局（小端）
//!
//! ```text
//! "WSP2" | version 3 | nlayers u16
//! layer: rows u32 | cols u32 | flags u8(bit1=grouped)
//!        | group_size u32 | group_count u32 | gbits u8
//!        | scales[group_count] f32 | bias_f32[rows] f32
//!        | trits 2bit | bias 2bit | groups[rows*cols] gbits
//! tail: checksum u64
//! ```
//!
//! `gbits` 由 `group_count` 决定并**写进文件**：解码时校验两者一致，避免"声明 4 位、
//! 实际塞 32 位"的假压缩。

use crate::error::{Error, Result};
use crate::grouped::GroupedLayer;
use crate::grouped_codec::{MAGIC_G, from_wspg, to_wspg};

use super::bytes::*;

/// 分组容器的版本号（WSP2 的 v3）。
pub const VERSION_GROUPED: u8 = 3;
const MAGIC: &[u8; 4] = b"WSP2";
const MAX_LAYERS: usize = 4096;
const FLAG_GROUPED: u8 = 0b10;

/// 表示 `max_value` 需要的位数（`max_value = 0` → 0 位）。
///
/// 组数 `n` 只需 `bits_for(n - 1)` 位。**不能用 32 位直存**——那会让每权重多 32 bit，
/// 压缩率直接掉到 1× 以下。
fn bits_for(max_value: usize) -> u8 {
    let needed = usize::BITS - max_value.leading_zeros();
    u8::try_from(needed).unwrap_or(32)
}

/// 把一组无符号整数按定长 `bits` 位打包（小端位序）。
fn pack_ids(values: &[u32], bits: u8) -> Result<Vec<u8>> {
    if bits > 32 {
        return Err(Error::Wire(format!("WSP2 位宽 {bits} 超过 32")));
    }
    if bits == 0 {
        if values.iter().any(|value| *value != 0) {
            return Err(Error::Wire("WSP2 位宽 0 只允许全 0 值".into()));
        }
        return Ok(Vec::new());
    }
    let limit = 1u64 << bits;
    let total_bits = values
        .len()
        .checked_mul(bits as usize)
        .ok_or_else(|| Error::Wire("WSP2 组号位数溢出".into()))?;
    let mut out = vec![0u8; total_bits.div_ceil(8)];
    let mut acc = 0u64;
    let mut acc_bits = 0u32;
    let mut pos = 0usize;
    for &value in values {
        if u64::from(value) >= limit {
            return Err(Error::Wire(format!("WSP2 组号 {value} 超过 {bits} 位")));
        }
        acc |= u64::from(value) << acc_bits;
        acc_bits += u32::from(bits);
        while acc_bits >= 8 {
            out[pos] = (acc & 0xff) as u8;
            pos += 1;
            acc >>= 8;
            acc_bits -= 8;
        }
    }
    if acc_bits > 0 {
        out[pos] = (acc & 0xff) as u8;
    }
    Ok(out)
}

/// 解包 `count` 个定长 `bits` 位的整数；字节数不符直接报错。
fn unpack_ids(bytes: &[u8], count: usize, bits: u8) -> Result<Vec<u32>> {
    if bits > 32 {
        return Err(Error::Wire(format!("WSP2 位宽 {bits} 超过 32")));
    }
    if bits == 0 {
        return Ok(vec![0; count]);
    }
    let total_bits = count
        .checked_mul(bits as usize)
        .ok_or_else(|| Error::Wire("WSP2 组号位数溢出".into()))?;
    let needed = total_bits.div_ceil(8);
    if bytes.len() != needed {
        return Err(Error::Wire(format!(
            "WSP2 组号段 {} 字节，应为 {needed}",
            bytes.len()
        )));
    }
    let mask = if bits == 32 {
        u64::from(u32::MAX)
    } else {
        (1u64 << bits) - 1
    };
    let mut out = Vec::with_capacity(count);
    let mut acc = 0u64;
    let mut acc_bits = 0u32;
    let mut pos = 0usize;
    while out.len() < count {
        while acc_bits < u32::from(bits) {
            acc |= u64::from(bytes[pos]) << acc_bits;
            pos += 1;
            acc_bits += 8;
        }
        out.push((acc & mask) as u32);
        acc >>= bits;
        acc_bits -= u32::from(bits);
    }
    Ok(out)
}

/// 把一组分组层编码成 WSP2G 二进制。
pub fn to_wsp2_grouped(layers: &[GroupedLayer]) -> Result<Vec<u8>> {
    let nlayers = u16::try_from(layers.len()).map_err(|_| Error::Wire("层数超过 u16".into()))?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION_GROUPED);
    out.extend_from_slice(&nlayers.to_le_bytes());
    for layer in layers {
        let rows = u32::try_from(layer.rows()).map_err(|_| Error::Wire("行数超过 u32".into()))?;
        let cols = u32::try_from(layer.cols()).map_err(|_| Error::Wire("列数超过 u32".into()))?;
        let group_count = layer.group_count();
        if group_count == 0 {
            return Err(Error::Wire("分组层没有 scale".into()));
        }
        if layer.scales().len() != group_count {
            return Err(Error::Wire("分组层 scale 数与组数不符".into()));
        }
        if layer.bias_f32.len() != layer.rows() {
            return Err(Error::Shape {
                what: "grouped bias_f32",
                expected: layer.rows(),
                got: layer.bias_f32.len(),
            });
        }
        let elements = layer.rows() * layer.cols();
        if layer.trits.len() != elements || layer.groups.len() != elements {
            return Err(Error::Shape {
                what: "grouped trits/groups",
                expected: elements,
                got: layer.trits.len().min(layer.groups.len()),
            });
        }
        let group_size =
            u32::try_from(layer.group_size()).map_err(|_| Error::Wire("组大小超过 u32".into()))?;
        if group_size == 0 {
            return Err(Error::Wire("组大小不能为 0".into()));
        }
        let group_count =
            u32::try_from(group_count).map_err(|_| Error::Wire("组数超过 u32".into()))?;
        let gbits = bits_for(group_count.saturating_sub(1) as usize);

        out.extend_from_slice(&rows.to_le_bytes());
        out.extend_from_slice(&cols.to_le_bytes());
        out.push(FLAG_GROUPED);
        out.extend_from_slice(&group_size.to_le_bytes());
        out.extend_from_slice(&group_count.to_le_bytes());
        out.push(gbits);
        for scale in layer.scales() {
            out.extend_from_slice(&scale.to_le_bytes());
        }
        for value in &layer.bias_f32 {
            out.extend_from_slice(&value.to_le_bytes());
        }
        let trits: Vec<i8> = layer.trits.iter().map(|trit| trit.to_i8()).collect();
        out.extend_from_slice(&pack_trits(&trits));
        let bias: Vec<i8> = layer.bias.iter().map(|trit| trit.to_i8()).collect();
        out.extend_from_slice(&pack_trits(&bias));
        out.extend_from_slice(&pack_ids(&layer.groups, gbits)?);
    }
    let checksum = fnv(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

/// 从 WSP2G 二进制解码分组层；magic/版本/校验和/位宽/组号越界都会报错。
pub fn from_wsp2_grouped(bytes: &[u8]) -> Result<Vec<GroupedLayer>> {
    if bytes.len() < 4 + 1 + 2 + 8 {
        return Err(Error::Wire("WSP2G 太短".into()));
    }
    if &bytes[..4] != MAGIC {
        return Err(Error::Wire("WSP2G magic 不符".into()));
    }
    let version = bytes[4];
    if version != VERSION_GROUPED {
        return Err(Error::Wire(format!(
            "WSP2 版本 {version} 不是分组容器（分组容器是 {VERSION_GROUPED}）"
        )));
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
        return Err(Error::Wire(format!("WSP2G 层数 {nlayers} 越界")));
    }
    let mut layers = Vec::with_capacity(nlayers);
    for _ in 0..nlayers {
        let rows = usize::try_from(read_u32(bytes, &mut pos)?)
            .map_err(|_| Error::Wire("WSP2G 行数超出 usize".into()))?;
        let cols = usize::try_from(read_u32(bytes, &mut pos)?)
            .map_err(|_| Error::Wire("WSP2G 列数超出 usize".into()))?;
        if rows == 0 || cols == 0 {
            return Err(Error::EmptyLayer { rows, cols });
        }
        let flags = take(bytes, &mut pos, 1)?[0];
        if flags & !FLAG_GROUPED != 0 {
            return Err(Error::Wire(format!("WSP2G flags {flags:#04x} 含未知位")));
        }
        if flags & FLAG_GROUPED == 0 {
            return Err(Error::Wire("WSP2G 层缺少分组标志".into()));
        }
        let group_size = usize::try_from(read_u32(bytes, &mut pos)?)
            .map_err(|_| Error::Wire("WSP2G 组大小超出 usize".into()))?;
        if group_size == 0 {
            return Err(Error::Wire("WSP2G 组大小不能为 0".into()));
        }
        let group_count = usize::try_from(read_u32(bytes, &mut pos)?)
            .map_err(|_| Error::Wire("WSP2G 组数超出 usize".into()))?;
        if group_count == 0 {
            return Err(Error::Wire("WSP2G 组数不能为 0".into()));
        }
        let gbits = take(bytes, &mut pos, 1)?[0];
        let want_bits = bits_for(group_count - 1);
        if gbits != want_bits {
            return Err(Error::Wire(format!(
                "WSP2G 组号位宽 {gbits} 与组数 {group_count} 不符（应为 {want_bits}）"
            )));
        }
        let elements = rows
            .checked_mul(cols)
            .ok_or_else(|| Error::Wire("WSP2G 元素数溢出".into()))?;
        let mut scales = Vec::with_capacity(group_count);
        for _ in 0..group_count {
            scales.push(read_f32(bytes, &mut pos)?);
        }
        let mut bias_f32 = Vec::with_capacity(rows);
        for _ in 0..rows {
            bias_f32.push(read_f32(bytes, &mut pos)?);
        }
        let trit_slice = take(bytes, &mut pos, elements.div_ceil(4))?;
        let trits = unpack_trits(trit_slice, elements)?;
        let bias_slice = take(bytes, &mut pos, rows.div_ceil(4))?;
        let bias = unpack_trits(bias_slice, rows)?;
        let group_bytes = elements
            .checked_mul(gbits as usize)
            .ok_or_else(|| Error::Wire("WSP2G 组号位数溢出".into()))?
            .div_ceil(8);
        let group_slice = take(bytes, &mut pos, group_bytes)?;
        let groups = unpack_ids(group_slice, elements, gbits)?;
        check_groups(&groups, group_count, group_size)?;
        check_bias(&bias, &bias_f32)?;
        let mut layer = GroupedLayer::from_trits(rows, cols, &trits, &groups, &scales, &bias_f32)?;
        // `from_trits` 拿不到组大小（它会退化成"组数"），必须以文件里的字段为准。
        layer.group_size = group_size;
        layers.push(layer);
    }
    if pos != payload_end {
        return Err(Error::Wire(format!(
            "WSP2G 尾部多出 {} 字节",
            payload_end.saturating_sub(pos)
        )));
    }
    Ok(layers)
}

/// 字节流是否是 WSP2G 分组容器（只看 magic + 版本，不校验和）。
#[must_use]
pub fn is_grouped_wsp2(bytes: &[u8]) -> bool {
    bytes.len() >= 5 && &bytes[..4] == MAGIC && bytes[4] == VERSION_GROUPED
}

/// 文本是 `wsp1g` 就编码成 WSP2G，否则返回 `None`（调用方继续走 WSP1 路径）。
pub fn pack_text(text: &str) -> Result<Option<Vec<u8>>> {
    let is_grouped = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .is_some_and(|line| line == MAGIC_G);
    if !is_grouped {
        return Ok(None);
    }
    let layer = from_wspg(text)?;
    let bytes = to_wsp2_grouped(std::slice::from_ref(&layer))?;
    Ok(Some(bytes))
}

/// WSP2G 二进制 → 单层 `wsp1g` 文本；多层容器直接报错（`wsp1g` 只表示单层）。
pub fn unpack_grouped(bytes: &[u8]) -> Result<String> {
    let layers = from_wsp2_grouped(bytes)?;
    match layers.as_slice() {
        [layer] => Ok(to_wspg(layer)),
        other => Err(Error::Wire(format!(
            "分组容器有 {} 层，wsp1g 只表示单层；请用库层 API 拆层",
            other.len()
        ))),
    }
}

/// bias 的 2-bit 符号快照必须与 f32 幅度同号，否则文件自相矛盾。
fn check_bias(bias: &[i8], bias_f32: &[f32]) -> Result<()> {
    if bias.len() != bias_f32.len() {
        return Err(Error::Shape {
            what: "grouped bias vs bias_f32",
            expected: bias_f32.len(),
            got: bias.len(),
        });
    }
    for (index, (trit, value)) in bias.iter().zip(bias_f32).enumerate() {
        let want = crate::grouped::trit_of(*value).to_i8();
        if *trit != want {
            return Err(Error::Wire(format!(
                "WSP2G bias[{index}] 的 2-bit 符号 {trit} 与 f32 {value} 不一致"
            )));
        }
    }
    Ok(())
}

/// 组号必须落在 `0..group_count`，且每组元素数不超过 `group_size`。
fn check_groups(groups: &[u32], group_count: usize, group_size: usize) -> Result<()> {
    let mut per_group = vec![0u32; group_count];
    for &group in groups {
        let index =
            usize::try_from(group).map_err(|_| Error::Wire("WSP2G 组号超出 usize".into()))?;
        if index >= group_count {
            return Err(Error::Wire(format!(
                "WSP2G 组号 {index} 越界（共 {group_count} 组）"
            )));
        }
        per_group[index] += 1;
    }
    if let Some((index, count)) = per_group
        .iter()
        .enumerate()
        .find(|(_, count)| **count as usize > group_size)
    {
        return Err(Error::Wire(format!(
            "WSP2G 组 {index} 有 {count} 个元素，超过组大小 {group_size}"
        )));
    }
    Ok(())
}
