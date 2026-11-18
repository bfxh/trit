//! 三值权重 GGUF 容器（GGUF v3 最小子集）。
//!
//! GGUF 是 llama.cpp 生态的权重容器；本 crate 只实现**读写三值张量**所需的最小子集：
//! 文件头、少量元数据 KV、2D 张量信息、32 字节对齐的数据区。张量类型用自定义
//! `GGML_TYPE_TRIT = 100`（上游未占用，格式见 `docs/GGUF-TRIT.md`），数据布局：
//!
//! ```text
//! trits   : rows×cols，trit-codec 5 trits/byte（canonical padding，低位在前）
//! scales  : rows × f32（小端）
//! bias    : rows，trit-codec 5 trits/byte
//! ```
//!
//! 打包与 `trit-codec` **共用** `trit_core::pack` 的同一实现（线格式契约唯一），字节序、padding 规则都沿用该 crate 的线格式契约。本 crate 不是上游 GGUF 的完整实现：未知元数据类型、
//! 非三值张量、非 2D 张量一律显式报错，不猜。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::Ternary;

const MAGIC: &[u8; 4] = b"GGUF";
const VERSION: u32 = 3;
/// 本仓自定义三值张量类型（上游 GGML type id 0..=30，100 起未占用）。
pub const TYPE_TRIT: u32 = 100;
const ALIGNMENT: usize = 32;
const META_U32: u32 = 4;
const META_F32: u32 = 6;
const META_STR: u32 = 8;

/// GGUF 元数据值（最小子集）。
#[derive(Clone, Debug, PartialEq)]
pub enum MetaValue {
    /// `GGUF_METADATA_VALUE_TYPE_UINT32`。
    U32(u32),
    /// `GGUF_METADATA_VALUE_TYPE_FLOAT32`。
    F32(f32),
    /// `GGUF_METADATA_VALUE_TYPE_STRING`。
    Str(String),
}

/// 三值 2D 张量：行主序 trits + 每行 scale + 每行三值 bias。
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    /// 张量名（GGUF 内唯一）。
    pub name: String,
    /// 行数（输出维）。
    pub rows: usize,
    /// 列数（输入维）。
    pub cols: usize,
    /// `rows × cols` 个 trit（行主序）。
    pub trits: Vec<Ternary>,
    /// 每行一个 scale（`rows` 个）。
    pub scales: Vec<f32>,
    /// 每行一个三值 bias（`rows` 个）。
    pub bias: Vec<Ternary>,
}

/// 一个 GGUF 模型：元数据 + 三值张量。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    /// 元数据（按 key 排序，保证输出确定性）。
    pub metadata: BTreeMap<String, MetaValue>,
    /// 张量列表（按写入顺序）。
    pub tensors: Vec<Tensor>,
}

mod error;
pub use error::Error;

/// 单张量数据区大小：trits + scales + bias。
#[must_use]
pub fn tensor_size(rows: usize, cols: usize) -> usize {
    rows.saturating_mul(cols).div_ceil(5) + rows.saturating_mul(4) + rows.div_ceil(5)
}

fn align(value: usize) -> usize {
    value.div_ceil(ALIGNMENT) * ALIGNMENT
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, value: &str) -> Result<(), Error> {
    put_u64(
        out,
        u64::try_from(value.len()).map_err(|_| Error::Field(value.into()))?,
    );
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn tensor_body(tensor: &Tensor) -> Result<Vec<u8>, Error> {
    if tensor.rows == 0 || tensor.cols == 0 {
        return Err(Error::Shape(tensor.rows, tensor.cols));
    }
    if tensor.trits.len() != tensor.rows.saturating_mul(tensor.cols)
        || tensor.scales.len() != tensor.rows
        || tensor.bias.len() != tensor.rows
    {
        return Err(Error::Field(tensor.name.clone()));
    }
    let mut out = Vec::with_capacity(tensor_size(tensor.rows, tensor.cols));
    out.extend_from_slice(&trit_core::pack::pack(&tensor.trits));
    for scale in &tensor.scales {
        put_f32(&mut out, *scale);
    }
    out.extend_from_slice(&trit_core::pack::pack(&tensor.bias));
    Ok(out)
}

/// 把模型写成 GGUF v3 字节流（32 字节对齐）。
pub fn write(model: &Model) -> Result<Vec<u8>, Error> {
    let mut names = BTreeSet::new();
    for tensor in &model.tensors {
        tensor_body(tensor)?;
        if !names.insert(tensor.name.clone()) {
            return Err(Error::Duplicate(tensor.name.clone()));
        }
    }
    let mut meta = Vec::new();
    for (key, value) in &model.metadata {
        put_str(&mut meta, key)?;
        match value {
            MetaValue::U32(number) => {
                put_u32(&mut meta, META_U32);
                put_u32(&mut meta, *number);
            }
            MetaValue::F32(number) => {
                put_u32(&mut meta, META_F32);
                put_f32(&mut meta, *number);
            }
            MetaValue::Str(text) => {
                put_u32(&mut meta, META_STR);
                put_str(&mut meta, text)?;
            }
        }
    }
    let mut infos = Vec::new();
    let mut bodies = Vec::with_capacity(model.tensors.len());
    let mut offset = 0usize;
    for tensor in &model.tensors {
        put_str(&mut infos, &tensor.name)?;
        put_u32(&mut infos, 2);
        put_u64(
            &mut infos,
            u64::try_from(tensor.cols).map_err(|_| Error::Field(tensor.name.clone()))?,
        );
        put_u64(
            &mut infos,
            u64::try_from(tensor.rows).map_err(|_| Error::Field(tensor.name.clone()))?,
        );
        put_u32(&mut infos, TYPE_TRIT);
        put_u64(
            &mut infos,
            u64::try_from(offset).map_err(|_| Error::Field(tensor.name.clone()))?,
        );
        let body = tensor_body(tensor)?;
        offset = align(offset.saturating_add(body.len()));
        bodies.push(body);
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, VERSION);
    put_u64(
        &mut out,
        u64::try_from(model.tensors.len()).map_err(|_| Error::Truncated)?,
    );
    put_u64(
        &mut out,
        u64::try_from(model.metadata.len()).map_err(|_| Error::Truncated)?,
    );
    out.extend_from_slice(&meta);
    out.extend_from_slice(&infos);
    while out.len() % ALIGNMENT != 0 {
        out.push(0);
    }
    for body in bodies {
        out.extend_from_slice(&body);
        while out.len() % ALIGNMENT != 0 {
            out.push(0);
        }
    }
    Ok(out)
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], Error> {
    let end = pos.checked_add(len).ok_or(Error::Truncated)?;
    if end > bytes.len() {
        return Err(Error::Truncated);
    }
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        take(bytes, pos, 4)?.try_into().expect("4 字节"),
    ))
}

