//! WSP2 块索引与随机访问：扫描层表/行块，只解码指定层或行块。
//!
//! WSP2 每层是连续的 `rows/cols/flags/scales[/bias_f32]/trits/bias`；trits 与 bias
//! 是 2-bit 打包（4 trits/byte）。索引只记录偏移与长度，不复制权重：
//!
//! ```text
//! WSP2-IDX v1: source_bytes | source_checksum | block_rows | layers[]
//! layer: rows cols flags layer_offset scales_offset [bias_f32_offset] trits_offset
//!        bias_offset end_offset row_blocks[]
//! row_block: row_start row_end trits_offset trits_len
//! ```
//!
//! 读取整层时按 `scales → trits → bias` 解码；读取行块时把 trits 起点向下取整到
//! 字节、解出块内前缀后截取目标行。索引本身不验证权重内容（`scan` 做一次全量
//! FNV），随机读取只比较文件长度与尾部校验和。

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::net::Layer;

use super::bytes::{fnv, read_u16, read_u32, unpack_trits};

/// 索引 JSON 的 magic。
pub const INDEX_MAGIC: &str = "WSP2-IDX";
/// 索引 JSON 的版本。
pub const INDEX_VERSION: u32 = 1;
/// 默认行块大小（行数）。
pub const DEFAULT_BLOCK_ROWS: usize = 64;
const MAX_LAYERS: usize = 4096;

/// 一个行块在 WSP2 文件中的位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowBlock {
    /// 起始行（含）。
    pub row_start: usize,
    /// 结束行（不含）。
    pub row_end: usize,
    /// 打包 trits 的字节偏移（相对文件起点）。
    pub trits_offset: usize,
    /// 打包 trits 的字节数。
    pub trits_len: usize,
}

/// 一层的偏移与行块表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerIndex {
    /// 层号（从 0 开始，连续）。
    pub index: usize,
    /// 行数（输出维）。
    pub rows: usize,
    /// 列数（输入维）。
    pub cols: usize,
    /// 是否带 f32 偏置幅度。
    pub has_bias_f32: bool,
    /// 层头（rows u32）的偏移。
    pub layer_offset: usize,
    /// scales 的偏移。
    pub scales_offset: usize,
    /// f32 偏置的偏移（没有就是 `None`）。
    pub bias_f32_offset: Option<usize>,
    /// 打包 trits 的偏移。
    pub trits_offset: usize,
    /// 打包 bias 的偏移。
    pub bias_offset: usize,
    /// 层数据结束（下一层起点或校验和起点）。
    pub end_offset: usize,
    /// 行块表（按行递增、无重叠、覆盖 `0..rows`）。
    pub row_blocks: Vec<RowBlock>,
}

/// WSP2 文件索引。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockIndex {
    /// 源文件字节数。
    pub source_bytes: usize,
    /// 源文件尾部 FNV-1a 校验和。
    pub source_checksum: u64,
    /// 行块大小（行数）。
    pub block_rows: usize,
    /// 层表。
    pub layers: Vec<LayerIndex>,
}

/// 一行的原始数据（只用于读取时拼接块）。
struct RowsRaw {
    trits: Vec<i8>,
    bias: Vec<i8>,
    scales: Vec<f32>,
    bias_f32: Vec<f32>,
}

