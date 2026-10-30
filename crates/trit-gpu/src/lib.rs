//! 三值点积的 GPU 实现（WGSL `countOneBits`）与 CPU 位并行对照。
//!
//! **`publish = false`，不属于稳定 API。** GPU 部分在 `gpu` feature 后面，
//! 默认构建不拉 `wgpu`（CI 的 MSRV/三平台 job 不会被 GPU 依赖拖累）。
//!
//! 编码：每个 trit 用双导轨 `(pos, neg)`，32 个 trit 打进一个 `u32`。
//! 点积公式与 CPU 版一致：
//!
//! ```text
//! dot = Σ popcount(a⁺ & b⁺) − Σ popcount(a⁺ & b⁻) − Σ popcount(a⁻ & b⁺) + Σ popcount(a⁻ & b⁻)
//! ```
//!
//! 实测（RTX 4060 Ti / Vulkan，16M trit）：GPU 三值 **6.7 ps/trit**，同卡 f32 点积
//! **83.9 ps/元素**（12.6×），CPU 位并行 54.4 ps/trit。1M 规模下反而 CPU 更快——
//! dispatch 延迟占主导，GPU 要到「数据大到装不进缓存」才拉开差距。
//!
//! 矩阵乘内核（`matmul.rs`，512×4096×512）六读数同尺：**naive 对 naive**
//! 三值 1.60 vs f32 25.89 ps/MAC（16.2×）；**共享内存分块**三值 0.208 vs f32
//! 2.641 ps/MAC（12.8×）；**分块 + 寄存器阻塞**（每线程 2×2 四累加器）三值
//! **0.129** vs f32 **2.712 ps/MAC**（**21.1×**）——三值从寄存器阻塞再赚 1.6×
//! （popcount 内循环吃到了指令级并行），f32 无收益（FMA 已饱和，负结果如实记）。
//! 三侧各自与 CPU 位并行镜像 [`matmul_words`] 逐元素一致（真机）。注意：tiled/reg
//! 内核在 `gpu` feature 后，默认构建的变异门测不到——正确性由真机对拍测试钉住。
//!
//! TT3 块纹理解码内核（`texture.rs`）与 trit-texture 的 CPU 解码逐字节对拍——
//! WGSL 里的基 3 取位与块网格行主序映射均已真机验证。

#![forbid(unsafe_code)]

/// 一个 32 位块装多少 trit。
pub const TRITS_PER_WORD: usize = 32;

/// 双导轨打包：`pos` 是 `+1` 的位，`neg` 是 `-1` 的位。
pub fn pack_dual_rail(values: &[i32]) -> (Vec<u32>, Vec<u32>) {
    let words = values.len().div_ceil(TRITS_PER_WORD);
    let mut pos = vec![0u32; words];
    let mut neg = vec![0u32; words];
    for (index, value) in values.iter().enumerate() {
        let bit = 1u32 << (index % TRITS_PER_WORD);
        match value {
            1 => pos[index / TRITS_PER_WORD] |= bit,
            -1 => neg[index / TRITS_PER_WORD] |= bit,
            _ => {}
        }
    }
    (pos, neg)
}

/// CPU 位并行三值点积（与 GPU 内核逐位一致）。
pub fn dot_words(a_pos: &[u32], a_neg: &[u32], b_pos: &[u32], b_neg: &[u32]) -> i64 {
    let mut total = 0i64;
    for block in 0..a_pos.len() {
        let pp = (a_pos[block] & b_pos[block]).count_ones();
        let pn = (a_pos[block] & b_neg[block]).count_ones();
        let np = (a_neg[block] & b_pos[block]).count_ones();
        let nn = (a_neg[block] & b_neg[block]).count_ones();
        total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);
    }
    total
}

/// 标量参考实现。
pub fn dot_scalar(a: &[i32], b: &[i32]) -> i64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| i64::from(*x) * i64::from(*y))
        .sum()
}

