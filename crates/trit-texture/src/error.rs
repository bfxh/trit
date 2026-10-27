//! 错误类型：TTX1/TTX2/TTX3 三类容器与编码共用一个 [`TextureError`]。

use crate::{BLOCK_HEIGHT, BLOCK_WIDTH, MAX_TEXTURE_PIXELS};

/// 纹理压缩错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextureError {
    /// 宽高必须分别是 4 / 8 的倍数。
    #[error("宽 {width} 必须是 {BLOCK_WIDTH} 的倍数，高 {height} 必须是 {BLOCK_HEIGHT} 的倍数")]
    Shape {
        /// 传入的宽。
        width: usize,
        /// 传入的高。
        height: usize,
    },
    /// 输入长度与宽高不符（RGBA，每纹素 4 字节）。
    #[error("输入 {actual} 字节，宽高要求 {expected} 字节（RGBA）")]
    SizeMismatch {
        /// 实际字节数。
        actual: usize,
        /// 期望字节数。
        expected: usize,
    },
    /// 纹理过大。
    #[error("纹理 {pixels} 纹素，超过上限 {MAX_TEXTURE_PIXELS}")]
    TooLarge {
        /// 纹素数。
        pixels: usize,
    },
    /// 调色板索引越界（baseline 调色板只有 3 色，合法索引 0..=2）。
    #[error("块 {block} 的调色板索引 {index} 越界（只支持 0..=2）")]
    InvalidPaletteIndex {
        /// 块序号。
        block: usize,
        /// 非法索引。
        index: u8,
    },
    /// 解码数据的块数与宽高不符。
    #[error("块数 {actual} 与宽高要求的 {expected} 不符")]
    BlockCount {
        /// 实际块数。
        actual: usize,
        /// 期望块数。
        expected: usize,
    },
    /// TTX 魔数不符（既非 "TTX1" 也非 "TTX2"；v2 读 v1 也走这条，带着实际字节）。
    #[error("TTX 魔数不符：{0:?}")]
    TtxMagic([u8; 4]),
    /// TTX 容器截断（不足头部/载荷/校验尾）。
    #[error("TTX 容器只有 {actual} 字节，长度不符")]
    TtxTruncated {
        /// 实际字节数。
        actual: usize,
    },
    /// TTX 头部声明的块数与宽高推出的块数不符。
    #[error("TTX 声明 {declared} 块，宽高要求 {expected} 块")]
    TtxBlockCount {
        /// 声明的块数。
        declared: u32,
        /// 期望块数。
        expected: usize,
    },
    /// TTX 的 GF(3) 校验和不符。
    #[error("TTX 校验和不符：存 {stored:#04x}，算得 {expected:#04x}")]
    TtxChecksum {
        /// 存的校验字节。
        stored: u8,
        /// 重算的校验字节。
        expected: u8,
    },
    /// TTX2 的 kind 不是 0（TEX）/ 1（MOD）。
    #[error("TTX2 未知 kind：{0}")]
    TtxKind(u8),
    /// TTX2 的 flags/reserved 非零（前向兼容显式拒绝，不静默猜）。
    #[error("TTX2 flags/reserved 必须为零，实际 flags={flags} reserved={reserved}")]
    TtxFlags {
        /// flags 字节。
        flags: u8,
        /// reserved 双字节。
        reserved: u16,
    },
    /// TTX2 段没有任何 mip 级 / 层。
    #[error("TTX2 段没有任何 mip 级/层")]
    TtxEmpty,
    /// TTX2 的 mip 级未严格小于上一级。
    #[error("TTX2 第 {level} 级 mip 未严格小于上一级（宽高须不增且至少一维递减）")]
    TtxMip {
        /// 违规的 mip 级下标。
        level: usize,
    },
    /// TTX2 模型层形状不合法。
    #[error("TTX2 第 {layer} 层形状不合法：{message}")]
    TtxModelShape {
        /// 层下标。
        layer: usize,
        /// 说明。
        message: String,
    },
    /// TTX3 魔数不符。
    #[error("TTX3 魔数不符")]
    Ttx3Magic,
    /// TTX3 的 flags 非零（未知位显式拒绝，不静默猜）。
    #[error("TTX3 flags 必须为零，实际 {flags}")]
    Ttx3Flags {
        /// flags 双字节。
        flags: u16,
    },
    /// TTX3 尺寸不合法（瓦片倍数 / 边长 / 纹素上限）。
    #[error("TTX3 尺寸 {width}×{height} 不合法：{reason}")]
    Ttx3Dims {
        /// 传入的宽。
        width: usize,
        /// 传入的高。
        height: usize,
        /// 说明。
        reason: String,
    },
    /// TTX3 的级不合法（不存在 / 尺寸未递减 / 政策未知 / 级数越界）。
    #[error("TTX3 第 {level} 级不合法：{message}")]
    Ttx3Level {
        /// 级下标（或政策字节）。
        level: usize,
        /// 说明。
        message: String,
    },
    /// TTX3 的瓦片不合法或损坏（坐标越界 / 冗余耗尽）。
    #[error("TTX3 第 {level} 级瓦片 ({tx},{ty}) 不合法：{message}")]
    Ttx3Tile {
        /// 级下标。
        level: usize,
        /// 瓦片 x。
        tx: usize,
        /// 瓦片 y。
        ty: usize,
        /// 说明。
        message: String,
    },
    /// TTX3 字节流被截断。
    #[error("TTX3 需要 {needed} 字节，实际 {got}")]
    Ttx3Truncated {
        /// 需要的字节数。
        needed: u64,
        /// 实际字节数。
        got: u64,
    },
}