impl BlockIndex {
    /// 扫描 WSP2 字节流，生成索引；magic/版本/校验和/结构都会校验。
    pub fn scan(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 4 + 1 + 2 + 8 {
            return Err(Error::Wire("WSP2 太短".into()));
        }
        if &bytes[..4] != b"WSP2" {
            return Err(Error::Wire("WSP2 magic 不符".into()));
        }
        let version = bytes[4];
        if version != 1 && version != 2 {
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
        for index in 0..nlayers {
            let layer_offset = pos;
            let rows = usize::try_from(read_u32(bytes, &mut pos)?)
                .map_err(|_| Error::Wire("WSP2 行数超出 usize".into()))?;
            let cols = usize::try_from(read_u32(bytes, &mut pos)?)
                .map_err(|_| Error::Wire("WSP2 列数超出 usize".into()))?;
            if rows == 0 || cols == 0 {
                return Err(Error::EmptyLayer { rows, cols });
            }
            let flags = if version == 2 {
                take(bytes, &mut pos, 1)?[0]
            } else {
                0
            };
            if flags & !1 != 0 {
                return Err(Error::Wire(format!("WSP2 flags {flags:#04x} 含未知位")));
            }
            let scales_offset = pos;
            pos = advance(bytes, pos, rows.checked_mul(4).ok_or_else(usize_overflow)?)?;
            let bias_f32_offset = if flags & 1 != 0 {
                let offset = pos;
                pos = advance(bytes, pos, rows.checked_mul(4).ok_or_else(usize_overflow)?)?;
                Some(offset)
            } else {
                None
            };
            let trits_offset = pos;
            let trit_bytes = rows
                .checked_mul(cols)
                .ok_or_else(|| Error::Wire("WSP2 元素数溢出".into()))?
                .div_ceil(4);
            pos = advance(bytes, pos, trit_bytes)?;
            let bias_offset = pos;
            pos = advance(bytes, pos, rows.div_ceil(4))?;
            let end_offset = pos;
            let row_blocks = build_blocks(rows, cols, trits_offset, trit_bytes)?;
            layers.push(LayerIndex {
                index,
                rows,
                cols,
                has_bias_f32: flags & 1 != 0,
                layer_offset,
                scales_offset,
                bias_f32_offset,
                trits_offset,
                bias_offset,
                end_offset,
                row_blocks,
            });
        }
        if pos != payload_end {
            return Err(Error::Wire(format!(
                "WSP2 尾部多出 {} 字节",
                payload_end.saturating_sub(pos)
            )));
        }
        Ok(Self {
            source_bytes: bytes.len(),
            source_checksum: expected,
            block_rows: DEFAULT_BLOCK_ROWS,
            layers,
        })
    }

    /// 读一整层。
    pub fn read_layer(&self, bytes: &[u8], layer: usize) -> Result<Layer> {
        let info = self.layer(layer)?;
        self.check_source(bytes)?;
        let raw = read_rows_raw(bytes, info, 0, info.rows)?;
        into_layer(info, raw)
    }

    /// 读 `row_start..row_start+rows` 的子层；跨块时逐块拼接。
    pub fn read_rows(
        &self,
        bytes: &[u8],
        layer: usize,
        row_start: usize,
        rows: usize,
    ) -> Result<Layer> {
        let info = self.layer(layer)?;
        let end = row_start
            .checked_add(rows)
            .ok_or_else(|| Error::Wire("行范围溢出".into()))?;
        if rows == 0 || end > info.rows {
            return Err(Error::Wire(format!(
                "行范围 {row_start}..{end} 越界（层 {layer} 共 {} 行）",
                info.rows
            )));
        }
        self.check_source(bytes)?;
        if row_start == 0 && rows == info.rows {
            return self.read_layer(bytes, layer);
        }
        let mut trits = Vec::with_capacity(rows.saturating_mul(info.cols));
        let mut bias = Vec::with_capacity(rows);
        let mut scales = Vec::with_capacity(rows);
        let mut bias_f32 = Vec::with_capacity(rows);
        let mut row = row_start;
        while row < end {
            let block = info
                .row_blocks
                .iter()
                .find(|block| row >= block.row_start && row < block.row_end)
                .ok_or_else(|| Error::Wire(format!("行 {row} 不在任何块内")))?;
            let take = (block.row_end - row).min(end - row);
            let raw = read_block_raw(bytes, info, block, row, take)?;
            trits.extend_from_slice(&raw.trits);
            bias.extend_from_slice(&raw.bias);
            scales.extend_from_slice(&raw.scales);
            bias_f32.extend_from_slice(&raw.bias_f32);
            row += take;
        }
        let layer = Layer::with_scales(rows, info.cols, &trits, &bias, scales)?;
        if info.has_bias_f32 {
            layer.with_bias_f32(bias_f32)
        } else {
            Ok(layer)
        }
    }

    /// 序列化为 JSON。
    pub fn to_json(&self) -> Value {
        json!({
            "magic": INDEX_MAGIC,
            "version": INDEX_VERSION,
            "source_bytes": self.source_bytes,
            "source_checksum": self.source_checksum,
            "block_rows": self.block_rows,
            "layers": self.layers.iter().map(LayerIndex::to_json).collect::<Vec<_>>(),
        })
    }

    /// 从 JSON 反序列化；缺字段/越界/块不连续都报错。
    pub fn from_json(value: &Value) -> Result<Self> {
        let magic = value
            .get("magic")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Wire("索引缺 magic".into()))?;
        if magic != INDEX_MAGIC {
            return Err(Error::Wire(format!("索引 magic `{magic}` 不符")));
        }
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Wire("索引缺 version".into()))?;
        if version != u64::from(INDEX_VERSION) {
            return Err(Error::Wire(format!("索引版本 {version} 不受支持")));
        }
        let source_bytes = usize_field(value, "source_bytes")?;
        let source_checksum = value
            .get("source_checksum")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Wire("索引缺 source_checksum".into()))?;
        let block_rows = usize_field(value, "block_rows")?;
        if block_rows == 0 {
            return Err(Error::Wire("索引 block_rows 不能为 0".into()));
        }
        let entries = value
            .get("layers")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Wire("索引缺 layers 数组".into()))?;
        if entries.is_empty() {
            return Err(Error::Wire("索引 layers 为空".into()));
        }
        let mut layers = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let info = LayerIndex::from_json(entry)?;
            if info.index != index {
                return Err(Error::Wire(format!(
                    "索引层号不连续：位置 {index} 是 {}",
                    info.index
                )));
            }
            layers.push(info);
        }
        Ok(Self {
            source_bytes,
            source_checksum,
            block_rows,
            layers,
        })
    }

    pub(crate) fn layer(&self, index: usize) -> Result<&LayerIndex> {
        self.layers.get(index).ok_or_else(|| {
            Error::Wire(format!(
                "层号 {index} 越界（索引共 {} 层）",
                self.layers.len()
            ))
        })
    }

    /// 轻量源检查：长度 + 尾部校验和（不重算整文件 FNV）。
    fn check_source(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() != self.source_bytes {
            return Err(Error::Wire(format!(
                "源文件 {} 字节，索引记录 {} 字节",
                bytes.len(),
                self.source_bytes
            )));
        }
        if bytes.len() < 8 {
            return Err(Error::Wire("WSP2 太短".into()));
        }
        let tail = u64::from_le_bytes(bytes[bytes.len() - 8..].try_into().expect("8 字节校验和"));
        if tail != self.source_checksum {
            return Err(Error::Checksum {
                expected: self.source_checksum,
                got: tail,
            });
        }
        Ok(())
    }
}

