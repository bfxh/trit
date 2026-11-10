//! 三值权重 × f32 激活的矩阵向量乘（decode 的主算子）。
//!
//! 同一份权重可以按两种格式存、走三条内核：
//!
//! | 格式 | 内核 | 关键指令 | 每 trit 字节 |
//! |---|---|---|---|
//! | [`TernaryMatrix`] 双导轨 | [`Kernel::Scalar`] | 标量分支 | 2 bit |
//! | [`TernaryMatrix`] 双导轨 | [`Kernel::Mask`] | 掩码 `AND` + 加减 | 2 bit |
//! | [`TryteMatrix`] tryte | [`Kernel::Lut`] | `3^5` 查表 | 1.6 bit |
//!
//! 三条内核互为交叉验证：同一权重、同一激活，结果必须一致。

use crate::pack::{TRITS_PER_BYTE, TRITS_PER_WORD, TRYTE_STATES, pack_row};
use crate::pool::{Ptr, par_rows_mut};
use crate::trit::Trit;

/// 内核选择。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kernel {
    /// 标量参考实现（正确性基准）。
    Scalar,
    /// 双导轨 + AVX2 掩码累加，无浮点乘法。
    Mask,
    /// tryte 查表（`3^5`）。
    Lut,
    /// 按行数自动选：小行数走掩码，大行数走查表。
    Auto,
}

/// 跨内核切换阈值：行数达到这个量级时，tryte 查表才可靠地快过双导轨掩码。
///
/// **2026-10-26 重标定**：掩码内核改成行阻塞 2 之后快了 4–10%，旧阈值 40960 已失效。
/// 实测（`bench --pair 掩码 tryte`，cols=576，tryte/mask 中位比值与胜场）：
///
/// | rows | 比值 | tryte 更快 | 轮数 |
/// |---:|---:|---:|---:|
/// | 32768 | 0.855 | 2/9 | 9 |
/// | 49152 | 0.945 | 4/15 | 15 |
/// | 65536 | 1.004 | 5/9 | 9 |
/// | 81920 | **1.164** | **9/9** | 9 |
/// | 98304 | **1.190** | **9/9** | 9 |
///
/// 交叉点夹在 49152 与 81920 之间，取 65536（该点两种内核打平，往上 tryte 稳定胜）。
/// **旧的 40960 已不成立**：49152 行实际是掩码更快（旧值会让输出层走更慢的查表）；
/// 更早的 2048 更是错的。
pub const AUTO_LUT_ROWS: usize = 65536;

/// 拆表方案：每个 5-trit 字节拆成「低 2 位」和「高 3 位」两张小表。
///
/// 整字节表每张 243 项（`3^5`），拆开是 9 + 27 = 36 项，建表便宜 6.75 倍；
/// 代价是每个字节要查两次表。
pub const SPLIT_LOW_STATES: usize = 9;
/// 高 3 位表的项数（`3^3`）。
pub const SPLIT_HIGH_STATES: usize = 27;
/// 拆表每字节的总项数。
pub const SPLIT_STATES: usize = SPLIT_LOW_STATES + SPLIT_HIGH_STATES;

/// 行数低于这个值时用拆表（建表便宜），高于则用整字节表（查表便宜）。
///
/// 实测交叉点（`bench --split`，cols=576，各 5–7 次取中位，拆表+映射/整表耗时比）：
/// `1792 → 0.349×`、`2048 → 1.067×`，所以阈值仍取 2048。
pub const LUT_SPLIT_ROWS: usize = 2048;

/// `code -> 拆表偏移`：低字节 = `code % 9`，高字节 = `code / 9`。
///
/// 拆表查表每字节原本要做一次 `% 9` 和一次 `/ 9`；换成 243 项映射（486 B，
/// 常驻 L1）后压成一次装入。两种写法累加顺序相同，结果逐位一致。
const SPLIT_MAP: [u16; TRYTE_STATES] = build_split_map();

const fn build_split_map() -> [u16; TRYTE_STATES] {
    let mut t = [0u16; TRYTE_STATES];
    let mut c = 0;
    while c < TRYTE_STATES {
        t[c] = (c % SPLIT_LOW_STATES) as u16 | (((c / SPLIT_LOW_STATES) as u16) << 8);
        c += 1;
    }
    t
}

