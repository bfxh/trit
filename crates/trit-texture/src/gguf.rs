//! GGUF → MOD（TTX2 模型容器）入向导入。
//!
//! 范围（如实限制）：
//! - 只读 **GGUF v2/v3**（小端：定头 + 元数据 KV + tensor 信息 + 对齐后的数据段）。
//! - 只解**未量化**张量：`F32` / `F16` / `BF16`。三值/分组量化类型（`TQ1_0`、`Q*_K` 等）
//!   的位布局没有在本仓验证过 ⇒ **显式拒绝**，不猜。猜错的解量化会静默产出错误权重，
//!   比"不支持"糟得多。
//! - 只产出「线性层链」：`*.weight`（2D）配同名 `*.bias`（1D，可选，缺省全 0）；
//!   层间维数衔接交给 `TritModel::new`（与 MOD 读侧同一套校验）。
//! - 三值化：`v > eps → +1`、`v < -eps → -1`、其余 `0`；默认 `eps = 0`（纯符号），
//!   要稀疏就把 |w| 小的压成 0。`NaN`/`Inf` 直接报错。
//!
//! 预算是硬门：文件字节、KV 数、张量数、维数乘积、权重总数都在分配前检查。

use crate::TextureError;
use crate::ttx2::{TritModel, TritModelLayer};

/// GGUF 文件魔数（小端读作 u32）。
pub const GGUF_MAGIC: u32 = 0x4655_4747;
/// 支持的版本。
pub const SUPPORTED_VERSIONS: [u32; 2] = [2, 3];

const TYPE_F32: u32 = 0;
const TYPE_F16: u32 = 1;
const TYPE_BF16: u32 = 30;

const DEFAULT_MAX_BYTES: usize = 256 << 20;
const MAX_KV: u64 = 100_000;
const MAX_TENSORS: u64 = 100_000;
const MAX_DIMS: u32 = 4;
const DEFAULT_MAX_WEIGHTS: u64 = 50_000_000;
const MAX_STRING: u64 = 1 << 16;

/// 导入选项。
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// 权重绝对值不超过它时落成 0（默认 0 = 纯符号）。
    pub zero_threshold: f32,
    /// 只导入这些张量（按给定顺序）；缺省按名字排序选全部 2D `*.weight`。
    pub tensor_names: Option<Vec<String>>,
    /// 文件字节上限。
    pub max_bytes: usize,
    /// 权重总数上限。
    pub max_weights: u64,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            zero_threshold: 0.0,
            tensor_names: None,
            max_bytes: DEFAULT_MAX_BYTES,
            max_weights: DEFAULT_MAX_WEIGHTS,
        }
    }
}

/// GGUF 导入错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GgufError {
    /// 超过预算。
    TooLarge {
        /// 实际（或权重总数）。
        bytes: usize,
        /// 上限。
        limit: usize,
    },
    /// 魔数不对。
    Magic(u32),
    /// 版本不支持。
    Version(u32),
    /// 数据截断。
    Truncated,
    /// 元数据/结构问题。
    Metadata(String),
    /// 张量类型未验证。
    UnsupportedType {
        /// 张量名。
        tensor: String,
        /// GGML 类型 id。
        type_id: u32,
    },
    /// 张量形状非法。
    Shape {
        /// 张量名。
        tensor: String,
        /// 说明。
        message: String,
    },
    /// 没有可导入的权重。
    NoWeights,
    /// 指定的张量不存在。
    Missing(String),
    /// 层链不合法。
    Chain(String),
    /// 数值非法。
    Value {
        /// 张量名。
        tensor: String,
        /// 说明。
        message: String,
    },
}

