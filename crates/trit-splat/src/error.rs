//! 渲染层统一错误类型（`crate::SplatError` 的 re-export 定义处）。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SplatError {
    #[error("PLY 头缺 end_header")]
    MissingEndHeader,
    #[error("PLY 不支持格式: {0}")]
    UnsupportedFormat(String),
    #[error("PLY 缺必需属性: {0}")]
    MissingProperty(String),
    #[error("PLY 数据段不足: 需要 {need} 字节, 实际 {got}")]
    TruncatedData { need: usize, got: usize },
    #[error("transforms.json 解析失败: {0}")]
    Transforms(String),
    #[error("渲染参数非法: {0}")]
    InvalidArgs(String),
    #[error("SPZ 解析失败: {0}")]
    Spz(String),
}
