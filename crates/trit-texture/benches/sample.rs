//! 采样吞吐：**压缩域按需解码** vs **整图解码后再过滤**（同机同构建）。
//!
//! 可复算：`cargo bench -p trit-texture --bench sample`。
//!
//! 口径（诚实记账）：
//! - 两者语义完全一致（纹素中心 + clamp-to-edge + f32 权重 + 四舍五入回 8 位），
//!   差别只在**何时解码**：压缩域每次采样解 4 个块；整图路径先展开成 RGBA 再取 4 个纹素。
//! - 逐样点走**光栅扫描**（片元的真实顺序），不是随机跳——随机访问会放大整图路径的劣势，
//!   那样赢也不代表真实场景。
//! - 计时取 5 次的中位数，先跑一遍做热缓存预热。
//! - 显存账单独列：块流字节 vs RGBA 字节。

use std::hint::black_box;
use std::time::Instant;

use trit_texture::convert::{rgba_to_ttx2_mips, ttx2_to_rgba};
use trit_texture::mip::MipSampler;
use trit_texture::sample::{Sampler, Wrap, sample_aniso, sample_bilinear, texel};
use trit_texture::ttx2::from_ttx2;
use trit_texture::{BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_BLOCK_BYTES, TritTexture};

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
                pixels[offset + 1] = salt;
                pixels[offset + 2] = salt;
            }
        }
    }
    pixels
}

