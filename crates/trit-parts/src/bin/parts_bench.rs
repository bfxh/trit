//! 训练步的分项计时（找热点用）。
//!
//! `parts-bench --dir <parity 目录> [--iters N]`：分别测「前向」「前向+反向」「更新」，
//! 以及无 GPU 累加时的读回开销（把 update 里的 pull 单独计时）。

use std::path::PathBuf;
use std::time::Instant;

use trit_parts::gpu::Gpu;
use trit_parts::net::Net;
use trit_parts::weights::Tensors;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = PathBuf::from("../trit-data/parts-parity");
    let mut iters = 5usize;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--dir" => {
                dir = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--iters" => {
                iters = argv[i + 1].parse().unwrap_or(5);
                i += 2;
            }
            "--adapter" => {
                // 只用于打印适配器后退出
                let g = Gpu::new().expect("GPU");
                println!("使用适配器：{}", g.adapter);
                return;
            }
            other => {
                eprintln!("未知参数 {other}");
                std::process::exit(2);
            }
        }
    }
    let gpu = Gpu::new().expect("没有可用 GPU");
    println!("使用适配器：{}", gpu.adapter);
    let t = Tensors::load(&dir).expect("权重");
    let mut net = Net::from_tensors(&t).expect("建网");
    net.upload_all(&gpu);

    let res = 384u32;
    let hw = (res * res) as usize;
    let x = vec![0.5f32; hw * 3];
    let dout_data = vec![0.01f32; hw * 16];

    let bench = |name: &str, f: &mut dyn FnMut()| {
        f(); // 预热
        gpu.sync();
        let t0 = Instant::now();
        for _ in 0..iters {
            f();
        }
        gpu.sync();
        println!(
            "{name:<28} {:>8.2} ms/次",
            t0.elapsed().as_secs_f64() * 1000.0 / iters as f64
        );
    };

    bench("forward 全链", &mut || {
        let tr = net.forward(&gpu, gpu.buf(&x, "x"), res);
        gpu.sync();
        drop(tr);
    });

    let tr = net.forward(&gpu, gpu.buf(&x, "x"), res);
    let dout = gpu.buf(&dout_data, "dout");
    gpu.sync();
    bench("backward 全链(含 GPU 累加)", &mut || {
        net.backward(&gpu, &tr, &dout, res);
        gpu.sync();
    });
    bench("update(拉回+STE+Adam+上传)", &mut || {
        net.update(&gpu, 1e-3, trit_train::Ste::Identity, true, 1.0 / 6.0);
        gpu.sync();
    });
    println!("\n注：update 每 accum 次调用一次；backward 里的梯度已在 GPU 累加。");
}