/// 双导轨三值矩阵（行主序，每行补齐到 64 的倍数）。
#[derive(Clone, Debug)]
pub struct TernaryMatrix {
    rows: usize,
    cols: usize,
    bpr: usize,
    pos: Vec<u64>,
    neg: Vec<u64>,
}

impl TernaryMatrix {
    /// 从按行给出的 trit 构造；每行长度必须等于 `cols`。
    pub fn from_rows(rows: &[Vec<Trit>], cols: usize) -> Self {
        let bpr = cols.div_ceil(TRITS_PER_WORD).max(1);
        let mut pos = vec![0u64; rows.len() * bpr];
        let mut neg = vec![0u64; rows.len() * bpr];
        for (r, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), cols, "第 {r} 行长度不是 {cols}");
            pack_row(
                row,
                &mut pos[r * bpr..(r + 1) * bpr],
                &mut neg[r * bpr..(r + 1) * bpr],
            );
        }
        Self {
            rows: rows.len(),
            cols,
            bpr,
            pos,
            neg,
        }
    }

    /// 用生成函数构造（基准用）。
    pub fn from_fn(rows: usize, cols: usize, mut f: impl FnMut(usize, usize) -> Trit) -> Self {
        let bpr = cols.div_ceil(TRITS_PER_WORD).max(1);
        let mut pos = vec![0u64; rows * bpr];
        let mut neg = vec![0u64; rows * bpr];
        for r in 0..rows {
            for c in 0..cols {
                let word = r * bpr + c / TRITS_PER_WORD;
                let bit = 1u64 << (c % TRITS_PER_WORD);
                match f(r, c).get() {
                    1 => pos[word] |= bit,
                    -1 => neg[word] |= bit,
                    _ => {}
                }
            }
        }
        Self {
            rows,
            cols,
            bpr,
            pos,
            neg,
        }
    }

    /// 行数。
    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数。
    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 存储字节数。
    #[inline]
    pub fn bytes(&self) -> usize {
        (self.pos.len() + self.neg.len()) * 8
    }

    /// 每行占多少个 64 位字。
    #[inline]
    pub fn blocks_per_row(&self) -> usize {
        self.bpr
    }

    /// 单行点积：`dot(row r, x)`。`x` 需补齐到 `blocks_per_row() * 64`。
    ///
    /// 这个入口让调用方用自己的线程池按行驱动，避免再引入第二套池。
    pub fn row_dot(&self, r: usize, x: &[f32]) -> f32 {
        assert!(r < self.rows, "行号越界");
        assert!(x.len() >= self.bpr * TRITS_PER_WORD, "x 未补齐");
        let pos = &self.pos[r * self.bpr..(r + 1) * self.bpr];
        let neg = &self.neg[r * self.bpr..(r + 1) * self.bpr];
        if crate::simd::avx2() {
            // SAFETY: pos/neg 是行内切片，x 已补齐到 bpr*64。
            unsafe { crate::simd::row_dot_mask(pos, neg, x) }
        } else {
            let mut acc = 0.0f32;
            for (c, &xc) in x.iter().enumerate().take(self.cols) {
                let w = c / TRITS_PER_WORD;
                let bit = 1u64 << (c % TRITS_PER_WORD);
                if pos[w] & bit != 0 {
                    acc += xc;
                } else if neg[w] & bit != 0 {
                    acc -= xc;
                }
            }
            acc
        }
    }

    /// 取一行 trit（测试/参考用）。
    pub fn row_trits(&self, r: usize) -> Vec<Trit> {
        let mut out = Vec::with_capacity(self.cols);
        for c in 0..self.cols {
            let word = self.pos[r * self.bpr + c / TRITS_PER_WORD];
            let bit = 1u64 << (c % TRITS_PER_WORD);
            let p = word & bit != 0;
            let n = self.neg[r * self.bpr + c / TRITS_PER_WORD] & bit != 0;
            out.push(match (p, n) {
                (true, false) => Trit::POS,
                (false, true) => Trit::NEG,
                _ => Trit::ZERO,
            });
        }
        out
    }

    /// `out[r] = dot(row r, x)`。
    pub fn gemv(&self, x: &[f32], out: &mut [f32], kernel: Kernel) {
        assert_eq!(x.len(), self.cols, "激活长度必须是 cols");
        assert_eq!(out.len(), self.rows, "输出长度必须是 rows");
        match kernel {
            Kernel::Scalar => self.gemv_scalar(x, out),
            Kernel::Mask => self.gemv_mask(x, out),
            Kernel::Auto => {
                // 双导轨矩阵没有 tryte 视图，Auto 只能走掩码；
                // 要走查表请用 TryteMatrix::gemv_with(Kernel::Lut)。
                self.gemv_mask(x, out)
            }
            Kernel::Lut => panic!("Lut 内核需要 TryteMatrix"),
        }
    }

    fn gemv_scalar(&self, x: &[f32], out: &mut [f32]) {
        let cols = self.cols;
        let bpr = self.bpr;
        let pos = Ptr::new(self.pos.as_ptr() as *mut u64);
        let neg = Ptr::new(self.neg.as_ptr() as *mut u64);
        let xp = Ptr::new(x.as_ptr() as *mut f32);
        // SAFETY: out 有 rows 个元素，各线程写不同行。
        unsafe {
            par_rows_mut(out.as_mut_ptr(), self.rows, move |o, r| {
                // SAFETY: r < rows；行内只读。
                let mut acc = 0.0f32;
                for c in 0..cols {
                    let word = *pos.get().add(r * bpr + c / TRITS_PER_WORD);
                    let bit = 1u64 << (c % TRITS_PER_WORD);
                    if word & bit != 0 {
                        acc += *xp.get().add(c);
                    } else if *neg.get().add(r * bpr + c / TRITS_PER_WORD) & bit != 0 {
                        acc -= *xp.get().add(c);
                    }
                }
                *o = acc;
            });
        }
    }

    /// 掩码内核：行阻塞 2 —— 两行共享一次 `x` 装载。
    ///
    /// 每 8 trit 的装入从 3 次（1 x + 2 掩码表）降到 2.5 次（两行均摊 1 x + 4 掩码表）。
    /// 单行仍用 4 路累加器，累加顺序与 [`gemv_mask_single`](Self::gemv_mask_single)
    /// 逐位一致，所以两者输出可以直接 `assert_eq!`。
    pub(crate) fn gemv_mask(&self, x: &[f32], out: &mut [f32]) {
        // 激活补齐到 64 的倍数，掩码内核按整字读
        let padded = self.bpr * TRITS_PER_WORD;
        let mut xp = vec![0.0f32; padded];
        xp[..self.cols].copy_from_slice(x);

        if !crate::simd::avx2() {
            return self.gemv_scalar(x, out);
        }

        let bpr = self.bpr;
        let rows = self.rows;
        let pos = Ptr::new(self.pos.as_ptr() as *mut u64);
        let neg = Ptr::new(self.neg.as_ptr() as *mut u64);
        let xptr = Ptr::new(xp.as_ptr() as *mut f32);
        let pairs = rows / 2;

        // SAFETY: 第 i 个工作单元写 out[2i] / out[2i+1]，工作单元互不相交；
        // 行内切片不越界；xp 长度 >= bpr*64。
        unsafe {
            par_rows_mut(out.as_mut_ptr(), pairs, move |o, i| {
                let r = i * 2;
                let pa = std::slice::from_raw_parts(pos.get().add(r * bpr), bpr);
                let na = std::slice::from_raw_parts(neg.get().add(r * bpr), bpr);
                let pb = std::slice::from_raw_parts(pos.get().add((r + 1) * bpr), bpr);
                let nb = std::slice::from_raw_parts(neg.get().add((r + 1) * bpr), bpr);
                let xv = std::slice::from_raw_parts(xptr.get(), padded);
                let (a, b) = crate::simd::row_dot_mask2(pa, na, pb, nb, xv);
                // o 指向 out[i]，所以 o.add(i) 正好是 out[2i]。
                *o.add(i) = a;
                *o.add(i + 1) = b;
            });
        }
        // 奇数行：最后一个工作单元之外的尾巴单独算一行。
        if rows % 2 == 1 {
            let r = rows - 1;
            // SAFETY: r 在范围内；行内切片不越界。
            let v = unsafe {
                let p = std::slice::from_raw_parts(pos.get().add(r * bpr), bpr);
                let n = std::slice::from_raw_parts(neg.get().add(r * bpr), bpr);
                let xv = std::slice::from_raw_parts(xptr.get(), padded);
                crate::simd::row_dot_mask(p, n, xv)
            };
            out[r] = v;
        }
    }

    /// 单行掩码内核（行阻塞 2 的 A/B 基线）。
    pub(crate) fn gemv_mask_single(&self, x: &[f32], out: &mut [f32]) {
        // 激活补齐到 64 的倍数，掩码内核按整字读
        let padded = self.bpr * TRITS_PER_WORD;
        let mut xp = vec![0.0f32; padded];
        xp[..self.cols].copy_from_slice(x);

        let bpr = self.bpr;
        let pos = Ptr::new(self.pos.as_ptr() as *mut u64);
        let neg = Ptr::new(self.neg.as_ptr() as *mut u64);
        let xptr = Ptr::new(xp.as_ptr() as *mut f32);

        if !crate::simd::avx2() {
            return self.gemv_scalar(x, out);
        }

        // SAFETY: out 有 rows 个元素；行内切片不越界；xp 长度 >= bpr*64。
        unsafe {
            par_rows_mut(out.as_mut_ptr(), self.rows, move |o, r| {
                let p = std::slice::from_raw_parts(pos.get().add(r * bpr), bpr);
                let n = std::slice::from_raw_parts(neg.get().add(r * bpr), bpr);
                let xv = std::slice::from_raw_parts(xptr.get(), padded);
                *o = crate::simd::row_dot_mask(p, n, xv);
            });
        }
    }
}