/// 整图路径的双线性：与 `sample::sample_bilinear` 同语义，只是数据已展开成 RGBA。
fn bilinear_rgba(rgba: &[u8], width: usize, height: usize, u: f32, v: f32) -> [u8; 4] {
    let fx = u.clamp(0.0, 1.0) * width as f32 - 0.5;
    let fy = v.clamp(0.0, 1.0) * height as f32 - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = |i: i64| i.clamp(0, width as i64 - 1) as usize;
    let yi = |i: i64| i.clamp(0, height as i64 - 1) as usize;
    let (x0i, x1i) = (xi(x0 as i64), xi(x0 as i64 + 1));
    let (y0i, y1i) = (yi(y0 as i64), yi(y0 as i64 + 1));
    let at = |x: usize, y: usize, c: usize| rgba[(y * width + x) * 4 + c] as f32;
    let mut out = [255u8; 4];
    for (c, slot) in out.iter_mut().enumerate().take(3) {
        let top = at(x0i, y0i, c) * (1.0 - tx) + at(x1i, y0i, c) * tx;
        let bottom = at(x0i, y1i, c) * (1.0 - tx) + at(x1i, y1i, c) * tx;
        *slot = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// 光栅扫描的采样点：(u, v) 序列，覆盖整张贴图。
fn samples(width: usize, height: usize) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            out.push((
                (x as f32 + 0.37) / width as f32,
                (y as f32 + 0.61) / height as f32,
            ));
        }
    }
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn run(width: usize, height: usize) {
    let pixels = synthetic(width, height);
    let bytes = rgba_to_ttx2_mips(&pixels, width, height).expect("encode");
    let doc = from_ttx2(&bytes).expect("parse");
    let levels = doc.levels().expect("texture");
    let tex: &TritTexture = &levels[0];
    assert_eq!((tex.width, tex.height), (width, height));
    let mut mip = MipSampler::with_wrap(levels, Wrap::Repeat);

    let pts = samples(width, height);
    let n = pts.len() as f64;

    // 压缩域：每样点解 4 个块
    let mut tri: Vec<f64> = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let mut acc = 0u64;
        for &(u, v) in &pts {
            let c = sample_bilinear(tex, u, v);
            acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
        }
        black_box(acc);
        tri.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    // 压缩域 + 采样器缓存（一次采样解码的块数：每块只解一次）
    let mut cached: Vec<f64> = Vec::new();
    for _ in 0..5 {
        let mut sampler = Sampler::new(tex);
        let t = Instant::now();
        let mut acc = 0u64;
        for &(u, v) in &pts {
            let c = sampler.sample_bilinear(u, v);
            acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
        }
        black_box(acc);
        cached.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    // 整图：先解码一次（不计入采样时间），再在 RGBA 上双线性
    let decoded = ttx2_to_rgba(&bytes).expect("decode");
    let mut whole: Vec<f64> = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let mut acc = 0u64;
        for &(u, v) in &pts {
            let c = bilinear_rgba(&decoded.rgba, width, height, u, v);
            acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
        }
        black_box(acc);
        whole.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    // 最近邻（压缩域）作参照：解 1 个纹素
    let mut near: Vec<f64> = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let mut acc = 0u64;
        for &(u, v) in &pts {
            let c = texel(
                tex,
                (u * width as f32) as usize,
                (v * height as f32) as usize,
            );
            acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
        }
        black_box(acc);
        near.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }

    // 单点基线：级别数（1 级 vs 2 级）× wrap 模式。用来解释下面 aniso 曲线的斜率，
    // 别把「taps=1 比双线性贵 2 倍」归因到缓存上——Repeat 每纹素一次 i64 rem_euclid 才是大头。
    let modes = [
        ("三线性 lod=0.0 clamp ", 0.0f32, Wrap::Clamp),
        ("三线性 lod=0.0 repeat", 0.0, Wrap::Repeat),
        ("三线性 lod=1.5 clamp ", 1.5, Wrap::Clamp),
        ("三线性 lod=1.5 repeat", 1.5, Wrap::Repeat),
    ];
    let mut base_rows: Vec<(&str, f64)> = Vec::new();
    for (label, lod, wrap) in modes {
        let mut runs = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            let mut acc = 0u64;
            for &(u, v) in &pts {
                let c = doc.sample_trilinear_with(u, v, lod, wrap).expect("tri");
                acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
            }
            black_box(acc);
            runs.push(t.elapsed().as_secs_f64() * 1e9 / n);
        }
        base_rows.push((label, median(runs)));
    }
    let mut mip_runs = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let mut acc = 0u64;
        for &(u, v) in &pts {
            let c = mip.sample_trilinear(u, v, 1.5).expect("mip tri");
            acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
        }
        black_box(acc);
        mip_runs.push(t.elapsed().as_secs_f64() * 1e9 / n);
    }
    let mip_tri = median(mip_runs);

    // 各向异性成本曲线：导数取强各向异性（长轴 64 纹素/像素），让 taps 真正生效。
    // 自由函数与 MipSampler（每级块缓存）在同一轮里交替跑，热漂移对两边一样。
    let du_ani = 64.0 / width as f32;
    let dv_ani = 0.25 / height as f32;
    let mut ani_rows: Vec<(u32, f64, f64)> = Vec::new();
    for &taps in &[1u32, 2, 4, 8] {
        let (mut free_runs, mut mip_runs) = (Vec::new(), Vec::new());
        for _ in 0..5 {
            for which in 0..2 {
                let t = Instant::now();
                let mut acc = 0u64;
                for &(u, v) in &pts {
                    let c = if which == 0 {
                        sample_aniso(&doc, u, v, du_ani, 0.0, 0.0, dv_ani, taps, Wrap::Repeat)
                            .expect("aniso")
                    } else {
                        mip.sample_aniso(u, v, du_ani, 0.0, 0.0, dv_ani, taps)
                            .expect("mip aniso")
                    };
                    acc += c[0] as u64 + c[1] as u64 + c[2] as u64;
                }
                black_box(acc);
                let ns = t.elapsed().as_secs_f64() * 1e9 / n;
                if which == 0 {
                    free_runs.push(ns);
                } else {
                    mip_runs.push(ns);
                }
            }
        }
        ani_rows.push((taps, median(free_runs), median(mip_runs)));
    }

    let blocks = (width / BLOCK_WIDTH) * (height / BLOCK_HEIGHT) * TRIT_BLOCK_BYTES;
    let rgba = width * height * 4;
    println!(
        "== {}x{} 采样（光栅顺序，{} 点/轮，5 轮中位数）==",
        width, height, n as u64
    );
    println!("  最近邻（压缩域）      {:8.2} ns/样点", median(near));
    println!("  双线性（压缩域按需解） {:8.2} ns/样点", median(tri));
    println!("  双线性（采样器+块缓存） {:8.2} ns/样点", median(cached));
    println!("  双线性（整图 RGBA）    {:8.2} ns/样点", median(whole));
    for (label, ns) in &base_rows {
        println!("  {label}  {ns:8.2} ns/样点");
    }
    println!("  三线性 lod=1.5 repeat（MipSampler 缓存）{mip_tri:8.2} ns/样点");
    for (taps, free, mip) in &ani_rows {
        println!(
            "  各向异性 taps={taps:<2}   自由 {free:8.2} / 缓存 {mip:8.2} ns/样点（{:4.2}×）",
            free / mip
        );
    }
    println!(
        "  显存：块流 {:.0} KiB vs RGBA {:.0} KiB（{:.1}×）",
        blocks as f64 / 1024.0,
        rgba as f64 / 1024.0,
        rgba as f64 / blocks as f64
    );
}

fn main() {
    // 小图：RGBA 只有 1 MB，整条链都在缓存里——压缩域没有优势才正常。
    run(512, 520);
    // 大图：受 MAX_TEXTURE_PIXELS = 1<<22 限制，取 2048×2040（RGBA 16 MB / 块流 1.2 MB）。
    run(2048, 2040);
}
