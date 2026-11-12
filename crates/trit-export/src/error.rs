//! 读写错误（从  拆出，控制单文件行数）。

/// 读写错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 文件短于声明的结构。
    #[error("GGUF 太短")]
    Truncated,
    /// magic 不是 `GGUF`。
    #[error("GGUF magic 不符")]
    Magic,
    /// 版本不是 3。
    #[error("GGUF 版本 {0} 不受支持")]
    Version(u32),
    /// 元数据类型码不受支持。
    #[error("GGUF 元数据类型 {0} 不受支持")]
    MetaType(u32),
    /// 张量类型码不是 `TYPE_TRIT`。
    #[error("GGUF 张量类型 {0} 不受支持")]
    TensorType(u32),
    /// 张量不是 2D。
    #[error("GGUF 张量维度 {0}（只支持 2D）")]
    Dims(u32),
    /// 行/列数为 0。
    #[error("GGUF 形状 {0}×{1} 非法")]
    Shape(usize, usize),
    /// 张量数据越界。
    #[error("GGUF 张量 {0} 数据越界/截断")]
    Tensor(String),
    /// 字符串不是 UTF-8。
    #[error("GGUF 字符串不是 UTF-8")]
    Utf8,
    /// 张量名重复。
    #[error("GGUF 张量 {0} 重复")]
    Duplicate(String),
    /// 张量字段长度不符。
    #[error("GGUF 张量 {0} 字段长度不符")]
    Field(String),
    /// 打包长度与 trit 数不符。
    #[error("GGUF 打包长度不符：期望 {expected}，实际 {got}")]
    PackedLen { expected: usize, got: usize },
    /// 打包字节 >242（3^5 不可达值）。
    #[error("GGUF 打包字节 {0} 非法（>242）")]
    InvalidByte(u8),
    /// 尾部 padding 不是规范 False。
    #[error("GGUF 打包 padding 非规范")]
    Padding,
}
