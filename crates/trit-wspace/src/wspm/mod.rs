//! WSPM：整模型批量容器——把一个 WSP1 模型目录（`manifest.json` + `NNNN-<name>.wsp`）
//! 打包成单个二进制文件，`unpack` 逐字节还原。
//!
//! 与 GGUF 容器（`trit-export`）不同：WSP2 blob 保留 `biasf`（f32 偏置幅度），
//! 往返是文本级无损的；GGUF 只存三值 bias，f32 幅度会丢（见 `docs/GGUF-TRIT.md` §4）。
//!
//! 布局（小端）：
//! ```text
//! magic "WSPM" | version u8=1 | flags u8=0 | count u32
//! 每 entry：name_len u16 | name | method u8 | vector u8 | bias_ref u32
//!           | rows u64 | cols u64 | offset u64 | len u64
//! blob 区：各 entry 的压缩工件（WSP2 / WSP3 / WSP4，按 entry 的 method）
//! 末尾：checksum u64（前面全部字节的 FNV-1a）
//! ```
//!
//! `offset` 是绝对文件偏移；`method`：0=WSP2、1=WSP3、2=WSP4；
//! `bias_ref` 指向同容器里的另一个 entry（清单 `bias` 字段的语义），`u32::MAX` 表示无。

mod entry;

use crate::error::{Error, Result};
use crate::net::Net;
use crate::wsp2::bytes::fnv;
use crate::wsp4::{decode_auto, encode};

pub use entry::ModelEntry;
use entry::read_entry;

const MAGIC: &[u8; 4] = b"WSPM";
const VERSION: u8 = 1;
const HEADER: usize = 10;
use entry::{ENTRY_FIXED, NO_BIAS};

/// 打包格式：`auto` 逐张量选最小的；其余固定。
pub const FORMATS: [&str; 4] = ["auto", "wsp2", "wsp3", "wsp4"];

/// 打包输入：一个名字 + 元数据 + 单层网络。
pub struct ModelItem<'a> {
    /// 张量名。
    pub name: &'a str,
    /// 是否向量张量。
    pub vector: bool,
    /// `bias` 引用的张量名（打包时解析成序号）。
    pub bias: Option<&'a str>,
    /// 单层网络。
    pub net: &'a Net,
}

/// 解码结果：条目 + 对应网络（与条目同序）。
pub struct ModelPack {
    /// 条目（按容器顺序）。
    pub entries: Vec<ModelEntry>,
    /// 每个条目解码出的单层网络。
    pub nets: Vec<Net>,
}

fn method_id(format: &str) -> Result<u8> {
    match format {
        "wsp2" => Ok(0),
        "wsp3" => Ok(1),
        "wsp4" => Ok(2),
        other => Err(Error::Wire(format!("未知打包格式 `{other}`"))),
    }
}

fn method_name(method: u8) -> &'static str {
    match method {
        1 => "wsp3",
        2 => "wsp4",
        _ => "wsp2",
    }
}

fn encode_blob(net: &Net, format: &str) -> Result<(u8, Vec<u8>)> {
    if format == "auto" {
        let wsp2 = crate::wsp2::to_wsp2(net)?;
        let wsp3 = crate::wsp3::to_wsp3(net)?;
        let wsp4 = crate::wsp4::to_wsp4(net)?;
        let best = [(&wsp3, 1u8), (&wsp4, 2u8)]
            .into_iter()
            .min_by_key(|(bytes, _)| bytes.len());
        return match best {
            Some((bytes, id)) if bytes.len() < wsp2.len() => Ok((id, bytes.clone())),
            _ => Ok((0, wsp2)),
        };
    }
    Ok((method_id(format)?, encode(net, format)?))
}

/// 打包结果：容器字节 + 每个张量最终选用的格式名。
pub struct PackOutput {
    /// 容器全部字节（含尾部校验和）。
    pub bytes: Vec<u8>,
    /// 每个张量的格式名（`wsp2` / `wsp3` / `wsp4`，与 items 同序）。
    pub formats: Vec<&'static str>,
}

