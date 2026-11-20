//! 三值权重 GGUF 容器（GGUF v3 最小子集）。
//! 只实现读写三值张量所需的最小子集：文件头、少量元数据 KV、2D 张量信息、32 字节对齐数据区。
//! 两种张量类型：本仓 `TYPE_TRIT = 100`（5 trits/byte + per-row `scales` + `bias`）与上游
//! bitnet.cpp 的 `TYPE_I2_S = 36`（2 bit/trit + per-tensor f32 scale + 32 字节尾）。

use trit_core::Ternary;

/// 本仓自定义三值张量类型（上游 GGML type id 0..=30，100 起未占用）。
pub const TYPE_TRIT: u32 = 100;
/// 上游 bitnet.cpp 的 I2_S 张量类型（实测 `microsoft/bitnet-b1.58-2B-4T-gguf` 的 I2_S 张量为 36）。
pub const TYPE_I2_S: u32 = 36;

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

/// 三值 2D 张量：行主序 trits + scale + 每行三值 bias。
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
    /// scale：`rows` 个表示 per-row（`TYPE_TRIT`），1 个表示 per-tensor（`TYPE_I2_S`，写回时复制）。
    pub scales: Vec<f32>,
    /// 每行一个三值 bias（`rows` 个；`TYPE_I2_S` 无此字段，读出来全是 `Unknown`）。
    pub bias: Vec<Ternary>,
}

/// 一个 GGUF 模型：元数据 + 三值张量。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    /// 元数据（按 key 排序，保证输出确定性）。
    pub metadata: std::collections::BTreeMap<String, MetaValue>,
    /// 张量列表（按写入顺序）。
    pub tensors: Vec<Tensor>,
}

pub mod bitnet;
mod error;
mod gguf_codec;
pub use error::Error;
pub use gguf_codec::{read, write};

/// 单张量数据区大小：trits + scales + bias（仅 `TYPE_TRIT`）。
#[must_use]
pub fn tensor_size(rows: usize, cols: usize) -> usize {
    rows.saturating_mul(cols).div_ceil(5) + rows.saturating_mul(4) + rows.div_ceil(5)
}