/// tryte 打包的三值矩阵（每字节 5 个 trit）。
#[derive(Clone, Debug)]
pub struct TryteMatrix {
    rows: usize,
    cols: usize,
    gpr: usize,
    data: Vec<u8>,
}

impl TryteMatrix {
    /// 从按行给出的 trit 构造。
    pub fn from_rows(rows: &[Vec<Trit>], cols: usize) -> Self {
        let gpr = cols.div_ceil(TRITS_PER_BYTE).max(1);
        let mut data = vec![0u8; rows.len() * gpr];
        for (r, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), cols);
            for (g, chunk) in row.chunks(TRITS_PER_BYTE).enumerate() {
                let mut code = 0usize;
                let mut pow = 1usize;
                for t in chunk {
                    code += (t.get() as i32 + 1) as usize * pow;
                    pow *= 3;
                }
                data[r * gpr + g] = code as u8;
            }
        }
        Self {
            rows: rows.len(),
            cols,
            gpr,
            data,
        }
    }

    /// 从双导轨矩阵转过来（基准用）。
    pub fn from_rail(m: &TernaryMatrix) -> Self {
        let rows: Vec<Vec<Trit>> = (0..m.rows()).map(|r| m.row_trits(r)).collect();
        Self::from_rows(&rows, m.cols())
    }

    /// 行数。
    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数。
    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 存储字节数。
    #[inline]
    pub fn bytes(&self) -> usize {
        self.data.len()
    }

    /// `out[r] = dot(row r, x)`；`kernel` 只接受 [`Kernel::Lut`] 与 [`Kernel::Auto`]。
    pub fn gemv_with(&self, x: &[f32], out: &mut [f32], kernel: Kernel) {
        match kernel {
            Kernel::Lut | Kernel::Auto => self.gemv(x, out),
            _ => panic!("TryteMatrix 只支持 Lut/Auto"),
        }
    }

    /// 建表：`gpr × 243` 项，`tables[g*243 + code]` = 第 `g` 组 5 个 trit 与 `x` 的加权和。
    ///
    /// 表只依赖激活 `x`，与行数无关——同一 `x` 上多次 gemv 可复用。
    /// 建表成本 `O(gpr × 243)` 与行数无关，所以行数少时它占的比例很高。
    pub fn build_tables(&self, x: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), self.cols, "激活长度必须是 cols");
        let gpr = self.gpr;
        let cols = self.cols;

        // 建表：gpr 张 × 243 项。串行建表在小行数时会成为瓶颈，按组并行。
        let mut tables = vec![0.0f32; gpr * TRYTE_STATES];
        {
            let xp = Ptr::new(x.as_ptr() as *mut f32);
            let tp = Ptr::new(tables.as_mut_ptr());
            let mut units = vec![0u8; gpr];
            // SAFETY: units 有 gpr 个元素；每个组只写自己那张 243 项的表。
            unsafe {
                par_rows_mut(units.as_mut_ptr(), gpr, move |_, g| {
                    let mut xv = [0.0f32; TRITS_PER_BYTE];
                    for (i, slot) in xv.iter_mut().enumerate() {
                        let c = g * TRITS_PER_BYTE + i;
                        if c < cols {
                            *slot = *xp.get().add(c);
                        }
                    }
                    let t = std::slice::from_raw_parts_mut(
                        tp.get().add(g * TRYTE_STATES),
                        TRYTE_STATES,
                    );
                    t[0] = -xv.iter().sum::<f32>();
                    let mut pow = 1usize;
                    for &xi in &xv {
                        for c in 0..pow {
                            let base = t[c];
                            t[c + pow] = base + xi;
                            t[c + 2 * pow] = base + 2.0 * xi;
                        }
                        pow *= 3;
                    }
                });
            }
        }
        tables
    }

    /// 用已建好的表算 `out[r] = dot(row r, x)`。
    ///
    /// `tables` 必须来自同一 `x` 上的 [`build_tables`](Self::build_tables)。
    pub fn gemv_with_tables(&self, tables: &[f32], out: &mut [f32]) {
        assert_eq!(tables.len(), self.gpr * TRYTE_STATES, "表尺寸不符");
        assert_eq!(out.len(), self.rows, "输出长度必须是 rows");
        let gpr = self.gpr;

        let tab = Ptr::new(tables.as_ptr() as *mut f32);
        let data = Ptr::new(self.data.as_ptr() as *mut u8);
        // SAFETY: out 有 rows 个元素；codes 行内不越界；表已完整建立。
        unsafe {
            par_rows_mut(out.as_mut_ptr(), self.rows, move |o, r| {
                let codes = std::slice::from_raw_parts(data.get().add(r * gpr), gpr);
                let mut acc = 0.0f32;
                for (g, &code) in codes.iter().enumerate() {
                    acc += *tab.get().add(g * TRYTE_STATES + code as usize);
                }
                *o = acc;
            });
        }
    }

    /// `out[r] = dot(row r, x)`，用 `3^5` 查表。
    ///
    /// 每个 5-trit 组对应一张 243 项的表：表项就是该组 5 个 trit 的加权和。
    /// 表在每次 gemv 时重建一次（激活变了），按行复用到所有行。
    pub fn gemv(&self, x: &[f32], out: &mut [f32]) {
        assert_eq!(out.len(), self.rows, "输出长度必须是 rows");
        let tables = self.build_tables(x);
        self.gemv_with_tables(&tables, out);
    }

    /// 建拆表：每字节两张小表（低 2 位 9 项 + 高 3 位 27 项），共 36 项。
    ///
    /// 建表成本只有整字节表的 `36/243`，小行数时能显著压低固定开销。
    pub fn build_tables_split(&self, x: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), self.cols, "激活长度必须是 cols");
        let gpr = self.gpr;
        let cols = self.cols;
        let mut tables = vec![0.0f32; gpr * SPLIT_STATES];
        let xp = Ptr::new(x.as_ptr() as *mut f32);
        let tp = Ptr::new(tables.as_mut_ptr());
        let mut units = vec![0u8; gpr];
        // SAFETY: units 有 gpr 个元素；每组只写自己那 36 项。
        unsafe {
            par_rows_mut(units.as_mut_ptr(), gpr, move |_, g| {
                let mut xv = [0.0f32; TRITS_PER_BYTE];
                for (i, slot) in xv.iter_mut().enumerate() {
                    let c = g * TRITS_PER_BYTE + i;
                    if c < cols {
                        *slot = *xp.get().add(c);
                    }
                }
                let base = tp.get().add(g * SPLIT_STATES);
                build_pow3(base, &xv[..2]);
                build_pow3(base.add(SPLIT_LOW_STATES), &xv[2..]);
            });
        }
        tables
    }

    /// 用拆表算 `out[r] = dot(row r, x)`；`tables` 须来自同一 `x` 的 [`build_tables_split`](Self::build_tables_split)。
    pub fn gemv_with_split_tables(&self, tables: &[f32], out: &mut [f32]) {
        assert_eq!(tables.len(), self.gpr * SPLIT_STATES, "拆表尺寸不符");
        assert_eq!(out.len(), self.rows, "输出长度必须是 rows");
        let gpr = self.gpr;
        let tab = Ptr::new(tables.as_ptr() as *mut f32);
        let data = Ptr::new(self.data.as_ptr() as *mut u8);
        // SAFETY: out 有 rows 个元素；codes 行内不越界；表已完整建立。
        unsafe {
            par_rows_mut(out.as_mut_ptr(), self.rows, move |o, r| {
                let codes = std::slice::from_raw_parts(data.get().add(r * gpr), gpr);
                let mut acc = 0.0f32;
                for (g, &code) in codes.iter().enumerate() {
                    let base = tab.get().add(g * SPLIT_STATES);
                    let c = code as usize;
                    acc += *base.add(c % SPLIT_LOW_STATES);
                    acc += *base.add(SPLIT_LOW_STATES + c / SPLIT_LOW_STATES);
                }
                *o = acc;
            });
        }
    }

    /// 同 [`gemv_with_split_tables`](Self::gemv_with_split_tables)，偏移改走预计算的 243 项映射表。
    pub fn gemv_with_split_tables_map(&self, tables: &[f32], out: &mut [f32]) {
        assert_eq!(tables.len(), self.gpr * SPLIT_STATES, "拆表尺寸不符");
        assert_eq!(out.len(), self.rows, "输出长度必须是 rows");
        let gpr = self.gpr;
        let tab = Ptr::new(tables.as_ptr() as *mut f32);
        let data = Ptr::new(self.data.as_ptr() as *mut u8);
        // SAFETY: out 有 rows 个元素；codes 行内不越界；表已完整建立。
        unsafe {
            par_rows_mut(out.as_mut_ptr(), self.rows, move |o, r| {
                let codes = std::slice::from_raw_parts(data.get().add(r * gpr), gpr);
                let mut acc = 0.0f32;
                for (g, &code) in codes.iter().enumerate() {
                    let base = tab.get().add(g * SPLIT_STATES);
                    // SAFETY: code 是 u8，SPLIT_MAP 覆盖 0..=242；243..=255 只在
                    // 构造期被拒绝，这里仍用 unchecked 省掉边界检查（越界是 UB 的
                    // 前提是 code > 242，TryteMatrix 的构造函数保证不会出现）。
                    let off = *SPLIT_MAP.get_unchecked(code as usize) as usize;
                    acc += *base.add(off & 0xFF);
                    acc += *base.add(SPLIT_LOW_STATES + (off >> 8));
                }
                *o = acc;
            });
        }
    }

    /// 建拆表 + 映射查表。
    pub fn gemv_split_map(&self, x: &[f32], out: &mut [f32]) {
        let tables = self.build_tables_split(x);
        self.gemv_with_split_tables_map(&tables, out);
    }

    /// 按行数自动选表粒度：小行数用拆表（省建表），大行数用整字节表（省查表）。
    pub fn gemv_auto(&self, x: &[f32], out: &mut [f32]) {
        if self.rows < LUT_SPLIT_ROWS {
            let tables = self.build_tables_split(x);
            self.gemv_with_split_tables_map(&tables, out);
        } else {
            self.gemv(x, out);
        }
    }

    /// 强制走拆表，绕过 [`LUT_SPLIT_ROWS`]（诊断/基准用）。
    pub fn gemv_auto_split(&self, x: &[f32], out: &mut [f32]) {
        let tables = self.build_tables_split(x);
        self.gemv_with_split_tables_map(&tables, out);
    }
}