/// 把模型目录的全部张量编码成一个 WSPM 容器。
///
/// `format` 取 [`FORMATS`] 之一；名字必须唯一，`bias` 引用必须能解析。
pub fn pack(items: &[ModelItem<'_>], format: &str) -> Result<PackOutput> {
    if !FORMATS.contains(&format) {
        return Err(Error::Wire(format!("未知打包格式 `{format}`")));
    }
    let mut entries = Vec::with_capacity(items.len());
    let mut blobs = Vec::with_capacity(items.len());
    let mut formats = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if item.name.is_empty() {
            return Err(Error::Wire(format!("第 {index} 个张量名为空")));
        }
        if entries
            .iter()
            .any(|entry: &ModelEntry| entry.name == item.name)
        {
            return Err(Error::Wire(format!("重复张量名 `{}`", item.name)));
        }
        let layer = item.net.layers().first().ok_or(Error::EmptySizes)?;
        let (method, blob) = encode_blob(item.net, format)?;
        formats.push(method_name(method));
        entries.push(ModelEntry {
            name: item.name.to_string(),
            method,
            rows: layer.rows(),
            cols: layer.cols(),
            vector: item.vector,
            bias: None,
            offset: 0,
            len: blob.len() as u64,
        });
        blobs.push(blob);
    }
    for (index, item) in items.iter().enumerate() {
        entries[index].bias = match item.bias {
            Some(name) => match items.iter().position(|item| item.name == name) {
                Some(target) => Some(target),
                None => {
                    return Err(Error::Wire(format!(
                        "`{}` 的 bias 引用 `{name}` 不在张量列表里",
                        item.name
                    )));
                }
            },
            None => None,
        };
    }
    let mut offset = HEADER
        + items
            .iter()
            .map(|item| 2 + item.name.len() + ENTRY_FIXED)
            .sum::<usize>();
    for (index, blob) in blobs.iter().enumerate() {
        entries[index].offset = offset as u64;
        offset += blob.len();
    }
    let mut out = Vec::with_capacity(offset + 8);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(0);
    let count = u32::try_from(items.len()).map_err(|_| Error::Wire("张量数超出 u32".into()))?;
    out.extend_from_slice(&count.to_le_bytes());
    for entry in &entries {
        let name = entry.name.as_bytes();
        let name_len =
            u16::try_from(name.len()).map_err(|_| Error::Wire("张量名超出 u16".into()))?;
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name);
        out.push(entry.method);
        out.push(u8::from(entry.vector));
        let bias = entry.bias.map_or(NO_BIAS, |index| index as u32);
        out.extend_from_slice(&bias.to_le_bytes());
        out.extend_from_slice(&(entry.rows as u64).to_le_bytes());
        out.extend_from_slice(&(entry.cols as u64).to_le_bytes());
        out.extend_from_slice(&entry.offset.to_le_bytes());
        out.extend_from_slice(&entry.len.to_le_bytes());
    }
    for blob in &blobs {
        out.extend_from_slice(blob);
    }
    let checksum = fnv(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(PackOutput {
        bytes: out,
        formats,
    })
}

/// 解码 WSPM 容器：magic/版本/条目/偏移/校验和/blob 全部校验。
pub fn unpack(bytes: &[u8]) -> Result<ModelPack> {
    if bytes.len() < HEADER + 8 {
        return Err(Error::Wire("WSPM 太短".into()));
    }
    if &bytes[..4] != MAGIC {
        return Err(Error::Wire("WSPM magic 不符".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Wire(format!("WSPM 版本 {} 不受支持", bytes[4])));
    }
    if bytes[5] != 0 {
        return Err(Error::Wire(format!("WSPM flags {} 不受支持", bytes[5])));
    }
    let payload_end = bytes.len() - 8;
    let expected = u64::from_le_bytes(bytes[payload_end..].try_into().expect("8 字节校验和"));
    let got = fnv(&bytes[..payload_end]);
    if got != expected {
        return Err(Error::Checksum { expected, got });
    }
    let count = u32::from_le_bytes(bytes[6..HEADER].try_into().expect("4 字节计数")) as usize;
    let mut pos = HEADER;
    let mut entries = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        entries.push(read_entry(bytes, &mut pos)?);
    }
    for (index, entry) in entries.iter().enumerate() {
        if let Some(target) = entry.bias {
            if target >= entries.len() {
                return Err(Error::Wire(format!(
                    "`{}` 的 bias 引用 {target} 越界（共 {} 个条目）",
                    entry.name,
                    entries.len()
                )));
            }
            if target == index {
                return Err(Error::Wire(format!("`{}` 的 bias 引用了自己", entry.name)));
            }
        }
        if entry.rows == 0 || entry.cols == 0 {
            return Err(Error::Wire(format!("`{}` 的形状为 0", entry.name)));
        }
        if entries[..index]
            .iter()
            .any(|other| other.name == entry.name)
        {
            return Err(Error::Wire(format!("重复张量名 `{}`", entry.name)));
        }
    }
    let mut nets = Vec::with_capacity(entries.len());
    for entry in &entries {
        let end = match entry.offset.checked_add(entry.len) {
            Some(end) if end <= payload_end as u64 => end,
            _ => {
                return Err(Error::Wire(format!(
                    "`{}` blob 越界（offset {} len {}）",
                    entry.name, entry.offset, entry.len
                )));
            }
        };
        let net = decode_auto(&bytes[entry.offset as usize..end as usize])
            .map_err(|error| Error::Wire(format!("`{}` 解码失败：{error}", entry.name)))?;
        let layer = net.layers().first().ok_or(Error::EmptySizes)?;
        if layer.rows() != entry.rows || layer.cols() != entry.cols {
            return Err(Error::Wire(format!(
                "`{}` 形状不符：声明 {}×{}，实际 {}×{}",
                entry.name,
                entry.rows,
                entry.cols,
                layer.rows(),
                layer.cols()
            )));
        }
        nets.push(net);
    }
    Ok(ModelPack { entries, nets })
}
