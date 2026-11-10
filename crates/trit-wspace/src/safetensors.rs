//! safetensors 外部权重读写：把真实模型的 2D 权重张量读成三值 [`Layer`]，或写回 f32。
//!
//! 只处理 2D 张量（`rows × cols`）与 `F32` / `F16` / `BF16` 三种 dtype；这是“权重空间
//! 模型”接真实权重的第一条入口，整模型分片/压缩仍另做。文件格式是 safetensors 的
//! 公开布局：8 字节小端 header 长度 + JSON header + 数据区，`data_offsets` 相对数据区
//! 起点。读取只 seek 到目标张量，不把整个权重文件读进内存。

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::net::Layer;

/// safetensors 支持的元素类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dtype {
    /// 32 位浮点。
    F32,
    /// 16 位半精度。
    F16,
    /// 16 位 bfloat16。
    Bf16,
}

impl Dtype {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "F32" => Some(Self::F32),
            "F16" => Some(Self::F16),
            "BF16" => Some(Self::Bf16),
            _ => None,
        }
    }

    /// 规范名（写回 header 时也用这个）。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::F32 => "F32",
            Self::F16 => "F16",
            Self::Bf16 => "BF16",
        }
    }

    /// 每个元素的字节数。
    #[must_use]
    pub fn size(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F16 | Self::Bf16 => 2,
        }
    }
}

/// 一个 2D 张量的元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TensorInfo {
    /// 张量名。
    pub name: String,
    /// 元素类型。
    pub dtype: Dtype,
    /// 形状（2D，`[rows, cols]`）。
    pub shape: Vec<usize>,
    /// 数据区内的字节范围 `[start, end)`。
    pub offsets: (usize, usize),
}

/// 读出的 2D 权重。
#[derive(Clone, Debug, PartialEq)]
pub struct Weights {
    /// 行数。
    pub rows: usize,
    /// 列数。
    pub cols: usize,
    /// 行主序权重。
    pub values: Vec<f32>,
}

impl Weights {
    /// 按 absmean 三值化成一层；偏置全零。
    pub fn to_layer(&self) -> Result<Layer> {
        Layer::from_f32(self.rows, self.cols, &self.values, &vec![0.0; self.rows])
    }
}

struct Entry {
    info: TensorInfo,
}

fn fail(what: &str) -> Error {
    Error::External(what.to_string())
}