impl LayerIndex {
    fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "rows": self.rows,
            "cols": self.cols,
            "has_bias_f32": self.has_bias_f32,
            "layer_offset": self.layer_offset,
            "scales_offset": self.scales_offset,
            "bias_f32_offset": self.bias_f32_offset,
            "trits_offset": self.trits_offset,
            "bias_offset": self.bias_offset,
            "end_offset": self.end_offset,
            "row_blocks": self.row_blocks.iter().map(RowBlock::to_json).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Result<Self> {
        let index = usize_field(value, "index")?;
        let rows = usize_field(value, "rows")?;
        let cols = usize_field(value, "cols")?;
        if rows == 0 || cols == 0 {
            return Err(Error::Wire("索引层 rows/cols 不能为 0".into()));
        }
        let has_bias_f32 = value
            .get("has_bias_f32")
            .and_then(Value::as_bool)
            .ok_or_else(|| Error::Wire("索引层缺 has_bias_f32".into()))?;
        let layer_offset = usize_field(value, "layer_offset")?;
        let scales_offset = usize_field(value, "scales_offset")?;
        let bias_f32_offset = value
            .get("bias_f32_offset")
            .and_then(Value::as_u64)
            .map(|raw| {
                usize::try_from(raw).map_err(|_| Error::Wire("bias_f32_offset 超出 usize".into()))
            })
            .transpose()?;
        if has_bias_f32 != bias_f32_offset.is_some() {
            return Err(Error::Wire(
                "索引层 has_bias_f32 与 bias_f32_offset 不一致".into(),
            ));
        }
        let trits_offset = usize_field(value, "trits_offset")?;
        let bias_offset = usize_field(value, "bias_offset")?;
        let end_offset = usize_field(value, "end_offset")?;
        if !(layer_offset < scales_offset
            && scales_offset <= bias_f32_offset.unwrap_or(trits_offset)
            && bias_f32_offset.unwrap_or(trits_offset) <= trits_offset
            && trits_offset < bias_offset
            && bias_offset < end_offset)
        {
            return Err(Error::Wire("索引层偏移顺序不合法".into()));
        }
        let entries = value
            .get("row_blocks")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Wire("索引层缺 row_blocks".into()))?;
        if entries.is_empty() {
            return Err(Error::Wire("索引层 row_blocks 为空".into()));
        }
        let mut row_blocks = Vec::with_capacity(entries.len());
        let mut expected_start = 0;
        for entry in entries {
            let block = RowBlock::from_json(entry)?;
            if block.row_start != expected_start {
                return Err(Error::Wire(format!(
                    "索引行块不连续：期望 {expected_start}，得到 {}",
                    block.row_start
                )));
            }
            expected_start = block.row_end;
            row_blocks.push(block);
        }
        if expected_start != rows {
            return Err(Error::Wire(format!(
                "索引行块覆盖到 {expected_start}，层有 {rows} 行"
            )));
        }
        for block in &row_blocks {
            let first = block
                .row_start
                .checked_mul(cols)
                .ok_or_else(|| Error::Wire("索引块起点溢出".into()))?
                / 4;
            let last = block
                .row_end
                .checked_mul(cols)
                .ok_or_else(|| Error::Wire("索引块终点溢出".into()))?
                .div_ceil(4);
            if block.trits_offset != trits_offset + first
                || block.trits_len != last.saturating_sub(first)
            {
                return Err(Error::Wire(format!(
                    "索引块 {}..{} 的 trits 偏移/长度与层不符",
                    block.row_start, block.row_end
                )));
            }
        }
        Ok(Self {
            index,
            rows,
            cols,
            has_bias_f32,
            layer_offset,
            scales_offset,
            bias_f32_offset,
            trits_offset,
            bias_offset,
            end_offset,
            row_blocks,
        })
    }
}

