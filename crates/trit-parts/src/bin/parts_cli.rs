//! 多后端推理命令：`parts-cli infer --dir <夹具> --backends refcpu,wgpu`
//!
//! 判据（与 `docs/MULTI-CHIP.md` §5 一致）：
//! - G1 逐元素：各后端与参考的 `max|Δlogit|` 记录在案；
//! - G2 任务：平面 argmax 一致率 ≥ 0.995 才算通过。

use std::path::PathBuf;
use std::time::Instant;

use trit_parts::model::{Backend, RefCpu, TriModel, compare, compare_nhwc};
use trit_parts::parts::PartList;
use trit_parts::tpm::TpmModel;
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
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    if !matches!(cmd, "infer" | "export" | "run") {
        eprintln!(
            "用法:
  parts-cli infer --dir <夹具> [--backends refcpu,wgpu]
  parts-cli export --state <state.bin> --out <model.tpm>
  parts-cli run --model <model.tpm> --image <png> --manifest <parts.json> --out <dir>"
        );
        std::process::exit(2);
    }
    if cmd == "export" {
        export(&argv);
        return;
    }
    if cmd == "run" {
        run(&argv);
        return;
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

// ---------------------------------------------------------------- export / run

/// `export --state <TPNT3> --out <TPM1>`：导出部署包（只含 q/scale/γ/β/bias）。
#[cfg(feature = "gpu")]
fn export(argv: &[String]) {
    let mut state = None;
    let mut out = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--state" => {
                state = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            other => {
                eprintln!("未知参数 {other}");
                std::process::exit(2);
            }
        }
    }
    let (state, out) = match (state, out) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("export 需要 --state 与 --out");
            std::process::exit(2);
        }
    };
    let (net, step, _) = trit_parts::state::load(&state).unwrap_or_else(|e| {
        eprintln!("读存档失败: {e}");
        std::process::exit(1);
    });
    let m = trit_parts::tpm::from_net(&net);
    m.save(&out).unwrap_or_else(|e| {
        eprintln!("写部署包失败: {e}");
        std::process::exit(1);
    });
    let n: usize = m.layers.iter().map(|l| l.q.len()).sum();
    println!(
        "已导出 {}（step {step}，{} 层，{} 个三值权重）",
        out.display(),
        m.layers.len(),
        n
    );
}

#[cfg(not(feature = "gpu"))]
fn export(_argv: &[String]) {
    eprintln!("本构建未启用 gpu feature，无法读训练存档；请在带 gpu 的构建里导出");
    std::process::exit(2);
}

/// `run --model <TPM1> --image <png> --manifest <json> --out <dir>`：
/// 单帧推理 → 清单约束解码 → 写 `ids.bin`（u32 LE）与 `ids.pgm`（16 位灰度，可直接看）。
#[cfg(feature = "gpu")]
fn run(argv: &[String]) {
    use trit_parts::data::read_rgb;

    let mut model = None;
    let mut image = None;
    let mut manifest = None;
    let mut outdir = PathBuf::from(".");
    let mut backend = String::from("wgpu");
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--model" => {
                model = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--image" => {
                image = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--manifest" => {
                manifest = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--out" => {
                outdir = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--backend" => {
                backend = argv[i + 1].clone();
                i += 2;
            }
            other => {
                eprintln!("未知参数 {other}");
                std::process::exit(2);
            }
        }
    }
    let (model, image, manifest) = match (model, image, manifest) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => {
            eprintln!("run 需要 --model / --image / --manifest");
            std::process::exit(2);
        }
    };
    let tpm = TpmModel::load(&model).unwrap_or_else(|e| {
        eprintln!("读部署包失败: {e}");
        std::process::exit(1);
    });
    let pl = PartList::load(&manifest).unwrap_or_else(|e| {
        eprintln!("读清单失败: {e}");
        std::process::exit(1);
    });
    // 推理只吃 RGB：不要求 seg 标签（用户给的图必然没有）
    let (w, _h, rgb) = read_rgb(&image).unwrap_or_else(|e| {
        eprintln!("读图失败: {e}");
        std::process::exit(1);
    });
    let res = w as u32;

    let t0 = std::time::Instant::now();
    let out = match backend.as_str() {
        "refcpu" => {
            let mut b = RefCpu::new(tpm.to_tri());
            b.forward(&rgb, res)
        }
        _ => {
            let mut b = trit_parts::backend_wgpu::WgpuBackend::from_tpm(&tpm)
                .unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));
            b.forward(&rgb, res)
        }
    }
    .unwrap_or_else(|e| {
        eprintln!("推理失败: {e}");
        std::process::exit(1);
    });
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let hw = (res * res) as usize;
    let ch = pl.k * 3 + 1;
    let mut ids = vec![0u32; hw];
    let mut parts = 0usize;
    let mut unk = 0usize;
    for p in 0..hw {
        let vpred = out[p * ch + pl.k * 3] > 0.0;
        if !vpred {
            continue;
        }
        parts += 1;
        let id = pl.decode_constrained(&out[p * ch..p * ch + pl.k * 3]);
        ids[p] = id;
        if !pl.contains(id) {
            unk += 1;
        }
    }
    std::fs::create_dir_all(&outdir).ok();
    let bin: Vec<u8> = ids.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(outdir.join("ids.bin"), bin).ok();
    let mut pgm = format!(
        "P5
{res} {res}
65535
"
    )
    .into_bytes();
    for &id in &ids {
        pgm.extend_from_slice(&(id as u16).to_be_bytes());
    }
    std::fs::write(outdir.join("ids.pgm"), pgm).ok();
    println!(
        "{}  {}²  后端 {backend}  {ms:.1} ms  部件像素 {parts}（{:.2}%）  不同 id {}  清单外 {unk}",
        image.display(),
        res,
        100.0 * parts as f64 / hw as f64,
        {
            let mut u: Vec<u32> = ids.iter().copied().filter(|&v| v > 0).collect();
            u.sort_unstable();
            u.dedup();
            u.len()
        }
    );
    println!(
        "输出：{} 与 {}",
        outdir.join("ids.bin").display(),
        outdir.join("ids.pgm").display()
    );
}

#[cfg(not(feature = "gpu"))]
fn run(_argv: &[String]) {
    eprintln!("本构建未启用 gpu feature；请用带 gpu 的构建，或 --backend refcpu");
    std::process::exit(2);
}
