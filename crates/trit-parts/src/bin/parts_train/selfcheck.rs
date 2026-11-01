use std::path::PathBuf;

use trit_parts::gpu::Gpu;
use trit_parts::net::Net;
use trit_parts::weights::Tensors;

/// 自检：拿 parity 夹具验证「整网前向 == 推理二进制」的口径（三值 vs PyTorch 三值）。
pub(crate) fn selfcheck(dir: &std::path::Path) {
    let gpu = match Gpu::new() {
        Some(g) => g,
        None => {
            eprintln!("没有可用的 GPU 适配器");
            std::process::exit(2);
        }
    };
    let t = Tensors::load(dir).unwrap_or_else(|e| {
        eprintln!("权重读取失败: {e}");
        std::process::exit(1);
    });
    let mut net = Net::from_tensors(&t).unwrap_or_else(|e| {
        eprintln!("建网失败: {e}");
        std::process::exit(1);
    });
    net.upload_all(&gpu);
    let x = read_f32(&dir.join("input.bin"));
    let res = (x.len() / 3).isqrt() as u32;
    let hw = (res * res) as usize;
    let xb = gpu.buf(&x, "x");
    let tr = net.forward(&gpu, xb, res);
    let out = gpu.read(&tr.out, hw * 16);
    let qt = read_f32(&dir.join("logits_trit_py.bin"));
    let mut agree = 0usize;
    let mut tot = 0usize;
    let mut maxd = 0f32;
    for i in 0..5 {
        for p in 0..hw {
            let (mut ra, mut rb) = (0usize, 0usize);
            let (mut va, mut vb) = (f32::MIN, f32::MIN);
            for c in 0..3 {
                let a = out[p * 16 + i * 3 + c];
                let b = qt[(i * 3 + c) * hw + p];
                maxd = maxd.max((a - b).abs());
                if a > va {
                    va = a;
                    ra = c;
                }
                if b > vb {
                    vb = b;
                    rb = c;
                }
            }
            tot += 1;
            agree += usize::from(ra == rb);
        }
    }
    println!(
        "自检：平面 argmax 一致率 {:.6} ({}/{})，max|Δ| {:.3e}",
        agree as f64 / tot as f64,
        agree,
        tot,
        maxd
    );
    if agree != tot {
        println!("（不一致数 {}）", tot - agree);
    }
}

/// `--data` / `--eval` 的目录：单场景（含 `rgb/`）或「场景的父目录」（子目录各含 `rgb/`）。
pub(crate) fn expand_scenes(root: &std::path::Path) -> Vec<PathBuf> {
    if root.join("rgb").is_dir() {
        return vec![root.to_path_buf()];
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.join("rgb").is_dir())
                .collect()
        })
        .unwrap_or_default();
    subs.sort();
    if std::env::var("PARTSTRAIN_DEBUG").is_ok() {
        eprintln!(
            "[debug] expand_scenes {} → {} 个子场景",
            root.display(),
            subs.len()
        );
    }
    if subs.is_empty() {
        eprintln!(
            "{}: 既不是场景（无 rgb/），也没有含 rgb/ 的子目录",
            root.display()
        );
    }
    subs
}

pub(crate) fn read_f32(path: &std::path::Path) -> Vec<f32> {
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