fn parse_header(file: &mut File, total: u64) -> Result<(u64, BTreeMap<String, Entry>)> {
    let mut len_bytes = [0u8; 8];
    file.read_exact(&mut len_bytes)
        .map_err(|error| fail(&format!("读取 header 长度失败：{error}")))?;
    let header_len = u64::from_le_bytes(len_bytes);
    if header_len == 0 || header_len > 64 * 1024 * 1024 {
        return Err(fail(&format!("header 长度 {header_len} 不在 1..64 MiB 内")));
    }
    if header_len > total.saturating_sub(8) {
        return Err(fail(&format!(
            "header 长度 {header_len} 超过文件剩余 {} 字节",
            total.saturating_sub(8)
        )));
    }
    let mut raw = vec![0u8; header_len as usize];
    file.read_exact(&mut raw)
        .map_err(|error| fail(&format!("读取 header 失败：{error}")))?;
    let text =
        std::str::from_utf8(&raw).map_err(|error| fail(&format!("header 不是 UTF-8：{error}")))?;
    let json: Value =
        serde_json::from_str(text).map_err(|error| fail(&format!("header JSON 非法：{error}")))?;
    let object = json
        .as_object()
        .ok_or_else(|| fail("header 顶层必须是对象"))?;
    let data_start = 8 + header_len;
    let data_len = usize::try_from(total - data_start).map_err(|_| fail("数据区长度超出 usize"))?;
    let mut entries = BTreeMap::new();
    for (name, value) in object {
        if name == "__metadata__" {
            continue;
        }
        let dtype = value
            .get("dtype")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(&format!("张量 {name} 缺 dtype")))?;
        let dtype = Dtype::parse(dtype)
            .ok_or_else(|| fail(&format!("张量 {name} 的 dtype `{dtype}` 不受支持")))?;
        let shape: Vec<usize> = value
            .get("shape")
            .and_then(Value::as_array)
            .ok_or_else(|| fail(&format!("张量 {name} 缺 shape")))?
            .iter()
            .map(|dim| {
                dim.as_u64()
                    .map(|dim| dim as usize)
                    .ok_or_else(|| fail(&format!("张量 {name} 的 shape 含非整数")))
            })
            .collect::<Result<Vec<_>>>()?;
        if shape.len() != 2 || shape[0] == 0 || shape[1] == 0 {
            return Err(fail(&format!(
                "张量 {name} 的形状 {shape:?} 不是非空 2D（只支持 [rows, cols]）"
            )));
        }
        let offsets = value
            .get("data_offsets")
            .and_then(Value::as_array)
            .ok_or_else(|| fail(&format!("张量 {name} 缺 data_offsets")))?;
        if offsets.len() != 2 {
            return Err(fail(&format!("张量 {name} 的 data_offsets 必须是两个整数")));
        }
        let start = offsets[0]
            .as_u64()
            .map(|value| value as usize)
            .ok_or_else(|| fail(&format!("张量 {name} 的 data_offsets[0] 非法")))?;
        let end = offsets[1]
            .as_u64()
            .map(|value| value as usize)
            .ok_or_else(|| fail(&format!("张量 {name} 的 data_offsets[1] 非法")))?;
        if start > end || end > data_len {
            return Err(fail(&format!(
                "张量 {name} 的 data_offsets [{start}, {end}) 越界（数据区 {data_len} 字节）"
            )));
        }
        let elements = shape[0]
            .checked_mul(shape[1])
            .ok_or_else(|| fail(&format!("张量 {name} 的元素数溢出")))?;
        let want = elements
            .checked_mul(dtype.size())
            .ok_or_else(|| fail(&format!("张量 {name} 的字节数溢出")))?;
        if end - start != want {
            return Err(fail(&format!(
                "张量 {name} 的 data_offsets 跨 {} 字节，形状 {shape:?} × {} 需要 {want}",
                end - start,
                dtype.size()
            )));
        }
        entries.insert(
            name.clone(),
            Entry {
                info: TensorInfo {
                    name: name.clone(),
                    dtype,
                    shape,
                    offsets: (start, end),
                },
            },
        );
    }
    Ok((data_start, entries))
}

fn open(path: &str) -> Result<(File, u64, BTreeMap<String, Entry>)> {
    let mut file = File::open(path).map_err(|error| fail(&format!("{path}: {error}")))?;
    let total = file
        .metadata()
        .map_err(|error| fail(&format!("{path}: {error}")))?
        .len();
    let (data_start, entries) = parse_header(&mut file, total)?;
    Ok((file, data_start, entries))
}

/// 列出文件里的全部 2D 张量（`__metadata__` 除外）。
pub fn list(path: &str) -> Result<Vec<TensorInfo>> {
    let (_, _, entries) = open(path)?;
    Ok(entries.into_values().map(|entry| entry.info).collect())
}

/// 读取一个 2D 张量。
pub fn read_tensor(path: &str, name: &str) -> Result<Weights> {
    let (mut file, data_start, entries) = open(path)?;
    let entry = entries
        .get(name)
        .ok_or_else(|| fail(&format!("{path} 里没有张量 `{name}`")))?;
    let info = &entry.info;
    let (start, end) = info.offsets;
    file.seek(SeekFrom::Start(data_start + start as u64))
        .map_err(|error| fail(&format!("seek 失败：{error}")))?;
    let mut raw = vec![0u8; end - start];
    file.read_exact(&mut raw)
        .map_err(|error| fail(&format!("读取张量 `{name}` 失败：{error}")))?;
    let values = decode(info.dtype, &raw);
    if let Some((index, value)) = values.iter().enumerate().find(|(_, v)| !v.is_finite()) {
        return Err(fail(&format!(
            "张量 `{name}` 第 {index} 个元素是 {value}（非有限值不能三值化）"
        )));
    }
    Ok(Weights {
        rows: info.shape[0],
        cols: info.shape[1],
        values,
    })
}

