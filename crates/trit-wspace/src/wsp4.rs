//! WSP4：WSP2 的静态 Huffman 二级压缩（字节级，无外部依赖）。
//!
//! 实测（`docs/WSP3-RESULTS.md`）：真实 MiniLM 的 WSP2 字节熵 **6.561 bit/byte**，
//! 固定 8 bit 存字节，RLE 无收益；静态 Huffman 的理论下限约 **82.0% of WSP2**。
//! WSP4 在 WSP2 外面做一层字节级 Huffman：256 个码长 + MSB-first 位流；
//! 压缩后不更小（或码长超过 32）时回退 method=0 的原样存储。
//!
//! 布局（小端）：
//! ```text
//! magic "WSP4" | version u8=1 | method u8 | raw_len u64
//! method=1: 256×u8 码长 | 位流
//! method=0: 原始 WSP2 字节
//! checksum u64
//! ```

mod bitwriter;

use std::cmp::Reverse;

use bitwriter::BitWriter;
use std::collections::{BinaryHeap, HashMap};

use crate::error::{Error, Result};
use crate::net::Net;
use crate::wsp2::bytes::fnv;
use crate::wsp2::{from_wsp2, to_wsp2};

const MAGIC: &[u8; 4] = b"WSP4";
const VERSION: u8 = 1;
const HEADER: usize = 4 + 1 + 1 + 8;
const MAX_CODE_LEN: usize = 32;

/// 字节级 Huffman 编码；返回 `256 码长 + 位流`，无法压缩（码长 > 32）时返回 `None`。
#[must_use]
pub fn huffman_encode(input: &[u8]) -> Option<Vec<u8>> {
    if input.is_empty() {
        return Some(vec![0u8; 256]);
    }
    let mut freq = [0u64; 256];
    for &byte in input {
        freq[usize::from(byte)] += 1;
    }
    let lens = code_lengths(&freq)?;
    let codes = canonical_codes(&lens).ok()?;
    let mut out = Vec::with_capacity(256 + input.len());
    out.extend_from_slice(&lens);
    let mut writer = BitWriter::new();
    for &byte in input {
        let (code, len) = codes[usize::from(byte)];
        writer.write(code, len);
    }
    writer.finish(&mut out);
    Some(out)
}

/// 按 256 码长 + 位流解码出 `raw_len` 个符号。
pub fn huffman_decode(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    if data.len() < 256 {
        return Err(Error::Wire("WSP4 码长表截断".into()));
    }
    // 分配护栏：每个符号至少 1 bit，声明符号数不得超过位流容量。
    // 否则声明 raw_len=2^40 的 266 字节文件就能让 with_capacity 试图分配 TB 级内存。
    if raw_len > (data.len() - 256).saturating_mul(8) {
        return Err(Error::Wire(format!(
            "WSP4 声明长度 {raw_len} 超出位流 {} 字节的符号上界",
            data.len() - 256
        )));
    }
    let lens: [u8; 256] = data[..256].try_into().expect("256 字节码长");
    let used = lens.iter().filter(|&&len| len > 0).count();
    if used == 0 {
        return if raw_len == 0 {
            Ok(Vec::new())
        } else {
            Err(Error::Wire("WSP4 空码表但 raw_len > 0".into()))
        };
    }
    let table = decode_table(&lens)?;
    let bits = &data[256..];
    let mut out = Vec::with_capacity(raw_len);
    let mut code = 0u32;
    let mut len = 0u8;
    let mut pos = 0usize;
    while out.len() < raw_len {
        let byte = *bits
            .get(pos / 8)
            .ok_or_else(|| Error::Wire("WSP4 位流截断".into()))?;
        let bit = (byte >> (7 - (pos % 8))) & 1;
        pos += 1;
        code = (code << 1) | u32::from(bit);
        len += 1;
        if len > MAX_CODE_LEN as u8 {
            return Err(Error::Wire("WSP4 码长越界".into()));
        }
        if let Some(&symbol) = table.get(&(len, code)) {
            out.push(symbol);
            code = 0;
            len = 0;
        }
    }
    Ok(out)
}

