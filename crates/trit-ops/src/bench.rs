//! 基准：同一形状下比较 f32 / f16 / Q4_0 / 三值三条内核。
//!
//! 所有实现共用同一个自旋池与同一份激活，避免「换个线程模型就赢」的假结论。

use crate::gemv::{Kernel, TernaryMatrix, TryteMatrix};
use crate::pool::Ptr;
use crate::simd;
use crate::trit::Trit;
use std::time::Instant;

/// 一次测量的结果。
#[derive(Clone, Debug)]
pub struct Measurement {
    /// 内核名。
    pub name: String,
    /// 每次 gemv 的纳秒数。
    pub ns: f64,
    /// 每秒多少百万次乘加。
    pub gmac_s: f64,
    /// 有效带宽（GB/s）。
    pub gbs: f64,
    /// 权重每 trit/元素的字节数。
    pub bytes_per_weight: f64,
    /// 与标量参考的最大绝对误差（正确性证据）。
    pub max_err: f32,
}

/// f32 权重矩阵向量乘。
pub fn gemv_f32(w: &[f32], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    assert_eq!(out.len(), rows, "输出长度必须是 rows");
    let wp = Ptr::new(w.as_ptr() as *mut f32);
    let xp = Ptr::new(x.as_ptr() as *mut f32);
    par_rows(out, move |o, r| {
        // SAFETY: r < rows，行内只读。
        unsafe {
            let row = std::slice::from_raw_parts(wp.get().add(r * cols), cols);
            let xv = std::slice::from_raw_parts(xp.get(), cols);
            *o = simd::row_dot_f32(row, xv);
        }
    });
}

/// f16 权重矩阵向量乘。
pub fn gemv_f16(w: &[u16], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    assert_eq!(out.len(), rows, "输出长度必须是 rows");
    let wp = Ptr::new(w.as_ptr() as *mut u16);
    let xp = Ptr::new(x.as_ptr() as *mut f32);
    par_rows(out, move |o, r| {
        // SAFETY: r < rows，行内只读。
        unsafe {
            let row = std::slice::from_raw_parts(wp.get().add(r * cols), cols);
            let xv = std::slice::from_raw_parts(xp.get(), cols);
            *o = simd::row_dot_f16(row, xv);
        }
    });
}

/// Q4_0 权重矩阵向量乘（每 32 元素 18 字节）。
pub fn gemv_q4(w: &[u8], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    assert_eq!(out.len(), rows, "输出长度必须是 rows");
    let bpr = cols.div_ceil(32) * 18;
    let (qx, dx) = simd::quantize_act_q8(x);
    let wp = Ptr::new(w.as_ptr() as *mut u8);
    let qp = Ptr::new(qx.as_ptr() as *mut i8);
    let dp = Ptr::new(dx.as_ptr() as *mut f32);
    par_rows(out, move |o, r| {
        // SAFETY: 行切片不越界；qx/dx 覆盖整行。
        unsafe {
            let row = std::slice::from_raw_parts(wp.get().add(r * bpr), bpr);
            let qv = std::slice::from_raw_parts(qp.get(), cols);
            let dv = std::slice::from_raw_parts(dp.get(), cols.div_ceil(32));
            *o = simd::row_dot_q4(row, qv, dv);
        }
    });
}

fn par_rows<F>(out: &mut [f32], f: F)
where
    F: Fn(*mut f32, usize) + Send + Sync + 'static,
{
    // SAFETY: out 有 out.len() 个元素，各线程写不同行。
    unsafe { crate::pool::par_rows_mut(out.as_mut_ptr(), out.len(), f) };
}

