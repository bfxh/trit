//! TT3 采样的同机读数：**CPU 参考 vs GPU 内核**（含设备 init 摊销）。
//!
//! 可复算：`cargo bench -p trit-gpu --features gpu --bench sample`。
//!
//! 口径：
//! - 同一张 2048×2040 纹理 + 完整 mip 链，同一批按光栅顺序的采样点（含 lod）。
//! - CPU 侧是 `TtxDocument::sample_trilinear_with`（本仓参考实现）。
//! - GPU 侧是 `sample_tt3_mips` 的**完整一次调用**：建实例 + 选适配器 + 设备 + 上传 +
//!   dispatch + 回读。所以同时给两组数：`含 init`（一次性离屏用法的真实代价）与
//!   `扣掉 init`（引擎复用设备时的净吞吐，init 由 1 点调用的耗时估）。
//! - 5 轮取中位数；GPU 每轮都是完整调用（不复用设备），跟 host API 的实际形态一致。

use std::time::Instant;

use trit_gpu::sampler::{GpuSampler, MipLevel, Wrap, sample_tt3_mips};
use trit_texture::convert::build_mip_chain;
use trit_texture::sample::Wrap as CpuWrap;
use trit_texture::ttx2::TtxDocument;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn synthetic(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height * 4];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            pixels[offset] = (x * 255 / width) as u8;
            pixels[offset + 1] = (y * 255 / height) as u8;
            pixels[offset + 2] = ((x + y) % 64 * 4) as u8;
            pixels[offset + 3] = 255;
            if rng.next().is_multiple_of(64) {
                let salt = (rng.next() % 256) as u8;
                pixels[offset] = salt;
                pixels[offset + 1] = 255 - salt;
                pixels[offset + 2] = salt / 2;
            }
        }
    }
    pixels
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let (width, height) = (2048usize, 2040usize);
    let pixels = synthetic(width, height);
    let t = Instant::now();
    let chain = build_mip_chain(&pixels, width, height).expect("mip chain");
    let build_ms = t.elapsed().as_secs_f64() * 1e3;
    let flat: Vec<Vec<u8>> = chain.iter().map(|t| t.blocks.concat()).collect();
    let levels: Vec<MipLevel> = chain
        .iter()
        .zip(&flat)
        .map(|(t, blocks)| MipLevel {
            blocks,
            width: t.width,
            height: t.height,
            rect: None,
        })
        .collect();
    let doc = TtxDocument::Texture {
        levels: chain.clone(),
    };

    // 光栅顺序 + 缩到 1/4（lod=2）的采样点：4M 个
    let mut pts: Vec<(f32, f32, f32)> = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            pts.push((
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
                2.0,
            ));
        }
    }
    let n = pts.len() as f64;

    // CPU 参考
    let mut cpu_ns: Vec<f64> = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let mut acc = 0u64;
        for &(u, v, lod) in &pts {
            let c = doc
                .sample_trilinear_with(u, v, lod, CpuWrap::Clamp)
                .expect("trilinear");
            acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
        }
        std::hint::black_box(acc);
        cpu_ns.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    // GPU 设备 init 单独估：1 个点的完整调用
    let t = Instant::now();
    let probe = sample_tt3_mips(&levels, &[(0.5, 0.5, 0.0)], Wrap::Clamp);
    let init_ms = t.elapsed().as_secs_f64() * 1e3;
    let Some(_) = probe else {
        println!("没有可用 adapter，跳过 GPU（mip 构建 {build_ms:.0} ms）");
        return;
    };

    // 一次性包装：每次重建设备 + 管线
    let mut oneshot_ns: Vec<f64> = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let out = sample_tt3_mips(&levels, &pts, Wrap::Clamp).expect("gpu");
        std::hint::black_box(out);
        oneshot_ns.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    // 持久采样器：设备/管线建一次，之后只重传点 + dispatch + 回读
    let t = Instant::now();
    let mut sampler = GpuSampler::new(&levels, Wrap::Clamp).expect("sampler");
    let setup_ms = t.elapsed().as_secs_f64() * 1e3;
    let mut gpu_ns: Vec<f64> = Vec::new();
    for _ in 0..7 {
        let t = Instant::now();
        let out = sampler.sample(&pts).expect("sample");
        std::hint::black_box(out);
        gpu_ns.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    let blocks: usize = chain.iter().map(|t| t.blocks.len() * 12).sum();
    let rgba = width * height * 4;
    let oneshot_med = median(oneshot_ns);
    println!(
        "== TT3 三线性采样 {width}×{height}，{} 点（mip {} 级，构建 {build_ms:.0} ms）==",
        pts.len(),
        chain.len()
    );
    println!("  CPU 参考          {:8.2} ns/样点", median(cpu_ns));
    println!(
        "  GPU 一次性调用    {:8.2} ns/样点（每次重建设备，init ≈ {init_ms:.0} ms）",
        oneshot_med
    );
    println!(
        "  GPU 持久采样器    {:8.2} ns/样点（setup 一次 ≈ {setup_ms:.0} ms）",
        median(gpu_ns)
    );
    println!(
        "  常驻：块流 {:.0} KiB vs RGBA {:.0} KiB（{:.1}×）",
        blocks as f64 / 1024.0,
        rgba as f64 / 1024.0,
        rgba as f64 / blocks as f64
    );
}