/// 把网络编码成 WSP4（先 WSP2，再 Huffman；压缩不更小就原样存）。
pub fn to_wsp4(net: &Net) -> Result<Vec<u8>> {
    let raw = to_wsp2(net)?;
    let packed = huffman_encode(&raw);
    let (method, body) = match packed {
        Some(body) if body.len() < raw.len() => (1u8, body),
        _ => (0u8, raw.clone()),
    };
    let mut out = Vec::with_capacity(HEADER + body.len() + 8);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(method);
    out.extend_from_slice(
        &u64::try_from(raw.len())
            .expect("usize <= u64")
            .to_le_bytes(),
    );
    out.extend_from_slice(&body);
    let checksum = fnv(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

/// 从 WSP4 解码网络；magic/版本/method/长度/校验和/位流都会校验。
pub fn from_wsp4(bytes: &[u8]) -> Result<Net> {
    if bytes.len() < HEADER + 8 {
        return Err(Error::Wire("WSP4 太短".into()));
    }
    if &bytes[..4] != MAGIC {
        return Err(Error::Wire("WSP4 magic 不符".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Wire(format!("WSP4 版本 {} 不受支持", bytes[4])));
    }
    let method = bytes[5];
    let payload_end = bytes.len() - 8;
    let expected = u64::from_le_bytes(bytes[payload_end..].try_into().expect("8 字节校验和"));
    let got = fnv(&bytes[..payload_end]);
    if got != expected {
        return Err(Error::Checksum { expected, got });
    }
    let raw_len = u64::from_le_bytes(bytes[6..HEADER].try_into().expect("8 字节长度"));
    let raw_len =
        usize::try_from(raw_len).map_err(|_| Error::Wire("WSP4 原始长度超出 usize".into()))?;
    let body = &bytes[HEADER..payload_end];
    let raw = match method {
        0 => {
            if body.len() != raw_len {
                return Err(Error::Wire(format!(
                    "WSP4 原样段长度 {} 与 raw_len {raw_len} 不符",
                    body.len()
                )));
            }
            body.to_vec()
        }
        1 => huffman_decode(body, raw_len)?,
        other => return Err(Error::Wire(format!("WSP4 method {other} 不受支持"))),
    };
    from_wsp2(&raw)
}

/// 从符号频率构造码长（限制最大 32）；超出返回 `None`。
fn code_lengths(freq: &[u64; 256]) -> Option<[u8; 256]> {
    let used: Vec<usize> = (0..256).filter(|&index| freq[index] > 0).collect();
    if used.is_empty() {
        return Some([0u8; 256]);
    }
    let mut lens = [0u8; 256];
    if used.len() == 1 {
        lens[used[0]] = 1;
        return Some(lens);
    }
    #[derive(Clone)]
    struct Node {
        freq: u64,
        left: Option<usize>,
        right: Option<usize>,
        symbol: Option<usize>,
    }
    let mut nodes: Vec<Node> = used
        .iter()
        .map(|&symbol| Node {
            freq: freq[symbol],
            left: None,
            right: None,
            symbol: Some(symbol),
        })
        .collect();
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| Reverse((node.freq, index)))
        .collect();
    while heap.len() > 1 {
        let Reverse((left_freq, left)) = heap.pop().expect("非空");
        let Reverse((right_freq, right)) = heap.pop().expect("非空");
        let index = nodes.len();
        nodes.push(Node {
            freq: left_freq + right_freq,
            left: Some(left),
            right: Some(right),
            symbol: None,
        });
        heap.push(Reverse((left_freq + right_freq, index)));
    }
    let root = heap.pop().expect("非空").0.1;
    let mut stack = vec![(root, 0usize)];
    while let Some((index, depth)) = stack.pop() {
        let node = &nodes[index];
        if let Some(symbol) = node.symbol {
            if depth == 0 || depth > MAX_CODE_LEN {
                return None;
            }
            lens[symbol] = u8::try_from(depth).expect("depth <= 32");
        } else {
            stack.push((node.left.expect("内部节点有左子"), depth + 1));
            stack.push((node.right.expect("内部节点有右子"), depth + 1));
        }
    }
    Some(lens)
}

/// 规范 Huffman 码：按 (码长, 符号) 排序后依次分配递增码字。
fn canonical_codes(lens: &[u8; 256]) -> Result<[(u32, u8); 256]> {
    let mut pairs: Vec<(u8, usize)> = lens
        .iter()
        .enumerate()
        .filter(|&(_, &len)| len > 0)
        .map(|(symbol, &len)| (len, symbol))
        .collect();
    pairs.sort();
    let mut codes = [(0u32, 0u8); 256];
    let mut code = 0u32;
    let mut prev = 0u8;
    for (len, symbol) in pairs {
        if len > MAX_CODE_LEN as u8 || len < prev {
            return Err(Error::Wire("WSP4 码长非法".into()));
        }
        code <<= u32::from(len - prev);
        if code >> len != 0 {
            return Err(Error::Wire("WSP4 码表溢出（Kraft 不等式）".into()));
        }
        codes[symbol] = (code, len);
        code += 1;
        prev = len;
    }
    Ok(codes)
}

fn decode_table(lens: &[u8; 256]) -> Result<HashMap<(u8, u32), u8>> {
    let codes = canonical_codes(lens)?;
    let mut table = HashMap::with_capacity(256);
    for (symbol, &(code, len)) in codes.iter().enumerate() {
        if len > 0 {
            table.insert((len, code), u8::try_from(symbol).expect("symbol <= 255"));
        }
    }
    Ok(table)
}

/// 按 format 选择 WSP2 / WSP3 / WSP4 编码（CLI `--rle` / `--huffman` 的入口）。
pub fn encode(net: &Net, format: &str) -> Result<Vec<u8>> {
    match format {
        "wsp3" => crate::wsp3::to_wsp3(net),
        "wsp4" => to_wsp4(net),
        _ => to_wsp2(net),
    }
}

/// 按 magic 自动识别 WSP2 / WSP3 / WSP4（CLI `unpack-artifact` 的入口）。
pub fn decode_auto(bytes: &[u8]) -> Result<Net> {
    if bytes.starts_with(b"WSP3") {
        crate::wsp3::from_wsp3(bytes)
    } else if bytes.starts_with(MAGIC) {
        from_wsp4(bytes)
    } else {
        from_wsp2(bytes)
    }
}
