//! 三值点积的位并行实现，以及「XNOR+POPC」到底能算什么。
//!
//! 先把话说清楚：
//!
//! - **XNOR + POPC 只能算二值（±1）点积**：`dot = n - 2·popcount(a XOR b)`。
//!   三值 `{-1,0,+1}` 不能塞进 1 bit，所以它算不了三值点积。
//! - **三值点积用双导轨**（每个 trit 2 bit：`pos` / `neg`）时是 4 组 AND + POPC：
//!   `dot = Σa⁺b⁺ - Σa⁺b⁻ - Σa⁻b⁺ + Σa⁻b⁻`。
//! - GPU 晶体管不是三值的；XNOR 是二进制门。三值在硬件上始终是**2 bit 模拟**。

// 唯一的 unsafe 在 `popcnt` 模块里（一条 `popcnt` 指令的运行时派发）；
// 其余代码仍按 deny 管住，新加的 unsafe 必须显式 `#[allow]` 才能进来。
#![deny(unsafe_code)]

use trit_core::value::Ternary;

/// 一个块有多少位。
pub const BLOCK_BITS: usize = 64;

/// 双导轨三值向量（每 trit 2 bit，按 64 位块打包）。
///
/// 字段私有（复审 L01 前是公开的）：手工拼出「长度不一致 / 正负导轨重叠 / 尾部有脏位」
/// 的向量会让 `dot` 与 `dot_portable` 给出不同结果（前者 panic，后者静默截断）。
/// 现在只能经 [`TernaryBits::new`]（校验）或 `from_trits`/`from_ternary` 构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TernaryBits {
    pos: Vec<u64>,
    neg: Vec<u64>,
    len: usize,
}

/// 双导轨向量的构造/点积错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotError {
    /// 平面长度不一致、正负导轨重叠或尾部有脏位。
    Shape(String),
    /// 两个向量的有效 trit 数不同。
    LengthMismatch {
        /// 左侧长度。
        left: usize,
        /// 右侧长度。
        right: usize,
    },
}

impl std::fmt::Display for DotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(message) => write!(formatter, "双导轨形状非法：{message}"),
            Self::LengthMismatch { left, right } => {
                write!(formatter, "点积长度不一致：{left} vs {right}")
            }
        }
    }
}

impl std::error::Error for DotError {}

impl TernaryBits {
    /// 从已打包的平面构造并校验。
    ///
    /// 规则：`pos`/`neg` 长度都必须等于 `len.div_ceil(BLOCK_BITS)`；同一 trit 的正负
    /// 导轨不能同时为 1；`len` 之后的填充位必须为 0（否则会静默参与点积）。
    pub fn new(pos: Vec<u64>, neg: Vec<u64>, len: usize) -> Result<Self, DotError> {
        let expected = len.div_ceil(BLOCK_BITS);
        if pos.len() != expected || neg.len() != expected {
            return Err(DotError::Shape(format!(
                "len={len} 需要 {expected} 个 64 位块，实际 pos={} neg={}",
                pos.len(),
                neg.len()
            )));
        }
        if let Some((block, overlap)) = pos
            .iter()
            .zip(neg.iter())
            .enumerate()
            .map(|(block, (pos, neg))| (block, pos & neg))
            .find(|(_, overlap)| *overlap != 0)
        {
            return Err(DotError::Shape(format!(
                "第 {block} 块正负导轨重叠：{overlap:#x}"
            )));
        }
        let used = len % BLOCK_BITS;
        if used != 0 {
            let mask = !((1u64 << used) - 1);
            let dirty_pos = pos[expected - 1] & mask;
            let dirty_neg = neg[expected - 1] & mask;
            if dirty_pos != 0 || dirty_neg != 0 {
                return Err(DotError::Shape(format!(
                    "尾部填充位非零：pos={dirty_pos:#x} neg={dirty_neg:#x}"
                )));
            }
        }
        Ok(Self { pos, neg, len })
    }

    /// 有效 trit 数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空向量。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 正导轨（只读）。
    pub fn pos(&self) -> &[u64] {
        &self.pos
    }

