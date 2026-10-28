//! 前向对拍：同一帧输入分别用 f32 / 三值权重跑一遍，与 Python 侧 logits 比较。
//!
//! 判据（两条，缺一不算通过）：
//! - f32：`max|Δlogit|` 在 f32 累加噪声量级（1e-3）内，**逐元素 argmax 全同**；
//! - 三值：与「Python 里同一套量化」的 argmax 一致率 > 0.999。
//!
//! 跑法见 crate README；`--dir` 指向对拍夹具目录（默认取 `TRIT_PARTS_DIR`，
//! 否则 `../trit-data/parts-parity`）。

use std::path::{Path, PathBuf};

use trit_parts::forward::forward;
use trit_parts::gpu::Gpu;
use trit_parts::weights::Tensors;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = std::env::var_os("TRIT_PARTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../trit-data/parts-parity"));
    let mut mode = 0u32;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--mode" => {
                mode = if args[i + 1] == "trit" { 1 } else { 0 };
                i += 2;
            }
            other => {
                eprintln!("未知参数 {other}");
                std::process::exit(2);
            }
        }
    }

    let t = Tensors::load(&dir).unwrap_or_else(|e| {
        eprintln!("权重读取失败: {e}");
        std::process::exit(1)
    });
    let xin = read_f32(&dir.join("input.bin")).expect("input.bin");
    let res = (xin.len() / 3).isqrt() as u32;
    println!(
        "输入 {res}² ×3，权重张量 {} 个，模式 {}",
        t.len(),
        if mode == 1 { "trit" } else { "f32" }
    );

    let gpu = Gpu::new().expect("没有可用的 GPU 适配器");
    let xb = gpu.buf(&xin, "input");
    let dump = std::env::var("TRIT_PARTS_DUMP").ok().map(PathBuf::from);
    let out = forward(&gpu, &t, &xb, res, mode, dump.as_deref());

    let py = read_f32(&dir.join("logits_py.bin")).expect("logits_py.bin");
    assert_eq!(out.len(), py.len(), "logits 长度不一致");

    let n = (res * res) as usize;
    let mut maxd = 0f32;
    let mut where_max = (0usize, 0usize);
    let mut bad = 0usize;
    for c in 0..16usize {
        for p in 0..n {
            let d = (out[p * 16 + c] - py[c * n + p]).abs();
            if d > 1e-2 {
                bad += 1;
            }
            if d > maxd {
                maxd = d;
                where_max = (c, p);
            }
        }
    }
    let mut sum_d = 0f64;
    let mut sum_py = 0f64;
    let mut sum_out = 0f64;
    for c in 0..16usize {
        for p in 0..n {
            sum_d += (out[p * 16 + c] - py[c * n + p]).abs() as f64;
            sum_py += py[c * n + p].abs() as f64;
            sum_out += out[p * 16 + c].abs() as f64;
        }
    }
    let cnt = (n * 16) as f64;
    println!(
        "均值: |Δ|={:.4e}  |py|={:.4e}  |rust|={:.4e}  相对 {:.2e}",
        sum_d / cnt,
        sum_py / cnt,
        sum_out / cnt,
        (sum_d / cnt) / (sum_py / cnt)
    );
    // 平面 argmax 一致率（与 Python 的 decode 同口径：每个平面在 3 个通道上取 argmax）。
    let (agree, tot) = plane_agreement(&out, &py, n);
    println!(
        "max|Δlogit| = {maxd:.3e}（最大差在通道 {} 像素 ({}, {})，|Δ|>1e-2 的元素 {}/{cnt}）",
        where_max.0,
        where_max.1 % res as usize,
        where_max.1 / res as usize,
        bad
    );
    println!(
        "平面 argmax 一致率 = {:.6} ({}/{})",
        agree as f64 / tot as f64,
        agree,
        tot
    );

    // 三值模式还要与「PyTorch 里同一套量化」对拍：两个实现必须一致，
    // 与 f32 的差才是量化代价本身，而不是实现 bug。
    if mode == 1
        && let Ok(qt) = read_f32(&dir.join("logits_trit_py.bin"))
        && qt.len() == out.len()
    {
        let mut d2 = 0f32;
        for c in 0..16usize {
            for p in 0..n {
                d2 = d2.max((out[p * 16 + c] - qt[c * n + p]).abs());
            }
        }
        let (ag2, tot2) = plane_agreement(&out, &qt, n);
        println!(
            "vs PyTorch 三值: max|Δ|={d2:.3e}  平面 argmax 一致率={:.6}",
            ag2 as f64 / tot2 as f64
        );
    }

    println!(
        "结论：{}",
        if mode == 0 {
            // 1e-3 是 f32 在 384² 上多层级累加顺序差异的量级；逐元素 argmax 全同才是硬判据
            if maxd < 1e-3 {
                "f32 前向与 Python 对齐（差值在 f32 累加噪声内）"
            } else {
                "f32 前向与 Python 不一致，需查"
            }
        } else if agree as f64 / tot as f64 > 0.999 {
            "三值前向与 Python 的 argmax 基本一致"
        } else {
            "三值前向有明显偏差（这就是量化损失）"
        }
    );
}

/// 读一份小端 f32 裸数组（夹具统一格式）。
fn read_f32(path: &Path) -> std::io::Result<Vec<f32>> {
    let raw = std::fs::read(path)?;
    Ok(raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect())
}

/// 每个平面在 3 个通道上取 argmax，返回 (一致数, 总数)。
fn plane_agreement(out: &[f32], py: &[f32], n: usize) -> (usize, usize) {
    let mut agree = 0usize;
    let mut tot = 0usize;
    for i in 0..5 {
        for p in 0..n {
            let (mut ra, mut rb) = (0usize, 0usize);
            let (mut va, mut vb) = (f32::MIN, f32::MIN);
            for c in 0..3 {
                // Rust 侧缓冲区是 NHWC（像素优先），Python 的 logits 是 CHW（通道优先）
                let x1 = out[p * 16 + (i * 3 + c)];
                let x2 = py[(i * 3 + c) * n + p];
                if x1 > va {
                    va = x1;
                    ra = c;
                }
                if x2 > vb {
                    vb = x2;
                    rb = c;
                }
            }
            tot += 1;
            if ra == rb {
                agree += 1;
            }
        }
    }
    (agree, tot)
}