/// 把 f32 权重按 Q4_0 打包（每块 32：f16 scale + 16 字节 nibble）。
pub fn pack_q4(w: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w.len() / 32 * 18);
    for blk in w.as_chunks::<32>().0 {
        let amax = blk.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let d = if amax > 0.0 { amax / 7.0 } else { 0.0 };
        out.extend_from_slice(&to_f16(d).to_le_bytes());
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            let q0 = if d > 0.0 {
                (blk[i] / d).round().clamp(-8.0, 7.0) as i32 + 8
            } else {
                8
            };
            let q1 = if d > 0.0 {
                (blk[i + 16] / d).round().clamp(-8.0, 7.0) as i32 + 8
            } else {
                8
            };
            bytes[i] = ((q0 as u8) & 0x0f) | (((q1 as u8) & 0x0f) << 4);
        }
        out.extend_from_slice(&bytes);
    }
    out
}

/// f32 -> f16 位模式。
pub fn to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let mant = ((b >> 13) & 0x3ff) as u16;
    if e <= 0 {
        sign
    } else if e >= 31 {
        sign | 0x7c00
    } else {
        sign | ((e as u16) << 10) | mant
    }
}

/// 确定性伪随机三值权重。
pub fn ternary_weights(rows: usize, cols: usize, seed: u64) -> Vec<Vec<Trit>> {
    let mut state = seed | 1;
    (0..rows)
        .map(|_| {
            (0..cols)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    // 近似三值高斯：三档等概率
                    Trit::from_i8_unchecked((state % 3) as i8 - 1)
                })
                .collect()
        })
        .collect()
}

/// 预处理好的基准夹具：权重、激活、参考结果、各精度表示。
///
/// 多轮测量时只建一次，避免把夹具构建时间算进任何一轮。
struct Prepared {
    rows: usize,
    cols: usize,
    x: Vec<f32>,
    reference: Vec<f32>,
    f32_w: Vec<f32>,
    f16_w: Vec<u16>,
    q4_w: Vec<u8>,
    rail: TernaryMatrix,
    trytes: TryteMatrix,
}

fn prepare(rows: usize, cols: usize) -> Prepared {
    let trits = ternary_weights(rows, cols, 0x9E37_79B9_7F4A_7C15);
    let x: Vec<f32> = (0..cols)
        .map(|i| ((i % 29) as f32 - 14.0) * 0.031)
        .collect();

    // 参考结果（标量）
    let mut reference = vec![0.0f32; rows];
    for (r, row) in trits.iter().enumerate() {
        reference[r] = row.iter().zip(&x).map(|(t, v)| t.get() as f32 * v).sum();
    }

    // 权重按各精度展开
    let f32_w: Vec<f32> = trits.iter().flatten().map(|t| t.get() as f32).collect();
    let f16_w: Vec<u16> = f32_w.iter().map(|v| to_f16(*v)).collect();
    let q4_w = pack_q4(&f32_w);

    let rail = TernaryMatrix::from_rows(&trits, cols);
    let trytes = TryteMatrix::from_rows(&trits, cols);

    Prepared {
        rows,
        cols,
        x,
        reference,
        f32_w,
        f16_w,
        q4_w,
        rail,
        trytes,
    }
}