    /// 负导轨（只读）。
    pub fn neg(&self) -> &[u64] {
        &self.neg
    }

    /// 从 `{-1,0,+1}` 构造。
    pub fn from_trits(values: &[i32]) -> Self {
        let blocks = values.len().div_ceil(BLOCK_BITS);
        let mut pos = vec![0u64; blocks];
        let mut neg = vec![0u64; blocks];
        for (index, value) in values.iter().enumerate() {
            let bit = 1u64 << (index % BLOCK_BITS);
            match value {
                1 => pos[index / BLOCK_BITS] |= bit,
                -1 => neg[index / BLOCK_BITS] |= bit,
                _ => {}
            }
        }
        Self {
            pos,
            neg,
            len: values.len(),
        }
    }

    /// 从逻辑三值构造（`X` 记 0）。
    pub fn from_ternary(values: &[Ternary]) -> Self {
        let digits: Vec<i32> = values
            .iter()
            .map(|value| match value {
                Ternary::False => -1,
                Ternary::Unknown => 0,
                Ternary::True => 1,
            })
            .collect();
        Self::from_trits(&digits)
    }

    /// 精确的三值点积：两组 POPC（`pp+nn` 与 `pn+np` 各合一次）。
    ///
    /// 内部走 [`dot_words`]，x86_64 上会派发到硬件 `popcnt`。
    pub fn dot(&self, other: &Self) -> Result<i64, DotError> {
        self.check_length(other)?;
        dot_words(&self.pos, &self.neg, &other.pos, &other.neg)
    }

    /// 便携版三值点积（单累加器，默认代码生成），无派发的对照/回退路径。
    pub fn dot_portable(&self, other: &Self) -> Result<i64, DotError> {
        self.check_length(other)?;
        dot_words_portable(&self.pos, &self.neg, &other.pos, &other.neg)
    }

    /// 旧写法：四组 POPC（A/B 对照）。
    pub fn dot_naive(&self, other: &Self) -> Result<i64, DotError> {
        self.check_length(other)?;
        let mut total = 0i64;
        for block in 0..self.pos.len() {
            let pp = (self.pos[block] & other.pos[block]).count_ones();
            let pn = (self.pos[block] & other.neg[block]).count_ones();
            let np = (self.neg[block] & other.pos[block]).count_ones();
            let nn = (self.neg[block] & other.neg[block]).count_ones();
            total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);
        }
        Ok(total)
    }

    /// 块级稀疏跳过：两边只要有一边整块为 0 就跳过（4 组 AND/POPC 全免）。
    pub fn dot_sparse(&self, other: &Self) -> Result<i64, DotError> {
        self.check_length(other)?;
        let mut total = 0i64;
        for block in 0..self.pos.len() {
            let a = self.pos[block] | self.neg[block];
            let b = other.pos[block] | other.neg[block];
            if a == 0 || b == 0 {
                continue;
            }
            let pp = (self.pos[block] & other.pos[block]).count_ones();
            let pn = (self.pos[block] & other.neg[block]).count_ones();
            let np = (self.neg[block] & other.pos[block]).count_ones();
            let nn = (self.neg[block] & other.neg[block]).count_ones();
            total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);
        }
        Ok(total)
    }

    fn check_length(&self, other: &Self) -> Result<(), DotError> {
        if self.len != other.len {
            return Err(DotError::LengthMismatch {
                left: self.len,
                right: other.len,
            });
        }
        Ok(())
    }

    /// 非零块数（稀疏度指标）。
    pub fn nonzero_blocks(&self) -> usize {
        self.pos
            .iter()
            .zip(self.neg.iter())
            .filter(|(pos, neg)| **pos | **neg != 0)
            .count()
    }
}

