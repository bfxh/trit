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

mod rle;
pub use rle::{rle_decode, rle_encode};

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

/// CLI 的容器名：`huffman` 优先于 `rle`，都没有时 WSP2。
#[must_use]
pub fn pack_format(huffman: bool, rle: bool) -> &'static str {
    if huffman {
        "wsp4"
    } else if rle {
        "wsp3"
    } else {
        "wsp2"
    }
}