impl std::fmt::Display for GgufError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(formatter, "GGUF 超过预算：{bytes} > {limit}")
            }
            Self::Magic(value) => write!(formatter, "不是 GGUF：魔数 {value:#010x}"),
            Self::Version(version) => {
                write!(formatter, "GGUF 版本 {version} 不受支持（只认 v2/v3）")
            }
            Self::Truncated => write!(formatter, "GGUF 数据截断"),
            Self::Metadata(message) => write!(formatter, "GGUF 元数据/结构非法：{message}"),
            Self::UnsupportedType { tensor, type_id } => write!(
                formatter,
                "张量 {tensor} 的量化类型 {type_id} 未验证；本导入器只支持 F32/F16/BF16"
            ),
            Self::Shape { tensor, message } => {
                write!(formatter, "张量 {tensor} 形状非法：{message}")
            }
            Self::NoWeights => write!(formatter, "没有找到 2D 的 *.weight 张量"),
            Self::Missing(name) => write!(formatter, "张量 {name} 不存在"),
            Self::Chain(message) => write!(formatter, "层链不合法：{message}"),
            Self::Value { tensor, message } => {
                write!(formatter, "张量 {tensor} 的数值非法：{message}")
            }
        }
    }
}

impl std::error::Error for GgufError {}

impl From<GgufError> for TextureError {
    fn from(error: GgufError) -> Self {
        TextureError::TtxModelShape {
            layer: 0,
            message: error.to_string(),
        }
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], GgufError> {
        let end = self.offset.checked_add(count).ok_or(GgufError::Truncated)?;
        if end > self.bytes.len() {
            return Err(GgufError::Truncated);
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, GgufError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, GgufError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 字节"),
        ))
    }

    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 字节"),
        ))
    }

    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 字节"),
        ))
    }

    fn string(&mut self, what: &str) -> Result<String, GgufError> {
        let len = self.u64()?;
        if len > MAX_STRING {
            return Err(GgufError::Metadata(format!(
                "{what} 字符串 {len} 字节，超过上限 {MAX_STRING}"
            )));
        }
        let bytes = self.take(len as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| GgufError::Metadata(format!("{what} 不是合法 UTF-8")))
    }
}

#[derive(Debug, Clone)]
struct TensorInfo {
    name: String,
    dims: Vec<u64>,
    type_id: u32,
    offset: u64,
}

impl TensorInfo {
    fn elements(&self) -> Result<u64, GgufError> {
        self.dims.iter().try_fold(1u64, |acc, dim| {
            acc.checked_mul(*dim).ok_or_else(|| GgufError::Shape {
                tensor: self.name.clone(),
                message: "元素数溢出 u64".to_string(),
            })
        })
    }

    fn element_size(&self) -> Option<usize> {
        match self.type_id {
            TYPE_F32 => Some(4),
            TYPE_F16 | TYPE_BF16 => Some(2),
            _ => None,
        }
    }
}

fn skip_value(cursor: &mut Cursor<'_>, type_id: u32, depth: u32) -> Result<(), GgufError> {
    if depth > 4 {
        return Err(GgufError::Metadata("数组嵌套过深".to_string()));
    }
    match type_id {
        0 | 1 | 7 => {
            cursor.u8()?;
        }
        2 | 3 => {
            cursor.u16()?;
        }
        4..=6 => {
            cursor.u32()?;
        }
        8 => {
            cursor.string("字符串值")?;
        }
        9 => {
            let element = cursor.u32()?;
            let count = cursor.u64()?;
            if count > MAX_KV {
                return Err(GgufError::Metadata(format!(
                    "数组元素 {count} 超过上限 {MAX_KV}"
                )));
            }
            for _ in 0..count {
                skip_value(cursor, element, depth + 1)?;
            }
        }
        10..=12 => {
            cursor.u64()?;
        }
        other => return Err(GgufError::Metadata(format!("未知元数据类型 {other}"))),
    }
    Ok(())
}

fn read_metadata(cursor: &mut Cursor<'_>, kv_count: u64) -> Result<u32, GgufError> {
    let mut alignment = 32u32;
    for _ in 0..kv_count {
        let key = cursor.string("键")?;
        let type_id = cursor.u32()?;
        if key == "general.alignment" {
            if type_id != 4 {
                return Err(GgufError::Metadata(
                    "general.alignment 必须是 UINT32".to_string(),
                ));
            }
            let value = cursor.u32()?;
            if value == 0 || !value.is_power_of_two() {
                return Err(GgufError::Metadata(format!(
                    "general.alignment={value} 不是 2 的幂"
                )));
            }
            alignment = value;
        } else {
            skip_value(cursor, type_id, 0)?;
        }
    }
    Ok(alignment)
}