/// 三值点积的循环体：4 路展开，四个独立累加器打断 `total` 的依赖链。
///
/// `#[inline(always)]` 不是调优偏好：硬件派发靠「把小函数内联进
/// `#[target_feature(enable = "popcnt")]` 函数」来拿到 `popcnt` 指令，
/// 不内联的话派发就白做了。
#[inline(always)]
fn unrolled_body(a_pos: &[u64], a_neg: &[u64], b_pos: &[u64], b_neg: &[u64]) -> i64 {
    let n = a_pos
        .len()
        .min(a_neg.len())
        .min(b_pos.len())
        .min(b_neg.len());
    let (a_pos, rest_a_pos) = a_pos[..n].as_chunks::<4>();
    let (a_neg, rest_a_neg) = a_neg[..n].as_chunks::<4>();
    let (b_pos, rest_b_pos) = b_pos[..n].as_chunks::<4>();
    let (b_neg, rest_b_neg) = b_neg[..n].as_chunks::<4>();
    let mut sums = [0i64; 4];
    for (((pa, na), pb), nb) in a_pos.iter().zip(a_neg).zip(b_pos).zip(b_neg) {
        for lane in 0..4 {
            let matches = ((pa[lane] & pb[lane]) | (na[lane] & nb[lane])).count_ones();
            let mismatches = ((pa[lane] & nb[lane]) | (na[lane] & pb[lane])).count_ones();
            sums[lane] += i64::from(matches) - i64::from(mismatches);
        }
    }
    let mut total: i64 = sums.iter().sum();
    for lane in 0..rest_a_pos.len() {
        let matches = ((rest_a_pos[lane] & rest_b_pos[lane])
            | (rest_a_neg[lane] & rest_b_neg[lane]))
            .count_ones();
        let mismatches = ((rest_a_pos[lane] & rest_b_neg[lane])
            | (rest_a_neg[lane] & rest_b_pos[lane]))
            .count_ones();
        total += i64::from(matches) - i64::from(mismatches);
    }
    total
}

/// 4 路展开的三值点积（不做派发，用来量「只改循环形状」的净收益）。
pub fn dot_words_unrolled(
    a_pos: &[u64],
    a_neg: &[u64],
    b_pos: &[u64],
    b_neg: &[u64],
) -> Result<i64, DotError> {
    check_planes(a_pos, a_neg, b_pos, b_neg)?;
    Ok(unrolled_body(a_pos, a_neg, b_pos, b_neg))
}

/// 便携版三值点积：单累加器。
pub fn dot_words_portable(
    a_pos: &[u64],
    a_neg: &[u64],
    b_pos: &[u64],
    b_neg: &[u64],
) -> Result<i64, DotError> {
    check_planes(a_pos, a_neg, b_pos, b_neg)?;
    let n = a_pos.len();
    let mut total = 0i64;
    for block in 0..n {
        let matches = ((a_pos[block] & b_pos[block]) | (a_neg[block] & b_neg[block])).count_ones();
        let mismatches =
            ((a_pos[block] & b_neg[block]) | (a_neg[block] & b_pos[block])).count_ones();
        total += i64::from(matches) - i64::from(mismatches);
    }
    Ok(total)
}

/// 默认三值点积入口：能派发就派发到硬件 `popcnt`，否则用便携版。
pub fn dot_words(
    a_pos: &[u64],
    a_neg: &[u64],
    b_pos: &[u64],
    b_neg: &[u64],
) -> Result<i64, DotError> {
    check_planes(a_pos, a_neg, b_pos, b_neg)?;
    #[cfg(target_arch = "x86_64")]
    if let Some(total) = popcnt::dot_words(a_pos, a_neg, b_pos, b_neg) {
        return Ok(total);
    }
    dot_words_portable(a_pos, a_neg, b_pos, b_neg)
}

/// 四个平面长度必须一致；不满足就返回 `LengthMismatch`（旧实现按最短长度静默截断）。
fn check_planes(
    a_pos: &[u64],
    a_neg: &[u64],
    b_pos: &[u64],
    b_neg: &[u64],
) -> Result<(), DotError> {
    let left = a_pos.len().min(a_neg.len());
    let right = b_pos.len().min(b_neg.len());
    if a_pos.len() != a_neg.len() || b_pos.len() != b_neg.len() || left != right {
        return Err(DotError::LengthMismatch { left, right });
    }
    Ok(())
}