impl RowBlock {
    fn to_json(&self) -> Value {
        json!({
            "row_start": self.row_start,
            "row_end": self.row_end,
            "trits_offset": self.trits_offset,
            "trits_len": self.trits_len,
        })
    }

    fn from_json(value: &Value) -> Result<Self> {
        let row_start = usize_field(value, "row_start")?;
        let row_end = usize_field(value, "row_end")?;
        let trits_offset = usize_field(value, "trits_offset")?;
        let trits_len = usize_field(value, "trits_len")?;
        if row_start >= row_end || trits_len == 0 {
            return Err(Error::Wire("索引行块范围为空".into()));
        }
        Ok(Self {
            row_start,
            row_end,
            trits_offset,
            trits_len,
        })
    }
}

fn build_blocks(
    rows: usize,
    cols: usize,
    trits_offset: usize,
    trit_bytes: usize,
) -> Result<Vec<RowBlock>> {
    let mut blocks = Vec::new();
    let mut start = 0;
    while start < rows {
        let end = (start + DEFAULT_BLOCK_ROWS).min(rows);
        let first = start
            .checked_mul(cols)
            .ok_or_else(|| Error::Wire("行块起点溢出".into()))?
            / 4;
        let last = end
            .checked_mul(cols)
            .ok_or_else(|| Error::Wire("行块终点溢出".into()))?
            .div_ceil(4);
        let len = last
            .checked_sub(first)
            .ok_or_else(|| Error::Wire("行块长度下溢".into()))?;
        if last > trit_bytes {
            return Err(Error::Wire("行块超出 trits 区".into()));
        }
        blocks.push(RowBlock {
            row_start: start,
            row_end: end,
            trits_offset: trits_offset + first,
            trits_len: len,
        });
        start = end;
    }
    Ok(blocks)
}

fn read_block_raw(
    bytes: &[u8],
    info: &LayerIndex,
    block: &RowBlock,
    row_start: usize,
    rows: usize,
) -> Result<RowsRaw> {
    let row_end = row_start
        .checked_add(rows)
        .ok_or_else(|| Error::Wire("行范围溢出".into()))?;
    if rows == 0 || row_start < block.row_start || row_end > block.row_end {
        return Err(Error::Wire(format!(
            "行范围 {row_start}..{row_end} 不在块 {}..{} 内",
            block.row_start, block.row_end
        )));
    }
    let relative_start = row_start - block.row_start;
    let relative_end = row_end - block.row_start;
    let trit_start = relative_start
        .checked_mul(info.cols)
        .ok_or_else(|| Error::Wire("trit 起点溢出".into()))?;
    let trit_end = relative_end
        .checked_mul(info.cols)
        .ok_or_else(|| Error::Wire("trit 终点溢出".into()))?;
    let first = trit_start / 4;
    let last = trit_end.div_ceil(4);
    if last > block.trits_len {
        return Err(Error::Wire(format!(
            "块 {}..{} 读取 {first}..{last} 超出 {} 字节",
            block.row_start, block.row_end, block.trits_len
        )));
    }
    let raw = slice(bytes, block.trits_offset + first, last - first)?;
    let prefix = trit_start % 4;
    let unpacked = unpack_trits(raw, trit_end - trit_start + prefix)?;
    let trits = unpacked[prefix..prefix + (trit_end - trit_start)].to_vec();

    let bias_prefix = row_start % 4;
    let bias_first = row_start / 4;
    let bias_last = row_end.div_ceil(4);
    let raw = slice(bytes, info.bias_offset + bias_first, bias_last - bias_first)?;
    let unpacked = unpack_trits(raw, rows + bias_prefix)?;
    let bias = unpacked[bias_prefix..bias_prefix + rows].to_vec();

    let scales = read_f32s(bytes, info.scales_offset + row_start * 4, rows)?;
    let bias_f32 = match info.bias_f32_offset {
        Some(offset) => read_f32s(bytes, offset + row_start * 4, rows)?,
        None => Vec::new(),
    };
    Ok(RowsRaw {
        trits,
        bias,
        scales,
        bias_f32,
    })
}