/// 把 `k × n` 的三值矩阵按**列**双导轨打包（得到 B 的转置行主序）。
///
/// GPU/CPU 矩阵乘内核都吃这个布局：`b_pos[n][w]` 是 B 第 n 列的第 w 个 32-trit 字。
/// 零填充的尾位与 0 相与不贡献 popcount，`k` 不必是 32 的倍数。
pub fn pack_columns(b: &[i32], k: usize, n: usize) -> (Vec<u32>, Vec<u32>) {
    let words = k.div_ceil(TRITS_PER_WORD);
    let mut pos = vec![0u32; n * words];
    let mut neg = vec![0u32; n * words];
    for col in 0..n {
        for row in 0..k {
            let value = b[row * n + col];
            let bit = 1u32 << (row % TRITS_PER_WORD);
            match value {
                1 => pos[col * words + row / TRITS_PER_WORD] |= bit,
                -1 => neg[col * words + row / TRITS_PER_WORD] |= bit,
                _ => {}
            }
        }
    }
    (pos, neg)
}

/// 把 `m × k` 的三值矩阵按**行**双导轨打包（每行各自整字，行间不跨界）。
pub fn pack_rows(a: &[i32], k: usize) -> (Vec<u32>, Vec<u32>) {
    let mut pos = Vec::new();
    let mut neg = Vec::new();
    for row in a.chunks(k) {
        let (row_pos, row_neg) = pack_dual_rail(row);
        pos.extend_from_slice(&row_pos);
        neg.extend_from_slice(&row_neg);
    }
    (pos, neg)
}

/// CPU 位并行三值矩阵乘（GPU 内核的逐位镜像）。
///
/// `a_*` 是 A 的行主序双导轨（m 行 × words 字），`b_*` 是 B 的**转置**行主序
/// （n 列 × words 字，来自 [`pack_columns`]）；返回 `m × n` 行主序的 `i64`。
/// 每个输出就是 [`dot_words`] 的一次行对行点积。
pub fn matmul_words(
    m: usize,
    n: usize,
    a_pos: &[u32],
    a_neg: &[u32],
    b_pos: &[u32],
    b_neg: &[u32],
) -> Vec<i64> {
    let words = a_pos.len().checked_div(m).unwrap_or(0);
    let mut out = Vec::with_capacity(m * n);
    for row in 0..m {
        let a_base = row * words;
        for col in 0..n {
            let b_base = col * words;
            out.push(dot_words(
                &a_pos[a_base..a_base + words],
                &a_neg[a_base..a_base + words],
                &b_pos[b_base..b_base + words],
                &b_neg[b_base..b_base + words],
            ));
        }
    }
    out
}

/// 一次运行的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// 设备名。
    pub adapter: String,
    /// 后端（Vulkan / Metal / Dx12 …）。
    pub backend: String,
    /// trit 数。
    pub trits: usize,
    /// CPU 标量结果。
    pub expected: i64,
    /// CPU 位并行结果。
    pub cpu: i64,
    /// GPU 结果。
    pub gpu: i64,
    /// GPU 与 CPU 是否一致。
    pub matches: bool,
    /// GPU 三值点积（默认内核：四组 POPC + 每字 atomic），ns / trit。
    pub gpu_ns_per_trit: f64,
    /// GPU 三值点积：只合并 POPC（两组）+ 每字 atomic，ns / trit。
    pub gpu_popc2_ns_per_trit: f64,
    /// GPU 三值点积：合并 POPC + workgroup 归约，ns / trit。
    pub gpu_reduce_ns_per_trit: f64,
    /// CPU 位并行（ns / trit）。
    pub cpu_ns_per_trit: f64,
    /// 同设备 f32 点积（ns / 元素）。
    pub gpu_f32_ns_per_element: f64,
    /// CPU f32 点积（四累加器，ns / 元素）。
    pub cpu_f32_ns_per_element: f64,
    /// f32 相对误差。
    pub float_relative_error: f64,
}