/// 把全部内核各测一遍（一个「轮次」）。
fn run_all(p: &Prepared, iters: usize) -> Vec<Measurement> {
    let rows = p.rows;
    let cols = p.cols;
    let mut results = Vec::new();
    let out = vec![0.0f32; rows];

    // 每个内核：先跑一次校验，再计时
    let mut check =
        |name: &str, bytes_per_weight: f64, out: &mut [f32], run: &mut dyn FnMut(&mut [f32])| {
            run(out);
            let max_err = out
                .iter()
                .zip(&p.reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            let t = Instant::now();
            for _ in 0..iters {
                run(out);
            }
            let ns = t.elapsed().as_secs_f64() * 1e9 / iters as f64;
            let macs = (rows * cols) as f64;
            results.push(Measurement {
                name: name.to_string(),
                ns,
                gmac_s: macs / ns,
                gbs: macs * bytes_per_weight / ns,
                bytes_per_weight,
                max_err,
            });
        };

    check("f32  (4 B)", 4.0, &mut out.clone(), &mut |o| {
        gemv_f32(&p.f32_w, rows, cols, &p.x, o)
    });
    check("f16  (2 B)", 2.0, &mut out.clone(), &mut |o| {
        gemv_f16(&p.f16_w, rows, cols, &p.x, o)
    });
    check("Q4_0 (0.5 B)", 0.5, &mut out.clone(), &mut |o| {
        gemv_q4(&p.q4_w, rows, cols, &p.x, o)
    });
    check(
        "trit 双导轨标量 (0.25 B)",
        0.25,
        &mut out.clone(),
        &mut |o| p.rail.gemv(&p.x, o, Kernel::Scalar),
    );
    check(
        "trit 双导轨掩码 (0.25 B)",
        0.25,
        &mut out.clone(),
        &mut |o| p.rail.gemv(&p.x, o, Kernel::Mask),
    );
    check(
        "trit tryte 查表 (0.2 B)",
        0.2,
        &mut out.clone(),
        &mut |o| p.trytes.gemv_auto(&p.x, o),
    );
    check("trit 自动选核", 0.25, &mut out.clone(), &mut |o| {
        if rows >= crate::gemv::AUTO_LUT_ROWS {
            p.trytes.gemv(&p.x, o)
        } else {
            p.rail.gemv(&p.x, o, Kernel::Mask)
        }
    });

    results
}

/// 在同一形状上跑完全部内核（单轮）。
pub fn measure(rows: usize, cols: usize, iters: usize) -> Vec<Measurement> {
    let p = prepare(rows, cols);
    run_all(&p, iters)
}

/// 多轮轮转测量：每轮把所有内核各测一遍，轮间交替。
///
/// **为什么轮转**：顺序测量时，先测的内核拿到的机器状态和后测的不同——
/// 实测同一段代码换个位置读数能差约 25%。轮转让每个内核都经历全部位置，
/// 取各轮中位数后，位置偏差被摊平。
///
/// `on_round` 在每轮结束后立即回调，长跑时可以流式打印进度。
pub fn measure_rounds(
    rows: usize,
    cols: usize,
    iters: usize,
    rounds: usize,
    mut on_round: impl FnMut(usize, &[Measurement]),
) -> Vec<Measurement> {
    let p = prepare(rows, cols);
    let rounds = rounds.max(1);
    let mut per_round: Vec<Vec<Measurement>> = Vec::with_capacity(rounds);
    for r in 0..rounds {
        let ms = run_all(&p, iters);
        on_round(r, &ms);
        per_round.push(ms);
    }
    median_per_kernel(&per_round, (rows * cols) as f64)
}

/// 对每个内核取各轮 `ns` 的中位数，并据此重算 GMAC/s 与带宽。
fn median_per_kernel(rounds: &[Vec<Measurement>], macs: f64) -> Vec<Measurement> {
    let mut out = Vec::new();
    let Some(first) = rounds.first() else {
        return out;
    };
    for k in 0..first.len() {
        let mut ns: Vec<f64> = rounds.iter().map(|r| r[k].ns).collect();
        ns.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
        let med = ns[ns.len() / 2];
        let max_err = rounds.iter().map(|r| r[k].max_err).fold(0.0f32, f32::max);
        out.push(Measurement {
            name: first[k].name.clone(),
            ns: med,
            gmac_s: macs / med,
            gbs: macs * first[k].bytes_per_weight / med,
            bytes_per_weight: first[k].bytes_per_weight,
            max_err,
        });
    }
    out
}

/// 某内核各轮 GMAC/s 的最小值与最大值，用来报告波动幅度。
pub fn round_spread(rounds: &[Vec<Measurement>], k: usize) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for r in rounds {
        lo = lo.min(r[k].gmac_s);
        hi = hi.max(r[k].gmac_s);
    }
    (lo, hi)
}