fn decode(dtype: Dtype, raw: &[u8]) -> Vec<f32> {
    match dtype {
        Dtype::F32 => raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect(),
        Dtype::F16 => raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| f16_to_f32(u16::from_le_bytes(*chunk)))
            .collect(),
        Dtype::Bf16 => raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| f32::from_bits(u32::from(u16::from_le_bytes(*chunk)) << 16))
            .collect(),
    }
}

/// IEEE 754 半精度 → f32（含次正规数与 Inf/NaN）。
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = (bits >> 10) & 0x1f;
    let fraction = bits & 0x03ff;
    let magnitude = match exponent {
        0 => f32::from(fraction) * 2f32.powi(-24),
        0x1f => {
            if fraction == 0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + f32::from(fraction) / 1024.0) * 2f32.powi(i32::from(exponent) - 15),
    };
    sign * magnitude
}

/// 一个待写出的 2D F32 张量。
pub struct TensorOut<'a> {
    /// 张量名。
    pub name: &'a str,
    /// 行数。
    pub rows: usize,
    /// 列数。
    pub cols: usize,
    /// 行主序权重。
    pub values: &'a [f32],
}

/// 写出一个或多个 2D F32 张量（单个 safetensors 文件）。
pub fn write_tensors(path: &str, tensors: &[TensorOut<'_>]) -> Result<()> {
    if tensors.is_empty() {
        return Err(fail("至少需要一个张量"));
    }
    let mut header = serde_json::Map::new();
    let mut data: Vec<u8> = Vec::new();
    for tensor in tensors {
        if tensor.rows == 0 || tensor.cols == 0 {
            return Err(Error::EmptyLayer {
                rows: tensor.rows,
                cols: tensor.cols,
            });
        }
        if tensor.values.len() != tensor.rows * tensor.cols {
            return Err(Error::Shape {
                what: "safetensors values",
                expected: tensor.rows * tensor.cols,
                got: tensor.values.len(),
            });
        }
        if let Some((index, value)) = tensor
            .values
            .iter()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
        {
            return Err(fail(&format!(
                "张量 `{}` 第 {index} 个权重是 {value}（非有限值）",
                tensor.name
            )));
        }
        let start = data.len();
        for value in tensor.values {
            data.extend_from_slice(&value.to_le_bytes());
        }
        header.insert(
            tensor.name.to_string(),
            json!({
                "dtype": Dtype::F32.name(),
                "shape": [tensor.rows, tensor.cols],
                "data_offsets": [start, data.len()],
            }),
        );
    }
    let mut header = serde_json::to_vec(&Value::Object(header))
        .map_err(|error| fail(&format!("header 序列化失败：{error}")))?;
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut out = Vec::with_capacity(8 + header.len() + data.len());
    out.extend_from_slice(&(header.len() as u64).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&data);
    let temp = format!("{path}.tmp-{}", std::process::id());
    fs::write(&temp, &out).map_err(|error| fail(&format!("{temp}: {error}")))?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(path);
            fs::rename(&temp, path).map_err(|error| fail(&format!("{path}: {error}")))
        }
    }
}

/// 写一个 2D F32 张量（单张量文件）。
pub fn write_tensor(
    path: &str,
    name: &str,
    rows: usize,
    cols: usize,
    values: &[f32],
) -> Result<()> {
    write_tensors(
        path,
        &[TensorOut {
            name,
            rows,
            cols,
            values,
        }],
    )
}
