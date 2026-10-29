//! 多后端推理命令：`parts-cli infer --dir <夹具> --backends refcpu,wgpu`
//!
//! 判据（与 `docs/MULTI-CHIP.md` §5 一致）：
//! - G1 逐元素：各后端与参考的 `max|Δlogit|` 记录在案；
//! - G2 任务：平面 argmax 一致率 ≥ 0.995 才算通过。

use std::path::PathBuf;
use std::time::Instant;

use trit_parts::model::{Backend, RefCpu, TriModel, compare, compare_nhwc};
use trit_parts::weights::Tensors;

fn read_f32(path: &std::path::Path) -> Vec<f32> {
    let raw = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", path.display());
        std::process::exit(1);
    });
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) != Some("infer") {
        eprintln!("用法: parts-cli infer --dir <夹具目录> [--backends refcpu,wgpu]");
        std::process::exit(2);
    }
    let mut dir = PathBuf::from("../trit-data/parts-parity");
    let mut backends = String::from("refcpu,wgpu");
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--dir" => {
                dir = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--backends" => {
                backends = argv[i + 1].clone();
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
        std::process::exit(1);
    });
    let x = read_f32(&dir.join("input.bin"));
    let res = (x.len() / 3).isqrt() as u32;
    let want = read_f32(&dir.join("logits_trit_py.bin")); // PyTorch 同一套三值量化的输出
    println!("输入 {res}²×3，后端 {backends}");

    let mut outs: Vec<(String, Vec<f32>, f64)> = Vec::new();
    for name in backends.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let t0 = Instant::now();
        let out = match name {
            "refcpu" => {
                let m = TriModel::from_tensors(&t).unwrap_or_else(|e| {
                    eprintln!("建模型失败: {e}");
                    std::process::exit(1);
                });
                let mut b = RefCpu::new(m);
                b.forward(&x, res)
            }
            #[cfg(feature = "gpu")]
            "wgpu" => {
                let mut b = trit_parts::backend_wgpu::WgpuBackend::new(&t)
                    .unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));
                b.forward(&x, res)
            }
            #[cfg(not(feature = "gpu"))]
            "wgpu" => Err("本构建未启用 gpu feature".into()),
            other => Err(format!("未知后端 {other}")),
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match out {
            Ok(v) => {
                let (agree, maxd) = compare(&v, &want, res);
                println!(
                    "[{name}] {ms:8.1} ms  平面 argmax {agree:.6}  与 PyTorch 三值 max|Δ| {maxd:.3e}"
                );
                outs.push((name.to_string(), v, ms));
            }
            Err(e) => eprintln!("[{name}] 跳过：{e}"),
        }
    }

    // 后端互相对拍（G1/G2）
    if outs.len() >= 2 {
        let (n0, v0, _) = &outs[0];
        for (n1, v1, _) in &outs[1..] {
            let (agree, maxd) = compare_nhwc(v1, v0, res);
            println!("[{n0} ↔ {n1}] 平面 argmax {agree:.6}  max|Δ| {maxd:.3e}");
            if agree < 0.995 {
                eprintln!("G2 判红：{n0} 与 {n1} 的 argmax 一致率 {agree:.6} < 0.995");
                std::process::exit(1);
            }
        }
    }
}