/// tryte 两种表粒度的成本分解（单位 ns/次 gemv）。
pub struct SplitCost {
    /// 整字节表（243 项）建表。
    pub build_whole: f64,
    /// 整字节表查表。
    pub lookup_whole: f64,
    /// 拆表（36 项）建表。
    pub build_split: f64,
    /// 拆表查表（`% 9` / `/ 9`）。
    pub lookup_split: f64,
    /// 拆表查表（预计算映射）。
    pub lookup_split_map: f64,
}

/// 把 tryte 的一次 gemv 拆成「建表」和「查表」两段分别计时，两种表粒度都测。
///
/// **建表用差分法**：`全量 gemv − 只查表 gemv`。直接计时建表会被编译器优化掉
/// （表写进去没人读，死存储消除），测出来只有真实值的一半以下。
///
/// 建表与行数无关，查表与行数成正比——两者的比值就是「小行数为什么亏」的直接答案，
/// 也是 [`crate::gemv::LUT_SPLIT_ROWS`] 阈值的依据。
pub fn tryte_split(rows: usize, cols: usize, iters: usize) -> SplitCost {
    let trits = ternary_weights(rows, cols, 0x9E37_79B9_7F4A_7C15);
    let x: Vec<f32> = (0..cols)
        .map(|i| ((i % 29) as f32 - 14.0) * 0.031)
        .collect();
    let trytes = TryteMatrix::from_rows(&trits, cols);
    let mut out = vec![0.0f32; rows];

    // 整字节：全量
    let t = Instant::now();
    for _ in 0..iters {
        trytes.gemv(&x, &mut out);
    }
    let full_whole = t.elapsed().as_secs_f64() * 1e9 / iters as f64;

    // 整字节：只查表
    let tables = trytes.build_tables(&x);
    let t = Instant::now();
    for _ in 0..iters {
        trytes.gemv_with_tables(&tables, &mut out);
    }
    let lookup_whole = t.elapsed().as_secs_f64() * 1e9 / iters as f64;

    // 拆表：全量
    let t = Instant::now();
    for _ in 0..iters {
        trytes.gemv_auto_split(&x, &mut out);
    }
    let full_split = t.elapsed().as_secs_f64() * 1e9 / iters as f64;

    // 拆表：只查表
    let tables = trytes.build_tables_split(&x);
    let t = Instant::now();
    for _ in 0..iters {
        trytes.gemv_with_split_tables(&tables, &mut out);
    }
    let lookup_split = t.elapsed().as_secs_f64() * 1e9 / iters as f64;

    // 拆表：只查表（映射）
    let t = Instant::now();
    for _ in 0..iters {
        trytes.gemv_with_split_tables_map(&tables, &mut out);
    }
    let lookup_split_map = t.elapsed().as_secs_f64() * 1e9 / iters as f64;

    SplitCost {
        build_whole: (full_whole - lookup_whole).max(0.0),
        lookup_whole,
        build_split: (full_split - lookup_split_map).max(0.0),
        lookup_split,
        lookup_split_map,
    }
}