/// 硬件 `POPCNT` 派发。
///
/// 本机实测（Arrow Lake，1 Mi trit）：默认代码生成把 `count_ones` 展开成约 12 条
/// 软件位运算，`objdump` 里一条 `popcnt` 都没有；开 `popcnt` 后同一条循环
/// 0.0128 → 0.0095 ns/trit（单累加器）/ 0.0088（4 路展开）。所以派发在这条
/// 路径上是**实打实的指令数收益**，不是带宽或缓存效应。
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
mod popcnt {
    /// 调用者必须已确认 CPU 支持 `POPCNT`。
    #[target_feature(enable = "popcnt")]
    unsafe fn dot(a_pos: &[u64], a_neg: &[u64], b_pos: &[u64], b_neg: &[u64]) -> i64 {
        super::unrolled_body(a_pos, a_neg, b_pos, b_neg)
    }

    /// 调用者必须已确认 CPU 支持 `POPCNT`。
    #[target_feature(enable = "popcnt")]
    unsafe fn bipolar_mismatches(a: &[u64], b: &[u64]) -> u64 {
        let mut mismatches = 0u64;
        for (left, right) in a.iter().zip(b.iter()) {
            mismatches += u64::from((left ^ right).count_ones());
        }
        mismatches
    }

    pub(super) fn dot_words(
        a_pos: &[u64],
        a_neg: &[u64],
        b_pos: &[u64],
        b_neg: &[u64],
    ) -> Option<i64> {
        if !std::arch::is_x86_feature_detected!("popcnt") {
            return None;
        }
        // SAFETY: 上面的检测就是这条指令集的存在性。
        Some(unsafe { dot(a_pos, a_neg, b_pos, b_neg) })
    }

    pub(super) fn bipolar_mismatches_or_zero(a: &[u64], b: &[u64]) -> Option<u64> {
        if !std::arch::is_x86_feature_detected!("popcnt") {
            return None;
        }
        // SAFETY: 同上。
        Some(unsafe { bipolar_mismatches(a, b) })
    }
}

/// 二值（`±1`）向量的位打包：`1 = +1`、`0 = -1`，**尾部补 1**。
pub fn pack_bipolar(values: &[i32]) -> Vec<u64> {
    let blocks = values.len().div_ceil(BLOCK_BITS);
    let mut words = vec![u64::MAX; blocks];
    for (index, value) in values.iter().enumerate() {
        if *value < 0 {
            words[index / BLOCK_BITS] &= !(1u64 << (index % BLOCK_BITS));
        }
    }
    words
}

/// 二值（`±1`）点积：`n - 2·popcount(a XOR b)`，也就是「XNOR + POPC」。
///
/// 与三值路径一样走硬件 `popcnt` 派发——对照必须同待遇，不然比的不是编码而是
/// 谁碰巧拿到了指令。
pub fn bipolar_dot(a: &[u64], b: &[u64], len: usize) -> i64 {
    #[cfg(target_arch = "x86_64")]
    if let Some(mismatches) = popcnt::bipolar_mismatches_or_zero(a, b) {
        return len as i64 - 2 * mismatches as i64;
    }
    bipolar_dot_portable(a, b, len)
}

/// 便携版二值点积（不做派发），用来量派发本身值多少。
pub fn bipolar_dot_portable(a: &[u64], b: &[u64], len: usize) -> i64 {
    let mismatches: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(left, right)| u64::from((left ^ right).count_ones()))
        .sum();
    len as i64 - 2 * mismatches as i64
}

/// 标量参考实现。
pub fn scalar_dot(a: &[i32], b: &[i32]) -> i64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| i64::from(*x) * i64::from(*y))
        .sum()
}

/// `f32` 参考实现（朴素单累加器，含串行依赖）。
pub fn float_dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// `f32` 参考实现：4 个累加器，打断串行依赖、便于自动向量化（公平对照）。
pub fn float_dot_4acc(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0.0f32; 4];
    let (chunks_a, rest_a) = a.as_chunks::<4>();
    let (chunks_b, rest_b) = b.as_chunks::<4>();
    for (left, right) in chunks_a.iter().zip(chunks_b.iter()) {
        for lane in 0..4 {
            sums[lane] += left[lane] * right[lane];
        }
    }
    let mut total: f32 = sums.iter().sum();
    for (x, y) in rest_a.iter().zip(rest_b.iter()) {
        total += x * y;
    }
    total
}

