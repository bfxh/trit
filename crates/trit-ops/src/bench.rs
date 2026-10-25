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

/// 在同一形状上跑完全部内核。
pub fn measure(rows: usize, cols: usize, iters: usize) -> Vec<Measurement> {
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

    let mut results = Vec::new();
    let out = vec![0.0f32; rows];

    // 每个内核：先跑一次校验，再计时
    let mut check =
        |name: &str, bytes_per_weight: f64, out: &mut [f32], run: &mut dyn FnMut(&mut [f32])| {
            run(out);
            let max_err = out
                .iter()
                .zip(&reference)
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
        gemv_f32(&f32_w, rows, cols, &x, o)
    });
    check("f16  (2 B)", 2.0, &mut out.clone(), &mut |o| {
        gemv_f16(&f16_w, rows, cols, &x, o)
    });
    check("Q4_0 (0.5 B)", 0.5, &mut out.clone(), &mut |o| {
        gemv_q4(&q4_w, rows, cols, &x, o)
    });
    check(
        "trit 双导轨标量 (0.25 B)",
        0.25,
        &mut out.clone(),
        &mut |o| rail.gemv(&x, o, Kernel::Scalar),
    );
    check(
        "trit 双导轨掩码 (0.25 B)",
        0.25,
        &mut out.clone(),
        &mut |o| rail.gemv(&x, o, Kernel::Mask),
    );
    check(
        "trit tryte 查表 (0.2 B)",
        0.2,
        &mut out.clone(),
        &mut |o| trytes.gemv(&x, o),
    );
    check("trit 自动选核", 0.25, &mut out.clone(), &mut |o| {
        if rows >= crate::gemv::AUTO_LUT_ROWS {
            trytes.gemv(&x, o)
        } else {
            rail.gemv(&x, o, Kernel::Mask)
        }
    });

    results
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