fn read_u64(bytes: &[u8], pos: &mut usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        take(bytes, pos, 8)?.try_into().expect("8 字节"),
    ))
}

fn read_f32(bytes: &[u8], pos: &mut usize) -> Result<f32, Error> {
    Ok(f32::from_le_bytes(
        take(bytes, pos, 4)?.try_into().expect("4 字节"),
    ))
}

fn read_str(bytes: &[u8], pos: &mut usize) -> Result<String, Error> {
    let len = usize::try_from(read_u64(bytes, pos)?).map_err(|_| Error::Truncated)?;
    let raw = take(bytes, pos, len)?;
    String::from_utf8(raw.to_vec()).map_err(|_| Error::Utf8)
}

struct Info {
    name: String,
    rows: usize,
    cols: usize,
    offset: usize,
}

/// 从 GGUF v3 字节流读取模型；未知类型/维度/越界一律报错。
pub fn read(bytes: &[u8]) -> Result<Model, Error> {
    let mut pos = 0usize;
    if take(bytes, &mut pos, 4)? != MAGIC {
        return Err(Error::Magic);
    }
    let version = read_u32(bytes, &mut pos)?;
    if version != VERSION {
        return Err(Error::Version(version));
    }
    let tensor_count = usize::try_from(read_u64(bytes, &mut pos)?).map_err(|_| Error::Truncated)?;
    let meta_count = usize::try_from(read_u64(bytes, &mut pos)?).map_err(|_| Error::Truncated)?;
    let mut metadata = BTreeMap::new();
    for _ in 0..meta_count {
        let key = read_str(bytes, &mut pos)?;
        let kind = read_u32(bytes, &mut pos)?;
        let value = match kind {
            META_U32 => MetaValue::U32(read_u32(bytes, &mut pos)?),
            META_F32 => MetaValue::F32(read_f32(bytes, &mut pos)?),
            META_STR => MetaValue::Str(read_str(bytes, &mut pos)?),
            other => return Err(Error::MetaType(other)),
        };
        metadata.insert(key, value);
    }
    let mut infos = Vec::with_capacity(tensor_count);
    for _ in 0..tensor_count {
        let name = read_str(bytes, &mut pos)?;
        let dims = read_u32(bytes, &mut pos)?;
        if dims != 2 {
            return Err(Error::Dims(dims));
        }
        let cols = usize::try_from(read_u64(bytes, &mut pos)?).map_err(|_| Error::Truncated)?;
        let rows = usize::try_from(read_u64(bytes, &mut pos)?).map_err(|_| Error::Truncated)?;
        let kind = read_u32(bytes, &mut pos)?;
        if kind != TYPE_TRIT {
            return Err(Error::TensorType(kind));
        }
        let offset = usize::try_from(read_u64(bytes, &mut pos)?).map_err(|_| Error::Truncated)?;
        if rows == 0 || cols == 0 {
            return Err(Error::Shape(rows, cols));
        }
        infos.push(Info {
            name,
            rows,
            cols,
            offset,
        });
    }
    let data_start = align(pos);
    let mut tensors = Vec::with_capacity(infos.len());
    for info in infos {
        let start = data_start
            .checked_add(info.offset)
            .ok_or_else(|| Error::Tensor(info.name.clone()))?;
        let trit_len = info.rows.saturating_mul(info.cols);
        let packed = trit_len.div_ceil(5);
        let bias_bytes = info.rows.div_ceil(5);
        let end = start
            .checked_add(packed)
            .and_then(|value| value.checked_add(info.rows.saturating_mul(4)))
            .and_then(|value| value.checked_add(bias_bytes))
            .ok_or_else(|| Error::Tensor(info.name.clone()))?;
        if end > bytes.len() {
            return Err(Error::Tensor(info.name));
        }
        let trits = trit_core::pack::unpack_canonical(&bytes[start..start + packed], trit_len)
            .map_err(|error| Error::Tensor(format!("{}: {error}", info.name)))?;
        let scale_start = start + packed;
        let scales = (0..info.rows)
            .map(|row| {
                let begin = scale_start + row * 4;
                f32::from_le_bytes(bytes[begin..begin + 4].try_into().expect("4 字节"))
            })
            .collect();
        let bias =
            trit_core::pack::unpack_canonical(&bytes[scale_start + info.rows * 4..end], info.rows)
                .map_err(|error| Error::Tensor(format!("{}: {error}", info.name)))?;
        tensors.push(Tensor {
            name: info.name,
            rows: info.rows,
            cols: info.cols,
            trits,
            scales,
            bias,
        });
    }
    Ok(Model { metadata, tensors })
}
