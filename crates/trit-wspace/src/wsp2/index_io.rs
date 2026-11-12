//! WSP2 块索引的流式读取（`Read + Seek`）：只 seek/read 目标块所需的字节。

use std::io::{Read, Seek, SeekFrom};

use crate::error::{Error, Result};
use crate::net::Layer;

use super::bytes::unpack_trits;
use super::index::BlockIndex;

impl BlockIndex {
    /// 从 `Read + Seek` 读整层；只 seek/read 该层数据。
    pub fn read_layer_io<R: Read + Seek>(&self, reader: &mut R, layer: usize) -> Result<Layer> {
        let info = self.layer(layer)?;
        self.check_source_io(reader)?;
        let trit_len = info
            .rows
            .checked_mul(info.cols)
            .ok_or_else(|| Error::Wire("WSP2 元素数溢出".into()))?
            .div_ceil(4);
        let raw = read_at(reader, info.trits_offset, trit_len)?;
        let trits = unpack_trits(&raw, info.rows * info.cols)?;
        let raw = read_at(reader, info.bias_offset, info.rows.div_ceil(4))?;
        let bias = unpack_trits(&raw, info.rows)?;
        let scales = read_f32s_at(reader, info.scales_offset, info.rows)?;
        let layer = Layer::with_scales(info.rows, info.cols, &trits, &bias, scales)?;
        match info.bias_f32_offset {
            Some(offset) => layer.with_bias_f32(read_f32s_at(reader, offset, info.rows)?),
            None => Ok(layer),
        }
    }

    /// 从 `Read + Seek` 读 `row_start..row_start+rows`；只读覆盖到的块。
    pub fn read_rows_io<R: Read + Seek>(
        &self,
        reader: &mut R,
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
        self.check_source_io(reader)?;
        if row_start == 0 && rows == info.rows {
            return self.read_layer_io(reader, layer);
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
            let relative_start = row - block.row_start;
            let relative_end = relative_start + take;
            let trit_start = relative_start
                .checked_mul(info.cols)
                .ok_or_else(|| Error::Wire("trit 起点溢出".into()))?;
            let trit_end = relative_end
                .checked_mul(info.cols)
                .ok_or_else(|| Error::Wire("trit 终点溢出".into()))?;
            let first = trit_start / 4;
            let last = trit_end.div_ceil(4);
            if last > block.trits_len {
                return Err(Error::Wire("行块读取超出索引长度".into()));
            }
            let raw = read_at(reader, block.trits_offset + first, last - first)?;
            let prefix = trit_start % 4;
            let unpacked = unpack_trits(&raw, trit_end - trit_start + prefix)?;
            trits.extend_from_slice(&unpacked[prefix..prefix + (trit_end - trit_start)]);

            let bias_prefix = row % 4;
            let bias_first = row / 4;
            let bias_last = (row + take).div_ceil(4);
            let raw = read_at(
                reader,
                info.bias_offset + bias_first,
                bias_last - bias_first,
            )?;
            let unpacked = unpack_trits(&raw, take + bias_prefix)?;
            bias.extend_from_slice(&unpacked[bias_prefix..bias_prefix + take]);

            scales.extend(read_f32s_at(reader, info.scales_offset + row * 4, take)?);
            if let Some(offset) = info.bias_f32_offset {
                bias_f32.extend(read_f32s_at(reader, offset + row * 4, take)?);
            }
            row += take;
        }
        let layer = Layer::with_scales(rows, info.cols, &trits, &bias, scales)?;
        if info.has_bias_f32 {
            layer.with_bias_f32(bias_f32)
        } else {
            Ok(layer)
        }
    }

    /// 流式源检查：只 seek 到尾部读 8 字节。
    fn check_source_io<R: Read + Seek>(&self, reader: &mut R) -> Result<()> {
        let len = reader
            .seek(SeekFrom::End(0))
            .map_err(|error| Error::Wire(format!("读 WSP2 长度失败：{error}")))?;
        if len != self.source_bytes as u64 {
            return Err(Error::Wire(format!(
                "源文件 {len} 字节，索引记录 {} 字节",
                self.source_bytes
            )));
        }
        if len < 8 {
            return Err(Error::Wire("WSP2 太短".into()));
        }
        reader
            .seek(SeekFrom::End(-8))
            .map_err(|error| Error::Wire(format!("seek 校验和失败：{error}")))?;
        let mut tail = [0u8; 8];
        reader
            .read_exact(&mut tail)
            .map_err(|error| Error::Wire(format!("读校验和失败：{error}")))?;
        let got = u64::from_le_bytes(tail);
        if got != self.source_checksum {
            return Err(Error::Checksum {
                expected: self.source_checksum,
                got,
            });
        }
        Ok(())
    }
}

fn read_at<R: Read + Seek>(reader: &mut R, offset: usize, len: usize) -> Result<Vec<u8>> {
    let start = u64::try_from(offset).map_err(|_| Error::Wire("偏移超出 u64".into()))?;
    reader
        .seek(SeekFrom::Start(start))
        .map_err(|error| Error::Wire(format!("seek {offset} 失败：{error}")))?;
    let mut buffer = vec![0u8; len];
    reader.read_exact(&mut buffer).map_err(|error| {
        Error::Wire(format!(
            "读 {offset}..{} 失败：{error}",
            offset.saturating_add(len)
        ))
    })?;
    Ok(buffer)
}

fn read_f32s_at<R: Read + Seek>(reader: &mut R, offset: usize, count: usize) -> Result<Vec<f32>> {
    let len = count
        .checked_mul(4)
        .ok_or_else(|| Error::Wire("f32 区长度溢出".into()))?;
    let raw = read_at(reader, offset, len)?;
    let mut out = Vec::with_capacity(count);
    for chunk in raw.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(*chunk));
    }
    Ok(out)
}
