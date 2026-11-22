//! 映射错误面（拆出单文件以守住模块行数预算）。

/// 映射错误。
#[derive(Debug, thiserror::Error)]
pub enum MmapError {
    /// 打开/查询文件失败。
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
    /// 文件长度超出 `usize`（32 位平台上的大文件）。
    #[error("文件过大（{len} 字节，超出 usize）")]
    TooLarge {
        /// 文件长度。
        len: u64,
    },
    /// 系统映射调用失败。
    #[error("映射失败（系统错误码 {code}）")]
    Map {
        /// 系统错误码。
        code: i32,
    },
    /// 映射建立后底层文件被截断；继续读映射会 SIGBUS（#589）。
    #[error("映射长度 {mapped} 字节，文件当前只有 {current} 字节（映射存活期禁止截断）")]
    Truncated {
        /// 建立映射时的长度。
        mapped: usize,
        /// 当前文件长度。
        current: u64,
    },
}