/// 把 `xs` 的 `3^len` 种加权和写进 `t`（`t[0]` 对应全 `-1` 组合）。
///
/// code 用 `0/1/2` 表示 trit `-1/0/+1`，所以全零组合的值是 `-sum(xs)`；
/// 之后每加一个 trit，把已有表扩成「原值 / +x / +2x」三份。
///
/// # Safety
/// `t` 必须指向至少 `3^xs.len()` 个连续 `f32`。
unsafe fn build_pow3(t: *mut f32, xs: &[f32]) {
    *t = -xs.iter().sum::<f32>();
    let mut pow = 1usize;
    for &xi in xs {
        for c in 0..pow {
            let base = *t.add(c);
            *t.add(c + pow) = base + xi;
            *t.add(c + 2 * pow) = base + 2.0 * xi;
        }
        pow *= 3;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(rows: usize, cols: usize) -> Vec<Vec<Trit>> {
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| Trit::from_i8_unchecked(((r * 31 + c * 17 + 5) % 3) as i8 - 1))
                    .collect()
            })
            .collect()
    }

    fn reference(rows: &[Vec<Trit>], x: &[f32]) -> Vec<f32> {
        rows.iter()
            .map(|row| row.iter().zip(x).map(|(t, v)| t.get() as f32 * v).sum())
            .collect()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| (x - y).abs() <= 1e-3 * (1.0 + y.abs()))
    }

    #[test]
    fn split_map_matches_divmod_bitwise() {
        let (rows, cols) = (37usize, 83usize);
        let m = TryteMatrix::from_rows(&make(rows, cols), cols);
        let x: Vec<f32> = (0..cols).map(|i| ((i % 13) as f32 - 6.0) * 0.1).collect();
        let tables = m.build_tables_split(&x);
        let mut a = vec![0.0f32; rows];
        let mut b = vec![0.0f32; rows];
        m.gemv_with_split_tables(&tables, &mut a);
        m.gemv_with_split_tables_map(&tables, &mut b);
        assert_eq!(a, b, "映射表与 %9//9 必须逐位一致");
    }

    #[test]
    fn scalar_matches_reference() {
        let (rows, cols) = (37usize, 130usize);
        let m = TernaryMatrix::from_rows(&make(rows, cols), cols);
        let x: Vec<f32> = (0..cols).map(|i| (i % 13) as f32 * 0.1 - 0.5).collect();
        let mut out = vec![0.0f32; rows];
        m.gemv(&x, &mut out, Kernel::Scalar);
        assert!(close(&out, &reference(&make(rows, cols), &x)));
    }

    #[test]
    fn mask_pair_matches_single_bitwise() {
        // 奇数行：覆盖「成对 + 尾行」两条路径。
        for rows in [1usize, 2, 7, 37] {
            let cols = 130usize;
            let m = TernaryMatrix::from_rows(&make(rows, cols), cols);
            let x: Vec<f32> = (0..cols).map(|i| (i % 17) as f32 * 0.07 - 0.4).collect();
            let mut single = vec![0.0f32; rows];
            let mut pair = vec![0.0f32; rows];
            m.gemv_mask_single(&x, &mut single);
            m.gemv_mask(&x, &mut pair);
            assert_eq!(pair, single, "rows={rows}：两行版与单行版必须逐位一致");
        }
    }

    #[test]
    fn mask_single_row_debug() {
        let cols = 64usize;
        let mut row = vec![Trit::ZERO; cols];
        row[0] = Trit::POS;
        row[1] = Trit::NEG;
        row[63] = Trit::POS;
        let m = TernaryMatrix::from_rows(&[row], cols);
        let x: Vec<f32> = (0..cols).map(|i| i as f32).collect();
        let mut a = vec![0.0f32; 1];
        let mut b = vec![0.0f32; 1];
        m.gemv(&x, &mut a, Kernel::Scalar);
        m.gemv(&x, &mut b, Kernel::Mask);
        println!("scalar={} mask={}（期望 0-1+63=62）", a[0], b[0]);
        assert_eq!(a[0], b[0]);
    }

    #[test]
    fn mask_matches_scalar() {
        let (rows, cols) = (37usize, 130usize);
        let rows_v = make(rows, cols);
        let m = TernaryMatrix::from_rows(&rows_v, cols);
        let x: Vec<f32> = (0..cols).map(|i| (i % 7) as f32 * 0.25 - 0.7).collect();
        let mut a = vec![0.0f32; rows];
        let mut b = vec![0.0f32; rows];
        m.gemv(&x, &mut a, Kernel::Scalar);
        m.gemv(&x, &mut b, Kernel::Mask);
        assert!(close(&a, &b), "mask 与 scalar 不一致");
    }

    #[test]
    fn lut_matches_scalar() {
        let (rows, cols) = (37usize, 130usize);
        let rows_v = make(rows, cols);
        let m = TernaryMatrix::from_rows(&rows_v, cols);
        let t = TryteMatrix::from_rail(&m);
        let x: Vec<f32> = (0..cols).map(|i| (i % 11) as f32 * 0.13 - 0.6).collect();
        let mut a = vec![0.0f32; rows];
        let mut b = vec![0.0f32; rows];
        m.gemv(&x, &mut a, Kernel::Scalar);
        t.gemv(&x, &mut b);
        assert!(close(&a, &b), "lut 与 scalar 不一致");
    }

    #[test]
    fn lut_handles_cols_not_multiple_of_five() {
        for cols in [1usize, 4, 5, 6, 13, 131] {
            let rows = 9usize;
            let rows_v = make(rows, cols);
            let m = TernaryMatrix::from_rows(&rows_v, cols);
            let t = TryteMatrix::from_rail(&m);
            let x: Vec<f32> = (0..cols).map(|i| (i % 5) as f32 * 0.3 - 0.5).collect();
            let mut a = vec![0.0f32; rows];
            let mut b = vec![0.0f32; rows];
            m.gemv(&x, &mut a, Kernel::Scalar);
            t.gemv(&x, &mut b);
            assert!(close(&a, &b), "cols={cols}");
        }
    }

    #[test]
    fn lut_split_matches_whole_byte_table() {
        // 拆表（2+3）必须与整字节表（5）给出同一结果，含 cols 不是 5 的倍数。
        for cols in [1usize, 4, 5, 6, 13, 130, 131] {
            let rows = 37usize;
            let rows_v = make(rows, cols);
            let m = TernaryMatrix::from_rows(&rows_v, cols);
            let t = TryteMatrix::from_rail(&m);
            let x: Vec<f32> = (0..cols).map(|i| (i % 11) as f32 * 0.13 - 0.6).collect();
            let mut a = vec![0.0f32; rows];
            let mut b = vec![0.0f32; rows];
            t.gemv(&x, &mut a);
            let tables = t.build_tables_split(&x);
            t.gemv_with_split_tables(&tables, &mut b);
            assert!(close(&a, &b), "拆表与整字节表不一致，cols={cols}");
        }
    }

    #[test]
    fn lut_auto_uses_split_below_threshold() {
        // 行数低于阈值时 gemv_auto 走拆表；结果必须与整字节表一致。
        let (rows, cols) = (13usize, 130usize);
        assert!(rows < LUT_SPLIT_ROWS, "本用例依赖小行数走拆表");
        let rows_v = make(rows, cols);
        let m = TernaryMatrix::from_rows(&rows_v, cols);
        let t = TryteMatrix::from_rail(&m);
        let x: Vec<f32> = (0..cols).map(|i| (i % 7) as f32 * 0.21 - 0.7).collect();
        let mut a = vec![0.0f32; rows];
        let mut b = vec![0.0f32; rows];
        t.gemv(&x, &mut a);
        t.gemv_auto(&x, &mut b);
        assert!(close(&a, &b), "gemv_auto 与整字节表不一致");
    }

    #[test]
    fn row_dot_matches_gemv() {
        let (rows, cols) = (13usize, 130usize);
        let m = TernaryMatrix::from_rows(&make(rows, cols), cols);
        let x: Vec<f32> = (0..cols).map(|i| (i % 9) as f32 * 0.2 - 0.8).collect();
        let mut padded = vec![0.0f32; m.blocks_per_row() * 64];
        padded[..cols].copy_from_slice(&x);
        let mut out = vec![0.0f32; rows];
        m.gemv(&x, &mut out, Kernel::Mask);
        for (r, &o) in out.iter().enumerate() {
            assert!((m.row_dot(r, &padded) - o).abs() < 1e-4, "第 {r} 行");
        }
    }

    #[test]
    fn zero_row_gives_zero() {
        let cols = 70usize;
        let rows_v = vec![vec![Trit::ZERO; cols]; 3];
        let m = TernaryMatrix::from_rows(&rows_v, cols);
        let x = vec![1.5f32; cols];
        let mut out = vec![9.0f32; 3];
        m.gemv(&x, &mut out, Kernel::Mask);
        assert!(out.iter().all(|v| v.abs() < 1e-6));
    }
}