fn read_tensors(cursor: &mut Cursor<'_>, count: u64) -> Result<Vec<TensorInfo>, GgufError> {
    if count > MAX_TENSORS {
        return Err(GgufError::Metadata(format!(
            "张量数 {count} 超过上限 {MAX_TENSORS}"
        )));
    }
    let mut tensors = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = cursor.string("张量名")?;
        let n_dims = cursor.u32()?;
        if n_dims == 0 || n_dims > MAX_DIMS {
            return Err(GgufError::Shape {
                tensor: name,
                message: format!("维数 {n_dims} 不在 1..={MAX_DIMS}"),
            });
        }
        let mut dims = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            dims.push(cursor.u64()?);
        }
        let type_id = cursor.u32()?;
        let offset = cursor.u64()?;
        tensors.push(TensorInfo {
            name,
            dims,
            type_id,
            offset,
        });
    }
    Ok(tensors)
}

/// 半精度 → f32 位模式（含次正规数、inf/nan）。
fn f16_to_f32_bits(bits: u16) -> u32 {
    let sign = (bits as u32 & 0x8000) << 16;
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = bits & 0x03ff;
    if exponent == 0 {
        if mantissa == 0 {
            return sign;
        }
        let mut mantissa = mantissa as u32;
        let mut shift = 0i32;
        while mantissa & 0x0400 == 0 {
            mantissa <<= 1;
            shift -= 1;
        }
        mantissa &= 0x03ff;
        let exponent = (127 - 15 + 1 + shift) as u32;
        return sign | (exponent << 23) | (mantissa << 13);
    }
    if exponent == 0x1f {
        return sign | 0x7f80_0000 | ((mantissa as u32) << 13);
    }
    let exponent = (exponent as u32 + 127 - 15) << 23;
    sign | exponent | ((mantissa as u32) << 13)
}

fn read_element(tensor: &TensorInfo, bytes: &[u8], index: usize) -> Result<f32, GgufError> {
    let value = match tensor.type_id {
        TYPE_F32 => {
            let at = index * 4;
            f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 字节"))
        }
        TYPE_F16 => {
            let at = index * 2;
            f32::from_bits(f16_to_f32_bits(u16::from_le_bytes(
                bytes[at..at + 2].try_into().expect("2 字节"),
            )))
        }
        TYPE_BF16 => {
            let at = index * 2;
            let bits = u16::from_le_bytes(bytes[at..at + 2].try_into().expect("2 字节"));
            f32::from_bits((bits as u32) << 16)
        }
        _ => {
            return Err(GgufError::UnsupportedType {
                tensor: tensor.name.clone(),
                type_id: tensor.type_id,
            });
        }
    };
    if !value.is_finite() {
        return Err(GgufError::Value {
            tensor: tensor.name.clone(),
            message: format!("第 {index} 个元素是 {value}"),
        });
    }
    Ok(value)
}

fn quantize(value: f32, threshold: f32) -> i8 {
    if value > threshold {
        1
    } else if value < -threshold {
        -1
    } else {
        0
    }
}

fn dequantize(
    tensor: &TensorInfo,
    data: &[u8],
    options: &ImportOptions,
) -> Result<Vec<i8>, GgufError> {
    let element_size = tensor
        .element_size()
        .ok_or_else(|| GgufError::UnsupportedType {
            tensor: tensor.name.clone(),
            type_id: tensor.type_id,
        })?;
    let elements = tensor.elements()?;
    let needed = (elements as usize)
        .checked_mul(element_size)
        .ok_or(GgufError::Truncated)?;
    if needed > data.len() {
        return Err(GgufError::Truncated);
    }
    let mut trits = Vec::with_capacity(elements as usize);
    for index in 0..elements as usize {
        trits.push(quantize(
            read_element(tensor, data, index)?,
            options.zero_threshold,
        ));
    }
    Ok(trits)
}

