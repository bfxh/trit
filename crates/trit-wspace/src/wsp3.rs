//! WSP3：WSP2 的 RLE 二级压缩容器（PackBits，无外部依赖）。
//!
//! WSP2 是 2-bit 定长容器，trit 全同的长段（大量 `0` / 同一符号）仍然逐字节占位；
//! WSP3 在 WSP2 外面加一层字节级 RLE：`raw_len u64` 记录解压后的长度，压缩流用
//! PackBits（literal 1–128 字节 / 重复 3–130 字节），末尾带 FNV-1a 校验和。
//!
//! 布局（小端）：
//! ```text
//! magic "WSP3" | version u8=1 | raw_len u64 | PackBits 流 | checksum u64
//! ```
//!
//! 它仍是**缓存/传输格式**：解包先还原 WSP2，再回到 WSP1 可读工件。
//! **实测边界**（`docs/WSP3-RESULTS.md`）：真实 MiniLM 103 张量上比 WSP2 大 0.7%（平均游程 1.5、熵 1.584 bit/trit），只对低熵/全同工件有效。

use crate::error::{Error, Result};
use crate::net::Net;
use crate::wsp2::bytes::fnv;
use crate::wsp2::{from_wsp2, to_wsp2};

const MAGIC: &[u8; 4] = b"WSP3";
const VERSION: u8 = 1;
const HEADER: usize = 4 + 1 + 8;

/// PackBits 编码：literal 1–128 字节（header 0..=127）与重复 3–130 次（header `0x80|(n-3)` + 1 字节）。
#[must_use]
pub fn rle_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < input.len() {
        let mut run = 1;
        while index + run < input.len() && input[index + run] == input[index] && run < 130 {
            run += 1;
        }
        if run >= 3 {
            out.push(0x80 | u8::try_from(run - 3).expect("run <= 130"));
            out.push(input[index]);
            index += run;
            continue;
        }
        let start = index;
        let mut end = index;
        while end < input.len() && end - start < 128 {
            if end + 2 < input.len() && input[end] == input[end + 1] && input[end] == input[end + 2]
            {
                break;
            }
            end += 1;
        }
        out.push(u8::try_from(end - start - 1).expect("literal <= 128"));
        out.extend_from_slice(&input[start..end]);
        index = end;
    }
    out
}

/// PackBits 解码；解压长度必须等于 `expected`，超出/不足都报错。
pub fn rle_decode(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    let mut index = 0;
    while index < input.len() {
        let header = input[index];
        index += 1;
        if header & 0x80 == 0 {
            let len = usize::from(header) + 1;
            let end = index
                .checked_add(len)
                .ok_or_else(|| Error::Wire("WSP3 literal 长度溢出".into()))?;
            if end > input.len() {
                return Err(Error::Wire("WSP3 literal 截断".into()));
            }
            out.extend_from_slice(&input[index..end]);
            index = end;
        } else {
            let len = usize::from(header & 0x7F) + 3;
            let byte = *input
                .get(index)
                .ok_or_else(|| Error::Wire("WSP3 重复字节缺失".into()))?;
            index += 1;
            out.extend(std::iter::repeat_n(byte, len));
        }
        if out.len() > expected {
            return Err(Error::Wire(format!("WSP3 解包超过原始长度 {expected}")));
        }
    }
    if out.len() != expected {
        return Err(Error::Wire(format!(
            "WSP3 解包长度 {} 与原始长度 {expected} 不符",
            out.len()
        )));
    }
    Ok(out)
}

/// 把网络编码成 WSP3（先 WSP2，再 RLE）。
pub fn to_wsp3(net: &Net) -> Result<Vec<u8>> {
    let raw = to_wsp2(net)?;
    let packed = rle_encode(&raw);
    let mut out = Vec::with_capacity(HEADER + packed.len() + 8);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(
        &u64::try_from(raw.len())
            .expect("usize <= u64")
            .to_le_bytes(),
    );
    out.extend_from_slice(&packed);
    let checksum = fnv(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

/// 按 `rle` 选择 WSP2 / WSP3 编码（CLI `--rle` 的入口）。
pub fn encode(net: &Net, rle: bool) -> Result<Vec<u8>> {
    if rle { to_wsp3(net) } else { to_wsp2(net) }
}

/// 按 magic 自动识别 WSP2 / WSP3（CLI `unpack-artifact` 的入口）。
pub fn decode_auto(bytes: &[u8]) -> Result<Net> {
    if bytes.starts_with(MAGIC) {
        from_wsp3(bytes)
    } else {
        from_wsp2(bytes)
    }
}

/// 从 WSP3 解码网络；magic/版本/长度/校验和/解压都会校验。
pub fn from_wsp3(bytes: &[u8]) -> Result<Net> {
    if bytes.len() < HEADER + 8 {
        return Err(Error::Wire("WSP3 太短".into()));
    }
    if &bytes[..4] != MAGIC {
        return Err(Error::Wire("WSP3 magic 不符".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Wire(format!("WSP3 版本 {} 不受支持", bytes[4])));
    }
    let payload_end = bytes.len() - 8;
    let expected = u64::from_le_bytes(bytes[payload_end..].try_into().expect("8 字节校验和"));
    let got = fnv(&bytes[..payload_end]);
    if got != expected {
        return Err(Error::Checksum { expected, got });
    }
    let raw_len = u64::from_le_bytes(bytes[5..HEADER].try_into().expect("8 字节长度"));
    let raw_len =
        usize::try_from(raw_len).map_err(|_| Error::Wire("WSP3 原始长度超出 usize".into()))?;
    let raw = rle_decode(&bytes[HEADER..payload_end], raw_len)?;
    from_wsp2(&raw)
}