/// 每条 64 位块的理论指令数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpCount {
    /// 二值点积：XOR + POPC。
    pub bipolar: usize,
    /// 三值点积：4 AND + 4 POPC。
    pub ternary: usize,
    /// 稀疏跳过时每个零块的探测成本：2 OR + 2 分支判断。
    pub sparse_probe: usize,
}

/// 理论指令数（每个 64 位块）。
pub const OPS: OpCount = OpCount {
    bipolar: 2,
    ternary: 8,
    sparse_probe: 2,
};

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn ternary(&mut self) -> i32 {
            (self.next() % 3) as i32 - 1
        }
        fn bipolar(&mut self) -> i32 {
            if self.next().is_multiple_of(2) { 1 } else { -1 }
        }
    }

    #[test]
    fn ternary_dot_matches_scalar_across_block_boundaries() {
        for len in [1usize, 63, 64, 65, 200] {
            for seed in 1..=20u64 {
                let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                let a: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let b: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let bits_a = TernaryBits::from_trits(&a);
                let bits_b = TernaryBits::from_trits(&b);
                let expected = scalar_dot(&a, &b);
                assert_eq!(
                    bits_a.dot(&bits_b).expect("同长度"),
                    expected,
                    "len={len} seed={seed}"
                );
                assert_eq!(
                    bits_a.dot_sparse(&bits_b).expect("同长度"),
                    expected,
                    "sparse len={len} seed={seed}"
                );
            }
        }
    }

    #[test]
    fn bipolar_dot_matches_scalar() {
        for len in [1usize, 64, 129] {
            let mut rng = Rng(7);
            let a: Vec<i32> = (0..len).map(|_| rng.bipolar()).collect();
            let b: Vec<i32> = (0..len).map(|_| rng.bipolar()).collect();
            let packed_a = pack_bipolar(&a);
            let packed_b = pack_bipolar(&b);
            assert_eq!(bipolar_dot(&packed_a, &packed_b, len), scalar_dot(&a, &b));
        }
    }

    #[test]
    fn bipolar_formula_is_n_minus_twice_mismatches() {
        let a = pack_bipolar(&[1, 1, 1, 1]);
        let b = pack_bipolar(&[1, -1, 1, -1]);
        // 两处不同：4 - 2*2 = 0。
        assert_eq!(bipolar_dot(&a, &b, 4), 0);
    }

    #[test]
    fn zeros_contribute_nothing() {
        let a = TernaryBits::from_trits(&[1, 0, -1, 0]);
        let b = TernaryBits::from_trits(&[1, 1, -1, -1]);
        assert_eq!(a.dot(&b).expect("同长度"), 2);
        assert_eq!(a.nonzero_blocks(), 1);
    }

    #[test]
    fn sparse_skip_is_exact_on_zero_blocks() {
        let mut a = vec![0i32; 256];
        let mut b = vec![0i32; 256];
        a[0] = 1;
        a[255] = -1;
        b[0] = 1;
        b[255] = 1;
        let bits_a = TernaryBits::from_trits(&a);
        let bits_b = TernaryBits::from_trits(&b);
        assert_eq!(bits_a.dot(&bits_b).expect("同长度"), 0);
        assert_eq!(bits_a.dot_sparse(&bits_b).expect("同长度"), 0);
        // 256 个元素 = 4 块；非零落在块 0 和块 3。
        assert_eq!(bits_a.nonzero_blocks(), 2);
        assert_eq!(bits_b.nonzero_blocks(), 2);
    }

    #[test]
    fn unrolled_matches_dense_on_all_tail_lengths() {
        // 块数覆盖 4 的整倍数与各种尾巴（0..=3 块余数）。
        for blocks in [0usize, 1, 3, 4, 5, 7, 8, 9] {
            for seed in 1..=7u64 {
                let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
                let len = blocks * BLOCK_BITS + (seed as usize % BLOCK_BITS);
                let a: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let b: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let bits_a = TernaryBits::from_trits(&a);
                let bits_b = TernaryBits::from_trits(&b);
                let expected = scalar_dot(&a, &b);
                assert_eq!(
                    dot_words_unrolled(bits_a.pos(), bits_a.neg(), bits_b.pos(), bits_b.neg())
                        .expect("同长度"),
                    expected,
                    "blocks={blocks} len={len} seed={seed}"
                );
                assert_eq!(bits_a.dot(&bits_b).expect("同长度"), expected);
            }
        }
    }

    #[test]
    fn unrolled_keeps_lane_contributions_separate() {
        // 四个 lane 各自只被一侧数据碰到：lane 相加时若串了累加器就会对不上。
        let mut a = vec![0i32; 4 * BLOCK_BITS];
        let mut b = vec![0i32; 4 * BLOCK_BITS];
        a[0] = 1;
        a[BLOCK_BITS] = -1;
        a[2 * BLOCK_BITS] = 1;
        a[3 * BLOCK_BITS] = -1;
        b[0] = 1;
        b[BLOCK_BITS] = 1;
        b[2 * BLOCK_BITS] = -1;
        b[3 * BLOCK_BITS] = -1;
        let bits_a = TernaryBits::from_trits(&a);
        let bits_b = TernaryBits::from_trits(&b);
        assert_eq!(scalar_dot(&a, &b), 0);
        assert_eq!(
            dot_words_unrolled(bits_a.pos(), bits_a.neg(), bits_b.pos(), bits_b.neg())
                .expect("同长度"),
            0
        );
    }

    #[test]
    fn dispatch_matches_portable() {
        // 派发可能落到硬件 popcnt 或便携实现；两条路径必须逐值一致。
        for blocks in [0usize, 1, 4, 17] {
            for seed in 1..=7u64 {
                let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                let len = blocks * BLOCK_BITS;
                let a: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let b: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let bits_a = TernaryBits::from_trits(&a);
                let bits_b = TernaryBits::from_trits(&b);
                assert_eq!(
                    dot_words(bits_a.pos(), bits_a.neg(), bits_b.pos(), bits_b.neg())
                        .expect("同长度"),
                    dot_words_portable(bits_a.pos(), bits_a.neg(), bits_b.pos(), bits_b.neg())
                        .expect("同长度"),
                    "blocks={blocks} seed={seed}"
                );
                assert_eq!(
                    bipolar_dot(bits_a.pos(), bits_b.pos(), len),
                    bipolar_dot_portable(bits_a.pos(), bits_b.pos(), len)
                );
            }
        }
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let a = TernaryBits::from_trits(&[1, 0]);
        let b = TernaryBits::from_trits(&[1]);
        assert_eq!(
            a.dot(&b),
            Err(DotError::LengthMismatch { left: 2, right: 1 })
        );
        assert_eq!(
            a.dot_portable(&b),
            Err(DotError::LengthMismatch { left: 2, right: 1 })
        );
        assert_eq!(a.dot_naive(&b), a.dot_sparse(&b));
    }

    #[test]
    fn invalid_planes_are_rejected_at_construction() {
        // 复审 L01：字段公开时可以把平面长度/重叠/尾部脏位拼错，dot 与 dot_portable 各错一边。
        assert!(matches!(
            TernaryBits::new(vec![1], vec![0], 128),
            Err(DotError::Shape(_))
        ));
        assert!(matches!(
            TernaryBits::new(vec![0b11], vec![0b01], 2),
            Err(DotError::Shape(_))
        ));
        assert!(matches!(
            TernaryBits::new(vec![0b1000], vec![0], 2),
            Err(DotError::Shape(_))
        ));
        let ok = TernaryBits::new(vec![0b11], vec![0], 2).expect("合法双导轨");
        assert_eq!(ok.dot(&ok).expect("同长度"), 2);
        assert_eq!(ok.len(), 2);
        assert_eq!(ok.pos(), &[0b11]);
        assert_eq!(ok.neg(), &[0]);
    }
}