fn tensor_data<'a>(tensor: &TensorInfo, data: &'a [u8]) -> Result<&'a [u8], GgufError> {
    let start = usize::try_from(tensor.offset).map_err(|_| GgufError::Truncated)?;
    if start > data.len() {
        return Err(GgufError::Truncated);
    }
    Ok(&data[start..])
}

/// 从 GGUF 导入三值模型。
pub fn import_gguf(bytes: &[u8], options: &ImportOptions) -> Result<TritModel, GgufError> {
    if !options.zero_threshold.is_finite() || options.zero_threshold < 0.0 {
        return Err(GgufError::Metadata(format!(
            "zero_threshold 必须是非负有限值，实际 {}",
            options.zero_threshold
        )));
    }
    if bytes.len() > options.max_bytes {
        return Err(GgufError::TooLarge {
            bytes: bytes.len(),
            limit: options.max_bytes,
        });
    }
    let mut cursor = Cursor { bytes, offset: 0 };
    let magic = cursor.u32()?;
    if magic != GGUF_MAGIC {
        return Err(GgufError::Magic(magic));
    }
    let version = cursor.u32()?;
    if !SUPPORTED_VERSIONS.contains(&version) {
        return Err(GgufError::Version(version));
    }
    let tensor_count = cursor.u64()?;
    let kv_count = cursor.u64()?;
    if kv_count > MAX_KV {
        return Err(GgufError::Metadata(format!(
            "元数据 KV 数 {kv_count} 超过上限 {MAX_KV}"
        )));
    }
    let alignment = read_metadata(&mut cursor, kv_count)?;
    let tensors = read_tensors(&mut cursor, tensor_count)?;
    let info_end = cursor.offset;
    let aligned = info_end.div_ceil(alignment as usize) * alignment as usize;
    if aligned > bytes.len() {
        return Err(GgufError::Truncated);
    }
    let data = &bytes[aligned..];

    let by_name: std::collections::BTreeMap<&str, &TensorInfo> = tensors
        .iter()
        .map(|item| (item.name.as_str(), item))
        .collect();

    let selected: Vec<&TensorInfo> = match &options.tensor_names {
        Some(names) => names
            .iter()
            .map(|name| {
                by_name
                    .get(name.as_str())
                    .copied()
                    .ok_or_else(|| GgufError::Missing(name.clone()))
            })
            .collect::<Result<_, _>>()?,
        None => {
            let mut weights: Vec<&TensorInfo> = tensors
                .iter()
                .filter(|item| item.name.ends_with(".weight") && item.dims.len() == 2)
                .collect();
            weights.sort_by(|left, right| left.name.cmp(&right.name));
            if weights.is_empty() {
                return Err(GgufError::NoWeights);
            }
            weights
        }
    };

    let mut layers = Vec::with_capacity(selected.len());
    let mut total_weights = 0u64;
    for tensor in &selected {
        if tensor.dims.len() != 2 {
            return Err(GgufError::Shape {
                tensor: tensor.name.clone(),
                message: format!("权重必须是 2D，实际 {} 维", tensor.dims.len()),
            });
        }
        let inputs = tensor.dims[0];
        let outputs = tensor.dims[1];
        if inputs == 0 || outputs == 0 {
            return Err(GgufError::Shape {
                tensor: tensor.name.clone(),
                message: "输入/输出维数不能为 0".to_string(),
            });
        }
        let weights_count = outputs
            .checked_mul(inputs)
            .ok_or_else(|| GgufError::Shape {
                tensor: tensor.name.clone(),
                message: "维数乘积溢出 u64".to_string(),
            })?;
        total_weights =
            total_weights
                .checked_add(weights_count)
                .ok_or_else(|| GgufError::Shape {
                    tensor: tensor.name.clone(),
                    message: "权重总数溢出 u64".to_string(),
                })?;
        if total_weights > options.max_weights {
            return Err(GgufError::TooLarge {
                bytes: total_weights as usize,
                limit: options.max_weights as usize,
            });
        }

        let flat = dequantize(tensor, tensor_data(tensor, data)?, options)?;

        let bias_name = tensor.name.trim_end_matches(".weight").to_string() + ".bias";
        let bias = if let Some(bias_info) = by_name.get(bias_name.as_str()) {
            if bias_info.dims.len() != 1 || bias_info.dims[0] != outputs {
                return Err(GgufError::Shape {
                    tensor: bias_name,
                    message: format!("偏置必须是长度 {outputs} 的 1D，实际 {:?}", bias_info.dims),
                });
            }
            dequantize(bias_info, tensor_data(bias_info, data)?, options)?
        } else {
            vec![0i8; outputs as usize]
        };

        let rows: Vec<Vec<i8>> = flat
            .chunks(inputs as usize)
            .map(|row| row.to_vec())
            .collect();
        layers.push(
            TritModelLayer::new(&rows, &bias)
                .map_err(|error| GgufError::Chain(error.to_string()))?,
        );
    }

    TritModel::new(layers).map_err(|error| GgufError::Chain(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        name: String,
        dims: Vec<u64>,
        type_id: u32,
        raw: Vec<u8>,
    }

    fn write_string(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(&(text.len() as u64).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
    }

    fn f32_tensor(name: &str, dims: &[u64], values: &[f32]) -> Fixture {
        Fixture {
            name: name.to_string(),
            dims: dims.to_vec(),
            type_id: TYPE_F32,
            raw: values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        }
    }

    fn raw_tensor(name: &str, dims: &[u64], type_id: u32, raw: Vec<u8>) -> Fixture {
        Fixture {
            name: name.to_string(),
            dims: dims.to_vec(),
            type_id,
            raw,
        }
    }

    fn build(tensors: &[Fixture]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        let mut offset = 0u64;
        for tensor in tensors {
            write_string(&mut out, &tensor.name);
            out.extend_from_slice(&(tensor.dims.len() as u32).to_le_bytes());
            for dim in &tensor.dims {
                out.extend_from_slice(&dim.to_le_bytes());
            }
            out.extend_from_slice(&tensor.type_id.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            offset += tensor.raw.len() as u64;
        }
        while out.len() % 32 != 0 {
            out.push(0);
        }
        for tensor in tensors {
            out.extend_from_slice(&tensor.raw);
        }
        out
    }

    fn two_layer_fixture() -> Vec<u8> {
        build(&[
            f32_tensor(
                "blk.0.ffn.weight",
                &[3, 2],
                &[1.0, -1.0, 0.0, 0.0, 1.0, -1.0],
            ),
            f32_tensor("blk.0.ffn.bias", &[2], &[0.9, -0.9]),
            f32_tensor("blk.1.out.weight", &[2, 1], &[1.0, -1.0]),
        ])
    }

    #[test]
    fn imports_chained_layers_with_bias() {
        let model = import_gguf(&two_layer_fixture(), &ImportOptions::default()).unwrap();
        assert_eq!(model.layers.len(), 2);
        assert_eq!((model.layers[0].inputs, model.layers[0].outputs), (3, 2));
        assert_eq!(model.layers[0].weights, vec![1, -1, 0, 0, 1, -1]);
        assert_eq!(model.layers[0].bias, vec![1, -1]);
        assert_eq!((model.layers[1].inputs, model.layers[1].outputs), (2, 1));
        assert_eq!(model.layers[1].weights, vec![1, -1]);
        assert_eq!(model.layers[1].bias, vec![0], "缺偏置时补 0");
    }

    #[test]
    fn zero_threshold_creates_sparsity() {
        let bytes = build(&[f32_tensor("a.weight", &[3, 1], &[0.2, -0.3, 0.8])]);
        let options = ImportOptions {
            zero_threshold: 0.25,
            ..ImportOptions::default()
        };
        let model = import_gguf(&bytes, &options).unwrap();
        assert_eq!(model.layers[0].weights, vec![0, -1, 1]);
    }

    #[test]
    fn half_and_bfloat16_are_decoded() {
        let mut f16 = Vec::new();
        for bits in [0x3C00u16, 0xBC00, 0x3800] {
            f16.extend_from_slice(&bits.to_le_bytes());
        }
        let bytes = build(&[raw_tensor("a.weight", &[3, 1], TYPE_F16, f16)]);
        let model = import_gguf(&bytes, &ImportOptions::default()).unwrap();
        assert_eq!(model.layers[0].weights, vec![1, -1, 1]);

        let bf16: Vec<u8> = [0x3F80u16, 0xBF80]
            .iter()
            .flat_map(|bits| bits.to_le_bytes())
            .collect();
        let bytes = build(&[raw_tensor("a.weight", &[2, 1], TYPE_BF16, bf16)]);
        let model = import_gguf(&bytes, &ImportOptions::default()).unwrap();
        assert_eq!(model.layers[0].weights, vec![1, -1]);
    }

    #[test]
    fn rejects_bad_magic_version_and_quantized_types() {
        let mut bytes = two_layer_fixture();
        bytes[0] ^= 0xFF;
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::Magic(_))
        ));

        let mut bytes = two_layer_fixture();
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::Version(1))
        ));

        // Q2_K = 10：未验证的量化布局必须显式拒绝。
        let bytes = build(&[raw_tensor("a.weight", &[2, 1], 10, vec![0; 4])]);
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::UnsupportedType { type_id: 10, .. })
        ));
    }

    #[test]
    fn rejects_chain_mismatch_and_missing_tensor() {
        let bytes = build(&[
            f32_tensor("blk.0.w.weight", &[4, 2], &[1.0; 8]),
            f32_tensor("blk.1.w.weight", &[3, 1], &[1.0; 3]),
        ]);
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::Chain(_))
        ));

        let bytes = two_layer_fixture();
        let options = ImportOptions {
            tensor_names: Some(vec!["nope".to_string()]),
            ..ImportOptions::default()
        };
        assert!(matches!(
            import_gguf(&bytes, &options),
            Err(GgufError::Missing(name)) if name == "nope"
        ));
    }

    #[test]
    fn rejects_non_finite_and_truncated() {
        let bytes = build(&[f32_tensor("a.weight", &[2, 1], &[f32::NAN, 1.0])]);
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::Value { .. })
        ));

        let mut bytes = two_layer_fixture();
        bytes.truncate(24);
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::Truncated)
        ));

        // 数据段已对齐但少一个字节：必须由"元素长度"检查报错，不能靠切片 panic。
        let mut bytes = two_layer_fixture();
        let length = bytes.len();
        bytes.truncate(length - 1);
        assert!(matches!(
            import_gguf(&bytes, &ImportOptions::default()),
            Err(GgufError::Truncated)
        ));

        let bytes = two_layer_fixture();
        let options = ImportOptions {
            max_bytes: 8,
            ..ImportOptions::default()
        };
        assert!(matches!(
            import_gguf(&bytes, &options),
            Err(GgufError::TooLarge { .. })
        ));
    }

    #[test]
    fn imported_model_round_trips_through_ttx2() {
        let model = import_gguf(&two_layer_fixture(), &ImportOptions::default()).unwrap();
        let container = crate::ttx2::to_ttx2(&crate::ttx2::TtxDocument::Model(model.clone()))
            .expect("MOD 写出");
        match crate::ttx2::from_ttx2(&container).expect("MOD 读回") {
            crate::ttx2::TtxDocument::Model(back) => assert_eq!(back, model),
            other => panic!("kind 不对：{other:?}"),
        }
    }

    #[test]
    fn unsupported_nan_threshold_is_rejected() {
        let bytes = two_layer_fixture();
        let options = ImportOptions {
            zero_threshold: f32::NAN,
            ..ImportOptions::default()
        };
        assert!(matches!(
            import_gguf(&bytes, &options),
            Err(GgufError::Metadata(_))
        ));
    }
}
