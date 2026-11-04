//! 统一错误类型：所有形状/越界/解析错误都在这里，不 panic。

use std::fmt;

/// 库内统一错误。
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// 网络至少要有输入层与输出层。
    EmptySizes,
    /// 验证集不能为空。
    EmptyOracle,
    /// 维度上限（域穷举是 2^n / 3^n，必须挡住爆内存）。
    DomainTooLarge {
        /// 请求的变量数。
        vars: usize,
        /// 该构造允许的最大变量数。
        limit: usize,
    },
    /// 单层行列不能为零。
    EmptyLayer {
        /// 行数。
        rows: usize,
        /// 列数。
        cols: usize,
    },
    /// 长度与声明形状不符。
    Shape {
        /// 出错的对象（`trits` / `bias` / `scales` / `input` / `output`）。
        what: &'static str,
        /// 期望长度。
        expected: usize,
        /// 实际长度。
        got: usize,
    },
    /// 权重只允许 `-1/0/+1`。
    InvalidTrit(i8),
    /// scale 必须是有限正数（负 scale 会把整行语义取反，零 scale 会抹掉该行）。
    InvalidScale {
        /// 行号。
        row: usize,
        /// 非法的 scale 值。
        value: f32,
    },
    /// 编辑目标越界。
    OutOfBounds {
        /// 层号。
        layer: usize,
        /// 行号。
        row: usize,
        /// 列号。
        col: usize,
    },
    /// 不是 `0..rows` 的排列。
    NotPermutation {
        /// 层号。
        layer: usize,
    },
    /// 输出层没有置换对称性，不能按行置换。
    OutputLayerPermutation {
        /// 层号。
        layer: usize,
    },
    /// 两个网络的架构不同，不能做 diff / 合并 / 对齐。
    ArchMismatch,
    /// WSP1 文本解析错误。
    Parse {
        /// 行号（从 1 开始）。
        line: usize,
        /// 原因。
        msg: String,
    },
    /// 缺少必填行。
    MissingLine(&'static str),
    /// 校验和与内容不符。
    Checksum {
        /// 文本里写的值。
        expected: u64,
        /// 按内容重算的值。
        got: u64,
    },
    /// 线格式（补丁/验证集/报告 JSON）错误。
    Wire(String),
}

/// 库内统一 `Result`。
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySizes => write!(f, "网络至少需要 2 层（输入维 + 输出维）"),
            Self::EmptyOracle => write!(f, "验证集为空"),
            Self::DomainTooLarge { vars, limit } => {
                write!(f, "穷举域 {vars} 个变量超过上限 {limit}")
            }
            Self::EmptyLayer { rows, cols } => write!(f, "层形状 {rows}x{cols} 含零"),
            Self::Shape {
                what,
                expected,
                got,
            } => write!(f, "{what} 长度 {got}，期望 {expected}"),
            Self::InvalidTrit(v) => write!(f, "非法三值 {v}（只允许 -1/0/+1）"),
            Self::InvalidScale { row, value } => {
                write!(f, "第 {row} 行 scale={value} 不是有限正数")
            }
            Self::OutOfBounds { layer, row, col } => {
                write!(f, "编辑目标越界：layer={layer} row={row} col={col}")
            }
            Self::NotPermutation { layer } => write!(f, "第 {layer} 层的置换表不是 0..rows 的排列"),
            Self::OutputLayerPermutation { layer } => {
                write!(f, "第 {layer} 层是输出层，没有行置换对称性")
            }
            Self::ArchMismatch => write!(f, "两个网络的架构不同"),
            Self::Parse { line, msg } => write!(f, "WSP1 第 {line} 行：{msg}"),
            Self::MissingLine(what) => write!(f, "WSP1 缺少 {what} 行"),
            Self::Checksum { expected, got } => {
                write!(f, "校验和不符：文本 {expected:016x}，重算 {got:016x}")
            }
            Self::Wire(msg) => write!(f, "线格式错误：{msg}"),
        }
    }
}

impl std::error::Error for Error {}