fn read_rows_raw(
    bytes: &[u8],
    info: &LayerIndex,
    row_start: usize,
    rows: usize,
) -> Result<RowsRaw> {
    let row_end = row_start
        .checked_add(rows)
        .ok_or_else(|| Error::Wire("行范围溢出".into()))?;
    if rows == 0 || row_end > info.rows {
        return Err(Error::Wire("行范围越界".into()));
    }
    let trit_start = row_start
        .checked_mul(info.cols)
        .ok_or_else(|| Error::Wire("trit 起点溢出".into()))?;
    let trit_end = row_end
        .checked_mul(info.cols)
        .ok_or_else(|| Error::Wire("trit 终点溢出".into()))?;
    let first_byte = trit_start / 4;
    let last_byte = trit_end.div_ceil(4);
    let raw = slice(
        bytes,
        info.trits_offset + first_byte,
        last_byte - first_byte,
    )?;
    let prefix = trit_start % 4;
    let unpacked = unpack_trits(raw, trit_end - trit_start + prefix)?;
    let trits = unpacked[prefix..prefix + (trit_end - trit_start)].to_vec();

    let bias_prefix = row_start % 4;
    let bias_first = row_start / 4;
    let bias_last = row_end.div_ceil(4);
    let raw = slice(bytes, info.bias_offset + bias_first, bias_last - bias_first)?;
    let unpacked = unpack_trits(raw, rows + bias_prefix)?;
    let bias = unpacked[bias_prefix..bias_prefix + rows].to_vec();

    let scales = read_f32s(bytes, info.scales_offset + row_start * 4, rows)?;
    let bias_f32 = match info.bias_f32_offset {
        Some(offset) => read_f32s(bytes, offset + row_start * 4, rows)?,
        None => Vec::new(),
    };
    Ok(RowsRaw {
        trits,
        bias,
        scales,
        bias_f32,
    })
}

fn into_layer(info: &LayerIndex, raw: RowsRaw) -> Result<Layer> {
    let layer = Layer::with_scales(info.rows, info.cols, &raw.trits, &raw.bias, raw.scales)?;
    if info.has_bias_f32 {
        layer.with_bias_f32(raw.bias_f32)
    } else {
        Ok(layer)
    }
}

fn read_f32s(bytes: &[u8], offset: usize, count: usize) -> Result<Vec<f32>> {
    let len = count
        .checked_mul(4)
        .ok_or_else(|| Error::Wire("f32 区长度溢出".into()))?;
    let slice = slice(bytes, offset, len)?;
    let mut out = Vec::with_capacity(count);
    for chunk in slice.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(*chunk));
    }
    Ok(out)
}

fn slice(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| Error::Wire("索引偏移溢出".into()))?;
    bytes.get(offset..end).ok_or_else(|| {
        Error::Wire(format!(
            "索引越界：{offset}..{end} 超出文件 {} 字节",
            bytes.len()
        ))
    })
}

fn advance(bytes: &[u8], pos: usize, len: usize) -> Result<usize> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| Error::Wire("WSP2 偏移溢出".into()))?;
    if end > bytes.len() {
        return Err(Error::Wire(format!(
            "WSP2 截断：需要到 {end}，只有 {}",
            bytes.len()
        )));
    }
    Ok(end)
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = advance(bytes, *pos, len)?;
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

fn usize_field(value: &Value, name: &str) -> Result<usize> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|raw| usize::try_from(raw).ok())
        .ok_or_else(|| Error::Wire(format!("索引缺 {name} 或不是非负整数")))
}

fn usize_overflow() -> Error {
    Error::Wire("WSP2 偏移溢出".into())
}
