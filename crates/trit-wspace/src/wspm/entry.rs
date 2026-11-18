//! WSPM entry：条目结构、线格式解析与格式名。
//!
//! 从 `wspm/mod.rs` 拆出（模块预算"要腾就拆"）；线格式见模块文档。

use crate::error::{Error, Result};

/// 每个定长 entry 字段的字节数（method 到 len，不含 name）。
pub(super) const ENTRY_FIXED: usize = 1 + 1 + 4 + 8 + 8 + 8 + 8;
pub(super) const NO_BIAS: u32 = u32::MAX;

/// 容器里的一个张量条目（元数据 + blob 位置）。
#[derive(Clone, Debug, PartialEq)]
pub struct ModelEntry {
    /// 张量名（清单里的 `name`，容器内唯一）。
    pub name: String,
    /// blob 的压缩格式（0=WSP2、1=WSP3、2=WSP4）。
    pub method: u8,
    /// 单层网络的行数 / 列数（清单语义，解码时复核）。
    pub rows: usize,
    /// 单层网络的列数。
    pub cols: usize,
    /// 清单里的 `vector` 标记（1×cols 向量张量）。
    pub vector: bool,
    /// `bias` 指向的 entry 序号；`None` 表示无。
    pub bias: Option<usize>,
    /// blob 在文件里的绝对偏移与字节数。
    pub offset: u64,
    /// blob 的字节数。
    pub len: u64,
}

/// 从容器字节解析一个 entry（`pos` 推进到下一 entry 起点）。
pub(super) fn read_entry(bytes: &[u8], pos: &mut usize) -> Result<ModelEntry> {
    if *pos + 2 > bytes.len() {
        return Err(Error::Wire("WSPM entry 名长度截断".into()));
    }
    let name_len =
        u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().expect("2 字节名长")) as usize;
    *pos += 2;
    if *pos + name_len + ENTRY_FIXED > bytes.len() {
        return Err(Error::Wire("WSPM entry 截断".into()));
    }
    let name = std::str::from_utf8(&bytes[*pos..*pos + name_len])
        .map_err(|_| Error::Wire("WSPM 张量名不是 UTF-8".into()))?
        .to_string();
    *pos += name_len;
    let fixed = &bytes[*pos..*pos + ENTRY_FIXED];
    *pos += ENTRY_FIXED;
    let method = fixed[0];
    if method > 2 {
        return Err(Error::Wire(format!("`{name}` 的 method {method} 不受支持")));
    }
    let vector = fixed[1] != 0;
    let bias_raw = u32::from_le_bytes(fixed[2..6].try_into().expect("4 字节 bias_ref"));
    let read_u64 =
        |start: usize| u64::from_le_bytes(fixed[start..start + 8].try_into().expect("8 字节字段"));
    let (rows, cols) = (read_u64(6) as usize, read_u64(14) as usize);
    let (offset, len) = (read_u64(22), read_u64(30));
    if len == 0 {
        return Err(Error::Wire(format!("`{name}` 的 blob 为空")));
    }
    let bias = match bias_raw {
        NO_BIAS => None,
        index => Some(index as usize),
    };
    Ok(ModelEntry {
        name,
        method,
        rows,
        cols,
        vector,
        bias,
        offset,
        len,
    })
}
