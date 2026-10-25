//! 三值编解码实验层（`publish = false`，不属于稳定 API）。
//!
//! 多个方向的「编码器 / 解码器」：
//!
//! - [`plan`]：按数据分布选最省编码（无 `X` → 1 bit/trit；极稀疏 → 索引；其余 1.6 bit/trit）。
//! - [`pack`]：5 trits / byte 二进制打包（1.6 bit/trit，比双导轨 2 bit/trit 省 20%）。
//! - [`balanced`]：平衡三进制整数 `+0-` 文本，i64 全域 41 位。
//! - [`hamming`]：GF(3) 上的 `[4,2,3]` 纠错码，保证纠正 1 个符号错误。
//! - [`quantize`]：f32 权重 ↔ trit 三值量化/反量化（TWN 风格，beta=0.7）。
//! - [`sparse`]：三值稀疏 COO 编码，只存非 Unknown 位置。
//! - [`interleave`]：块交织/解交织，把 burst error 打散后配合 hamming 纠错。
//! - [`checksum`]：GF(3) CRC-3 完整性校验，只检错不纠错。
//!
//! 大多数都是**无损**的：解码后必须逐位还原，任何非法输入都要显式报错而不是猜；
//! 唯一有损的是 [`quantize`]——它的目的就是压缩浮点到三值。

#![forbid(unsafe_code)]

pub mod balanced;
pub mod checksum;
pub mod hamming;
pub mod interleave;
pub mod pack;
pub mod plan;
pub mod quantize;
pub mod sparse;

/// 编解码错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// 字节落在 243..=255，不是合法的 5-trit 组合。
    #[error("非法字节 {0}：5 个 trit 只能表示 0..=242")]
    InvalidByte(u8),
    /// 最后一个字节里未使用的 trit 槽位不是 `False`，不是 canonical 编码。
    #[error("非规范尾部填充（字节 {0}）：未使用的 trit 槽位必须是 False")]
    NonCanonicalPadding(u8),
    /// 字节数与声明的 trit 长度不匹配。
    #[error("字节数不符：期望 {expected} 字节，实际 {actual}")]
    WrongByteCount {
        /// 期望的字节数。
        expected: usize,
        /// 实际字节数。
        actual: usize,
    },
    /// 请求的 trit 数超过字节容量。
    #[error("长度不足：需要 {expected} 个 trit，容量只有 {available}")]
    Length {
        /// 请求的 trit 数。
        expected: usize,
        /// 字节实际能表示的 trit 数。
        available: usize,
    },
    /// 平衡三进制里的非法字符。
    #[error("平衡三进制只接受 +/0/-，遇到 {0:?}")]
    InvalidDigit(char),
    /// 空字符串。
    #[error("平衡三进制字符串为空")]
    Empty,
    /// 数值超出 i64。
    #[error("数值超出 i64 范围")]
    Overflow,
    /// 稀疏下标越界。
    #[error("稀疏下标 {index} 越界（总长 {len}）")]
    SparseIndexOutOfRange {
        /// 越界的下标。
        index: u32,
        /// 序列总长。
        len: usize,
    },
    /// 稀疏下标不是严格递增。
    #[error("稀疏下标必须严格递增：上一个 {prev}，遇到 {found}")]
    SparseIndexNotIncreasing {
        /// 上一个下标。
        prev: u32,
        /// 当前下标。
        found: u32,
    },
    /// 交织块大小非法（0）。
    #[error("交织块大小必须 ≥1，收到 {0}")]
    BadBlockSize(usize),
    /// 解交织输入长度与声明长度不一致。
    #[error("解交织长度不符：声明 {expected}，实际 {actual}")]
    InterleaveLengthMismatch {
        /// 期望长度。
        expected: usize,
        /// 实际长度。
        actual: usize,
    },
}