/// 拆表查表的两种实现逐轮配对：`% 9` / `/ 9` vs 预计算 243 项映射。
///
/// 每轮交替先后顺序（奇数轮 A→B，偶数轮 B→A），把顺序偏差摊到两边。
/// 返回 `(除法 ns, 映射 ns)`，均为单次 gemv。
pub fn tryte_split_lookup(
    rows: usize,
    cols: usize,
    iters: usize,
    rounds: usize,
) -> (Vec<f64>, Vec<f64>) {
    let p = prepare(rows, cols);
    let tables = p.trytes.build_tables_split(&p.x);
    let mut out = vec![0.0f32; rows];

    // 先验证两种写法逐位一致；不一致时后面所有读数都没意义。
    let mut o_div = vec![0.0f32; rows];
    let mut o_map = vec![0.0f32; rows];
    p.trytes.gemv_with_split_tables(&tables, &mut o_div);
    p.trytes.gemv_with_split_tables_map(&tables, &mut o_map);
    assert_eq!(o_div, o_map, "映射表与 %9//9 结果必须逐位一致");

    let time_one = |iters: usize, out: &mut [f32], f: &mut dyn FnMut(&mut [f32])| {
        let t = Instant::now();
        for _ in 0..iters {
            f(out);
        }
        t.elapsed().as_secs_f64() * 1e9 / iters as f64
    };

    let rounds = rounds.max(1);
    let mut divmod_ns = Vec::with_capacity(rounds);
    let mut map_ns = Vec::with_capacity(rounds);
    for r in 0..rounds {
        let mut divmod = |out: &mut [f32]| p.trytes.gemv_with_split_tables(&tables, out);
        let mut map = |out: &mut [f32]| p.trytes.gemv_with_split_tables_map(&tables, out);
        let (a, b) = if r % 2 == 0 {
            (
                time_one(iters, &mut out, &mut divmod),
                time_one(iters, &mut out, &mut map),
            )
        } else {
            let b = time_one(iters, &mut out, &mut map);
            let a = time_one(iters, &mut out, &mut divmod);
            (a, b)
        };
        divmod_ns.push(a);
        map_ns.push(b);
    }
    (divmod_ns, map_ns)
}

/// 掩码内核行阻塞的配对测量：单行 vs 两行共享 `x`。
///
/// 每轮交替先后顺序；先断言两者输出逐位一致，否则读数没有意义。
pub fn mask_pair(rows: usize, cols: usize, iters: usize, rounds: usize) -> (Vec<f64>, Vec<f64>) {
    let p = prepare(rows, cols);
    let mut out = vec![0.0f32; rows];
    let mut single_out = vec![0.0f32; rows];
    let mut pair_out = vec![0.0f32; rows];
    p.rail.gemv_mask_single(&p.x, &mut single_out);
    p.rail.gemv_mask(&p.x, &mut pair_out);
    assert_eq!(single_out, pair_out, "两行共享 x 必须与单行逐位一致");

    let time_one = |iters: usize, out: &mut [f32], f: &mut dyn FnMut(&mut [f32])| {
        let t = Instant::now();
        for _ in 0..iters {
            f(out);
        }
        t.elapsed().as_secs_f64() * 1e9 / iters as f64
    };

    let rounds = rounds.max(1);
    let mut single_ns = Vec::with_capacity(rounds);
    let mut pair_ns = Vec::with_capacity(rounds);
    for r in 0..rounds {
        let mut single = |out: &mut [f32]| p.rail.gemv_mask_single(&p.x, out);
        let mut pair = |out: &mut [f32]| p.rail.gemv_mask(&p.x, out);
        let (a, b) = if r % 2 == 0 {
            (
                time_one(iters, &mut out, &mut single),
                time_one(iters, &mut out, &mut pair),
            )
        } else {
            let b = time_one(iters, &mut out, &mut pair);
            let a = time_one(iters, &mut out, &mut single);
            (a, b)
        };
        single_ns.push(a);
        pair_ns.push(b);
    }
    (single_ns, pair_ns)
}

/// 把结果排成表。
pub fn format_table(results: &[Measurement]) -> String {
    let base = results.first().map(|m| m.ns).unwrap_or(1.0);
    let mut s = String::new();
    s.push_str(&format!(
        "{:<26} {:>10} {:>10} {:>9} {:>8} {:>9}\n",
        "内核", "ns/gemv", "GMAC/s", "GB/s", "相对 f32", "max_err"
    ));
    for m in results {
        s.push_str(&format!(
            "{:<26} {:>10.1} {:>10.2} {:>9.2} {:>7.2}x {:>9.1e}\n",
            m.name,
            m.ns,
            m.gmac_s,
            m.gbs,
            base / m.ns,
            m.max_err
        ));
    }
    s
}