#[cfg(feature = "gpu")]
mod bench;
#[cfg(feature = "gpu")]
pub mod forward;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "gpu")]
pub mod matmul;
#[cfg(feature = "gpu")]
mod matmul_f32;
#[cfg(feature = "gpu")]
mod popc;
#[cfg(feature = "gpu")]
pub mod sampler;
#[cfg(feature = "gpu")]
pub mod texture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_round_trips_across_word_boundaries() {
        for len in [1usize, 31, 32, 33, 100] {
            let a: Vec<i32> = (0..len).map(|i| (i % 3) as i32 - 1).collect();
            let b: Vec<i32> = (0..len).map(|i| ((i * 2) % 3) as i32 - 1).collect();
            let (a_pos, a_neg) = pack_dual_rail(&a);
            let (b_pos, b_neg) = pack_dual_rail(&b);
            assert_eq!(
                dot_words(&a_pos, &a_neg, &b_pos, &b_neg),
                dot_scalar(&a, &b),
                "len={len}"
            );
        }
    }

    #[test]
    fn all_four_sign_products_are_counted() {
        // 四种组合各出现一次：(+1,-1) 与 (-1,+1) 各贡献 -1，(+1,+1)=+1，(-1,-1)=+1。
        let (a_pos, a_neg) = pack_dual_rail(&[1, -1, 1, -1]);
        let (b_pos, b_neg) = pack_dual_rail(&[-1, 1, 1, -1]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b_pos, &b_neg), 0);
        assert_eq!(dot_scalar(&[1, -1, 1, -1], &[-1, 1, 1, -1]), 0);
        // 只保留最后一个乘积 (-1)(-1)=+1，其余为 -1：净和 1-1-0+2 = 2。
        let (b2_pos, b2_neg) = pack_dual_rail(&[-1, -1, 1, -1]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b2_pos, &b2_neg), 2);
    }

    #[test]
    fn positive_vectors_do_not_land_on_the_negative_rail() {
        // a 只有 +1、b 只有 -1：两侧必须落在不同导轨，乘积才是 -1。
        // （若 a、b 同形，把 +1 写错到负导轨会两边同时错、结果反而对得上。）
        let (a_pos, a_neg) = pack_dual_rail(&[1, 0, 0, 0]);
        let (b_pos, b_neg) = pack_dual_rail(&[-1, 0, 0, 0]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b_pos, &b_neg), -1);
    }

    #[test]
    fn column_packing_and_matmul_match_scalar_reference() {
        // 非整字边界（k=100 → 4 字尾 28 位零填充）+ 非方阵，边界与转置一起钉。
        let m = 7usize;
        let k = 100usize;
        let n = 5usize;
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 3) as i32 - 1
        };
        let a: Vec<i32> = (0..m * k).map(|_| next()).collect();
        let b: Vec<i32> = (0..k * n).map(|_| next()).collect();
        let mut a_rows: Vec<Vec<i32>> = Vec::new();
        for row in 0..m {
            a_rows.push(a[row * k..(row + 1) * k].to_vec());
        }
        let (a_pos, a_neg) = pack_rows(&a, k);
        let (b_pos, b_neg) = pack_columns(&b, k, n);
        let packed = matmul_words(m, n, &a_pos, &a_neg, &b_pos, &b_neg);
        for row in 0..m {
            for col in 0..n {
                let column: Vec<i32> = (0..k).map(|r| b[r * n + col]).collect();
                let expected = dot_scalar(&a_rows[row], &column);
                assert_eq!(
                    packed[row * n + col],
                    expected,
                    "({row},{col}) 打包路径与标量参考不一致"
                );
            }
        }
    }

    #[test]
    fn zeros_do_not_contribute() {
        let (a_pos, a_neg) = pack_dual_rail(&[1, 0, -1, 0]);
        let (b_pos, b_neg) = pack_dual_rail(&[1, 1, -1, -1]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b_pos, &b_neg), 2);
    }
}
