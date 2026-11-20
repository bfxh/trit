//! GGUF v3 编解码（从 `lib.rs` 拆出）：本仓 `TYPE_TRIT`（per-row）与上游 `TYPE_I2_S`
//! （2 bit/trit + per-tensor scale + 32 B 尾）。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::Ternary;

use crate::bitnet::i2s;
use crate::{Error, MetaValue, Model, TYPE_I2_S, TYPE_TRIT, Tensor};

const MAGIC: &[u8; 4] = b"GGUF";
const VERSION: u32 = 3;
const ALIGNMENT: usize = 32;
/// 单文件张量数上限（与 `trit-texture` 的 GGUF 导入器同口径）。
const MAX_TENSORS: usize = 100_000;
const META_U32: u32 = 4;
const META_F32: u32 = 6;
const META_STR: u32 = 8;
/// I2_S 的 32 字节尾（前 4 字节 f32 scale，其余在真实文件里非零，读侧不依赖）。
const I2S_TAIL: usize = 32;

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

/// 生成张量体并返回它的 GGUF 类型码。
///
/// `scales` 有 `rows` 个 → `TYPE_TRIT`（per-row）；只有 1 个且 `rows > 1` → `TYPE_I2_S`。
fn tensor_body(tensor: &Tensor) -> Result<(u32, Vec<u8>), Error> {
    if tensor.rows == 0 || tensor.cols == 0 {
        return Err(Error::Shape(tensor.rows, tensor.cols));
    }
    let per_tensor = tensor.scales.len() == 1 && tensor.rows > 1;
    if tensor.trits.len() != tensor.rows.saturating_mul(tensor.cols)
        || (!per_tensor && tensor.scales.len() != tensor.rows)
        || tensor.bias.len() != tensor.rows
    {
        return Err(Error::Field(tensor.name.clone()));
    }
    if per_tensor {
        let mut out = i2s::pack(&tensor.trits)
            .map_err(|error| Error::Tensor(format!("{}: {error}", tensor.name)))?;
        let payload = out.len();
        put_f32(&mut out, tensor.scales[0]);
        out.resize(payload + I2S_TAIL, 0);
        Ok((TYPE_I2_S, out))
    } else {
        let mut out = Vec::with_capacity(crate::tensor_size(tensor.rows, tensor.cols));
        out.extend_from_slice(&trit_core::pack::pack(&tensor.trits));
        for scale in &tensor.scales {
            put_f32(&mut out, *scale);
        }
        out.extend_from_slice(&trit_core::pack::pack(&tensor.bias));
        Ok((TYPE_TRIT, out))
    }
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
        let (kind, body) = tensor_body(tensor)?;
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
        put_u32(&mut infos, kind);
        put_u64(
            &mut infos,
            u64::try_from(offset).map_err(|_| Error::Field(tensor.name.clone()))?,
        );
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
    kind: u32,
    offset: usize,
}

/// 读一个 I2_S 张量：2 bit/trit + 32 字节尾（前 4 字节 f32 是该张量的 per-tensor scale）。
fn read_i2s(bytes: &[u8], start: usize, info: &Info) -> Result<Tensor, Error> {
    let count = info.rows.saturating_mul(info.cols);
    let end = start
        .checked_add(count / 4)
        .and_then(|value| value.checked_add(I2S_TAIL))
        .ok_or_else(|| Error::Tensor(info.name.clone()))?;
    if end > bytes.len() {
        return Err(Error::Tensor(info.name.clone()));
    }
    let trits = i2s::unpack(&bytes[start..start + count / 4], count)
        .map_err(|error| Error::Tensor(format!("{}: {error}", info.name)))?;
    let at = start + count / 4;
    let scale = f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 字节"));
    Ok(Tensor {
        name: info.name.clone(),
        rows: info.rows,
        cols: info.cols,
        trits,
        scales: vec![scale],
        bias: vec![Ternary::Unknown; info.rows],
    })
}

/// 读一个本仓三值张量：5 trits/byte + per-row f32 scale + per-row bias。
fn read_trit(bytes: &[u8], start: usize, info: &Info) -> Result<Tensor, Error> {
    let trit_len = info.rows.saturating_mul(info.cols);
    let packed = trit_len.div_ceil(5);
    let bias_bytes = info.rows.div_ceil(5);
    let end = start
        .checked_add(packed)
        .and_then(|value| value.checked_add(info.rows.saturating_mul(4)))
        .and_then(|value| value.checked_add(bias_bytes))
        .ok_or_else(|| Error::Tensor(info.name.clone()))?;
    if end > bytes.len() {
        return Err(Error::Tensor(info.name.clone()));
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
    Ok(Tensor {
        name: info.name.clone(),
        rows: info.rows,
        cols: info.cols,
        trits,
        scales,
        bias,
    })
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
    if tensor_count > MAX_TENSORS || tensor_count > bytes.len().saturating_sub(pos) / 40 {
        return Err(Error::TooMany {
            count: tensor_count,
        });
    }
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
        if kind != TYPE_TRIT && kind != TYPE_I2_S {
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
            kind,
            offset,
        });
    }
    let data_start = align(pos);
    let mut tensors = Vec::with_capacity(infos.len());
    for info in infos {
        let start = data_start
            .checked_add(info.offset)
            .ok_or_else(|| Error::Tensor(info.name.clone()))?;
        let tensor = if info.kind == TYPE_I2_S {
            read_i2s(bytes, start, &info)?
        } else {
            read_trit(bytes, start, &info)?
        };
        tensors.push(tensor);
    }
    Ok(Model { metadata, tensors })
}
