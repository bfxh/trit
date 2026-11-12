//! 编解码错误类型（从 lib.rs 拆出，纯搬家不改语义）。

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
    /// 稀疏解码长度超过文档化硬顶（在分配**之前**拒绝，外部 `--len` 不许撑爆内存）。
    #[error("稀疏解码长度 {len} 超过硬顶 {max}——分配前拒绝，不接受外部任意长度")]
    SparseLenTooLarge {
        /// 请求的解码长度。
        len: usize,
        /// 允许的最大解码长度。
        max: usize,
    },
    /// 稀疏解码长度在硬顶内但内存 reservation 失败（结构化错误，不 abort）。
    #[error("稀疏解码无法分配 {len} 个 trit 的内存")]
    SparseAllocFailed {
        /// 请求的解码长度。
        len: usize,
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
    /// RLE 游程计数为 0（空游程无意义，直接拒绝）。
    #[error("RLE 游程计数不能为 0")]
    RleZeroCount,
    /// RLE 展开后总长度与声明的期望长度不一致。
    #[error("RLE 长度不符：期望 {expected}，展开实际 {actual}")]
    RleLengthMismatch {
        /// 期望长度。
        expected: usize,
        /// 游程展开后的实际长度。
        actual: usize,
    },
    /// 格雷码 index 超出 `[0, 3^n)`。
    #[error("格雷码 index {index} 越界（n={n}，合法范围 0 到 3^n-1）")]
    GrayIndexOutOfRange {
        /// 请求的 index。
        index: usize,
        /// 码字位数 n。
        n: usize,
    },
    /// 真值表同一输入向量出现两次，但输出不一致。
    #[error("真值表冲突：输入行 {row} 与之前的输出矛盾")]
    TruthTableConflict {
        /// 冲突行的下标（按调用方传入顺序）。
        row: usize,
    },
    /// 真值表不是 SOP 可表示的：化简结果无法与 True 集合逐点等价
    /// （存在某个 True 赋值，无法用「不过度覆盖」的积项覆盖）。
    #[error("真值表不是 SOP 可表示的：化简结果与 True 集合逐点不等价")]
    TruthTableNotRepresentable,
    /// SOP 积项长度不一致。
    #[error("SOP 积项元数不一致：期望 {expected}，遇到 {found}")]
    TermArityMismatch {
        /// 首个积项的长度。
        expected: usize,
        /// 冲突积项的长度。
        found: usize,
    },
    /// 积项数超过容斥枚举的可表示上限（`2^m` 个子集）。
    #[error("积项数 {found} 超过容斥上限 {max}（需枚举 2^{found} 个子集）")]
    TermSetTooLarge {
        /// 实际收到的积项数。
        found: usize,
        /// 允许的最大积项数。
        max: usize,
    },
    /// 三值立方体 `3^n` 超出 `u64` 表示范围。
    #[error("三值立方体 3^{n} 超出 u64 范围（n 最大允许 {max}）")]
    CubeTooLarge {
        /// 实际位数 n。
        n: usize,
        /// 允许的最大位数（3^max < u64::MAX）。
        max: usize,
    },
}
