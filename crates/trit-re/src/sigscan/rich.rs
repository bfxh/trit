//! PE Rich header（`pe.rich_signature.*`）解析。
//!
//! 结构（DOS stub 内）：`DanS` + 3×0 + N×(comp_id, count) + `Rich` + key；
//! 除 `Rich`/key 外每个 DWORD 与 key 异或。官方语义：
//! - `version` = 首条 entry 的 `comp_id >> 16`（构建版本号）；
//! - `key` / `offset`（clear_data 文件偏移）/ `length`（clear_data 字节数）；
//! - `clear_data` / `raw_data`（解密/原始字节串）。
//!
//! 无 Rich header = 确定的未定义（比较为假），不是证据缺失。本实现尚未
//! 与官方 yara-python 探针逐位对拍（本机无环境），语义按官方结构定义。

/// 解析出的 Rich header。
pub(super) struct RichSig {
    pub(super) key: u32,
    pub(super) version: u32,
    pub(super) offset: u64,
    pub(super) clear_data: Vec<u8>,
    pub(super) raw_data: Vec<u8>,
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// 从 PE 文件字节解析 Rich header；`None` = 没有/不完整。
pub(super) fn parse(data: &[u8]) -> Option<RichSig> {
    // e_lfanew（0x3c）→ PE header 起点；Rich 在其之前的 DOS stub 内。
    let pe_off = u32_at(data, 0x3c)? as usize;
    if pe_off <= 0x40 || pe_off > data.len() {
        return None;
    }
    // 官方口径：从后往前找最后一个 `Rich`（避免 stub 里的假标记）。
    let mut rich = None;
    let mut i = pe_off.saturating_sub(4);
    while i >= 0x40 {
        if data.get(i..i + 4) == Some(b"Rich") {
            rich = Some(i);
            break;
        }
        i -= 1;
    }
    let rich = rich?;
    let key = u32_at(data, rich + 4)?;
    // 向前按 DWORD 找 XOR key 解密的 `DanS`（0x536E6144）。
    let mut dans = None;
    let mut j = rich.saturating_sub(4);
    while j >= 0x40 {
        if u32_at(data, j)? ^ key == 0x536E_6144 {
            dans = Some(j);
            break;
        }
        j -= 4;
    }
    let dans = dans?;
    let raw_data = data.get(dans..rich)?.to_vec();
    let mut clear_data = Vec::with_capacity(raw_data.len());
    let (words, _rest) = raw_data.as_chunks::<4>();
    for c in words {
        let v = u32::from_le_bytes(*c) ^ key;
        clear_data.extend_from_slice(&v.to_le_bytes());
    }
    // 首条 entry 在 DanS + 3×padding 之后（偏移 16）。
    let version = u32_at(&clear_data, 16).map_or(0, |comp_id| comp_id >> 16);
    Some(RichSig {
        key,
        version,
        offset: dans as u64,
        clear_data,
        raw_data,
    })
}

/// 数值属性：`version` / `key` / `offset` / `length`。
pub(super) fn resolve_num(path: &str, data: &[u8]) -> Option<u64> {
    let sig = parse(data)?;
    match path {
        "pe.rich_signature.version" => Some(u64::from(sig.version)),
        "pe.rich_signature.key" => Some(u64::from(sig.key)),
        "pe.rich_signature.offset" => Some(sig.offset),
        "pe.rich_signature.length" => Some(sig.clear_data.len() as u64),
        _ => None,
    }
}

/// 字符串属性：`clear_data` / `raw_data`。
pub(super) fn resolve_str(path: &str, data: &[u8]) -> Option<Vec<u8>> {
    let sig = parse(data)?;
    match path {
        "pe.rich_signature.clear_data" => Some(sig.clear_data),
        "pe.rich_signature.raw_data" => Some(sig.raw_data),
        _ => None,
    }
}
